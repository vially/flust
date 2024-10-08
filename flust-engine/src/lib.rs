pub mod builder;
pub mod channel;
pub mod codec;
pub mod compositor;
pub mod error;
pub mod ffi;
mod flutter_callbacks;
pub mod plugins;
pub mod tasks;
pub mod view;

pub mod texture_registry;

use crate::builder::FlutterEngineBuilder;
use crate::channel::{Channel, ChannelRegistry};

use crate::channel::platform_message::{PlatformMessage, PlatformMessageResponseHandle};
use crate::tasks::TaskRunner;
use crate::texture_registry::{Texture, TextureRegistry};
use compositor::FlutterCompositorHandler;
use crossbeam_channel::{unbounded, Receiver, Sender};
use ffi::{
    FlutterEngineAOTData, FlutterEngineDisplay, FlutterEngineDisplaysUpdateType,
    FlutterEngineResult, FlutterEngineResultExt, FlutterKeyEvent, FlutterPointerEvent,
    FlutterViewId,
};
use flust_engine_api::FlutterOpenGLHandler;
use flust_engine_sys::{
    FlutterCompositor, FlutterEngineDisplayId, FlutterEngineGetCurrentTime,
    FlutterEngineRunsAOTCompiledDartCode, FlutterTask, VsyncCallback,
};
use parking_lot::{Mutex, RwLock};
use std::ffi::{c_void, CString};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};
use thiserror::Error;
use tracing::trace;
use view::{FlutterView, ViewRegistry};

pub(crate) type MainThreadEngineFn = Box<dyn FnOnce(&FlutterEngine) + Send>;
pub(crate) type MainThreadRenderThreadFn = Box<dyn FnOnce(&FlutterEngine) + Send>;

pub(crate) enum MainThreadCallback {
    Engine(MainThreadEngineFn),
    RenderThread(MainThreadRenderThreadFn),
}

struct FlutterEngineInner {
    view_registry: RwLock<ViewRegistry>,
    vsync_handler: Option<Arc<Mutex<dyn FlutterVsyncHandler + Send>>>,
    engine_ptr: flust_engine_sys::FlutterEngine,
    channel_registry: RwLock<ChannelRegistry>,
    platform_runner: TaskRunner,
    platform_receiver: Receiver<MainThreadCallback>,
    platform_sender: Sender<MainThreadCallback>,
    texture_registry: TextureRegistry,
    aot_data: FlutterEngineAOTData,
    assets: PathBuf,
    icu_data: PathBuf,
    persistent_cache: PathBuf,
    arguments: Vec<String>,
}

impl FlutterEngineInner {
    fn implicit_view_opengl_handler(&self) -> Option<Arc<dyn FlutterOpenGLHandler>> {
        self.view_registry.read().implicit_view_opengl_handler()
    }

    fn compositor_handler_for_view(
        &self,
        view_id: FlutterViewId,
    ) -> Option<Arc<dyn FlutterCompositorHandler>> {
        self.view_registry
            .read()
            .compositor_handler_for_view(view_id)
    }
}

pub struct FlutterEngineWeakRef {
    inner: Weak<FlutterEngineInner>,
}

unsafe impl Send for FlutterEngineWeakRef {}

unsafe impl Sync for FlutterEngineWeakRef {}

impl FlutterEngineWeakRef {
    pub fn upgrade(&self) -> Option<FlutterEngine> {
        self.inner.upgrade().map(|arc| FlutterEngine { inner: arc })
    }

    pub fn is_valid(&self) -> bool {
        self.upgrade().is_some()
    }

    pub fn ptr_equal(&self, other: Self) -> bool {
        self.inner.ptr_eq(&other.inner)
    }
}

impl Default for FlutterEngineWeakRef {
    fn default() -> Self {
        Self { inner: Weak::new() }
    }
}

impl Clone for FlutterEngineWeakRef {
    fn clone(&self) -> Self {
        Self {
            inner: Weak::clone(&self.inner),
        }
    }
}

pub struct FlutterEngine {
    inner: Arc<FlutterEngineInner>,
}

unsafe impl Send for FlutterEngine {}

unsafe impl Sync for FlutterEngine {}

impl Clone for FlutterEngine {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

pub trait FlutterVsyncHandler {
    fn request_frame_callback(&self, baton: isize);
}

impl FlutterEngine {
    pub(crate) fn new(builder: FlutterEngineBuilder) -> Result<Self, CreateError> {
        // Convert arguments into flutter compatible
        //
        // FlutterProjectArgs expects a full argv, so when processing it for flags
        // the first item is treated as the executable and ignored. Add a dummy value
        // so that all switches are used.
        let dummy_args: Vec<String> = vec!["flust".into()];
        let args = [
            dummy_args,
            FlutterEngine::args_from_env_vars(),
            builder.args.clone(),
        ]
        .concat();

        let mut args: Vec<_> = args
            .iter()
            .map(|arg| CString::new(arg.as_str()).unwrap().into_raw())
            .collect();

        let (main_tx, main_rx) = unbounded();

        let engine = Self {
            #[allow(clippy::arc_with_non_send_sync)]
            inner: Arc::new(FlutterEngineInner {
                view_registry: RwLock::new(ViewRegistry::default()),
                vsync_handler: builder.vsync_handler,
                engine_ptr: ptr::null_mut(),
                channel_registry: RwLock::new(ChannelRegistry::new()),
                platform_runner: TaskRunner::new(
                    builder.platform_handler.expect("No platform runner set"),
                ),
                platform_receiver: main_rx,
                platform_sender: main_tx,
                texture_registry: TextureRegistry::new(),
                aot_data: FlutterEngineAOTData::new(&builder.aot_library)?,
                assets: builder.assets,
                icu_data: builder.icu_data,
                persistent_cache: builder.persistent_cache,
                arguments: builder.args,
            }),
        };

        let inner = &engine.inner;
        inner.channel_registry.write().init(engine.downgrade());
        inner.platform_runner.init(engine.downgrade());

        // Configure renderer
        let renderer_config = flust_engine_sys::FlutterRendererConfig {
            type_: flust_engine_sys::FlutterRendererType::kOpenGL,
            __bindgen_anon_1: flust_engine_sys::FlutterRendererConfig__bindgen_ty_1 {
                open_gl: flust_engine_sys::FlutterOpenGLRendererConfig {
                    struct_size: std::mem::size_of::<flust_engine_sys::FlutterOpenGLRendererConfig>(
                    ),
                    make_current: Some(flutter_callbacks::make_current),
                    clear_current: Some(flutter_callbacks::clear_current),
                    present: Some(flutter_callbacks::present),
                    fbo_callback: None,
                    make_resource_current: Some(flutter_callbacks::make_resource_current),
                    fbo_reset_after_present: false,
                    surface_transformation: None,
                    gl_proc_resolver: Some(flutter_callbacks::gl_proc_resolver),
                    gl_external_texture_frame_callback: Some(
                        flutter_callbacks::gl_external_texture_frame,
                    ),
                    fbo_with_frame_info_callback: Some(
                        flutter_callbacks::fbo_with_frame_info_callback,
                    ),
                    present_with_info: None,
                    populate_existing_damage: None,
                },
            },
        };

        // Configure engine threads
        let runner_ptr = {
            let arc = inner.platform_runner.clone().inner;
            Weak::into_raw(Arc::downgrade(&arc)) as *mut std::ffi::c_void
        };

        let platform_task_runner = flust_engine_sys::FlutterTaskRunnerDescription {
            struct_size: std::mem::size_of::<flust_engine_sys::FlutterTaskRunnerDescription>(),
            user_data: runner_ptr,
            runs_task_on_current_thread_callback: Some(
                flutter_callbacks::runs_task_on_current_thread,
            ),
            post_task_callback: Some(flutter_callbacks::post_task),
            identifier: 0,
        };
        let custom_task_runners = flust_engine_sys::FlutterCustomTaskRunners {
            struct_size: std::mem::size_of::<flust_engine_sys::FlutterCustomTaskRunners>(),
            platform_task_runner: &platform_task_runner
                as *const flust_engine_sys::FlutterTaskRunnerDescription,
            render_task_runner: std::ptr::null(),
            thread_priority_setter: None,
        };

        let vsync_callback: VsyncCallback = match inner.vsync_handler {
            Some(_) => Some(flutter_callbacks::vsync_callback),
            None => None,
        };

        let compositor: *const FlutterCompositor = match builder.compositor_enabled {
            false => std::ptr::null(),
            true => &FlutterCompositor {
                struct_size: std::mem::size_of::<FlutterCompositor>(),
                user_data: Weak::into_raw(Arc::downgrade(inner)) as *mut std::ffi::c_void,
                create_backing_store_callback: Some(
                    flutter_callbacks::compositor_backing_store_create_callback,
                ),
                collect_backing_store_callback: Some(
                    flutter_callbacks::compositor_backing_store_collect_callback,
                ),
                present_layers_callback: None,
                present_view_callback: Some(flutter_callbacks::compositor_present_view_callback),
                avoid_backing_store_cache: false,
            } as *const FlutterCompositor,
        };

        // Configure engine
        let project_args = flust_engine_sys::FlutterProjectArgs {
            struct_size: std::mem::size_of::<flust_engine_sys::FlutterProjectArgs>(),
            assets_path: path_to_cstring(&inner.assets).into_raw(),
            main_path__unused__: std::ptr::null(),
            packages_path__unused__: std::ptr::null(),
            icu_data_path: path_to_cstring(&inner.icu_data).into_raw(),
            command_line_argc: args.len() as i32,
            command_line_argv: args.as_mut_ptr() as _,
            platform_message_callback: Some(flutter_callbacks::platform_message_callback),
            vm_snapshot_data: std::ptr::null(),
            vm_snapshot_data_size: 0,
            vm_snapshot_instructions: std::ptr::null(),
            vm_snapshot_instructions_size: 0,
            isolate_snapshot_data: std::ptr::null(),
            isolate_snapshot_data_size: 0,
            isolate_snapshot_instructions: std::ptr::null(),
            isolate_snapshot_instructions_size: 0,
            root_isolate_create_callback: Some(flutter_callbacks::root_isolate_create_callback),
            update_semantics_node_callback: None,
            update_semantics_custom_action_callback: None,
            persistent_cache_path: path_to_cstring(&inner.persistent_cache).into_raw(),
            is_persistent_cache_read_only: false,
            vsync_callback,
            custom_dart_entrypoint: std::ptr::null(),
            custom_task_runners: &custom_task_runners
                as *const flust_engine_sys::FlutterCustomTaskRunners,
            shutdown_dart_vm_when_done: true,
            compositor,
            dart_old_gen_heap_size: -1,
            aot_data: inner.aot_data.data,
            compute_platform_resolved_locale_callback: None,
            dart_entrypoint_argc: 0,
            dart_entrypoint_argv: std::ptr::null(),
            log_message_callback: None,
            log_tag: std::ptr::null(),
            on_pre_engine_restart_callback: None,
            update_semantics_callback: None,
            update_semantics_callback2: None,
            channel_update_callback: None,
        };

        // Initialise engine
        unsafe {
            let inner_ptr = Weak::into_raw(Arc::downgrade(inner)) as *mut std::ffi::c_void;

            if flust_engine_sys::FlutterEngineInitialize(
                1,
                &renderer_config,
                &project_args,
                inner_ptr,
                &inner.engine_ptr as *const flust_engine_sys::FlutterEngine
                    as *mut flust_engine_sys::FlutterEngine,
            ) != flust_engine_sys::FlutterEngineResult::kSuccess
                || inner.engine_ptr.is_null()
            {
                Err(CreateError::EnginePtrNull)
            } else {
                Ok(engine)
            }
        }
    }

    pub fn get_current_time_duration() -> Duration {
        let current_time_nanos = unsafe { FlutterEngineGetCurrentTime() };
        Duration::from_nanos(current_time_nanos)
    }

    pub fn runs_aot_compiled_dart_code() -> bool {
        unsafe { FlutterEngineRunsAOTCompiledDartCode() }
    }

    #[inline]
    pub fn engine_ptr(&self) -> flust_engine_sys::FlutterEngine {
        self.inner.engine_ptr
    }

    fn args_from_env_vars() -> Vec<String> {
        let mut args: Vec<String> = vec![];

        // Allow enabling verbose engine logging though an environment variable.
        if let Ok(verbose) = std::env::var("FLUTTER_ENGINE_VERBOSE_LOGGING") {
            if verbose == "1" || verbose.to_lowercase() == "true" {
                args.push("--verbose-logging".into());
            }
        }

        args
    }

    pub fn register_channel<C>(&self, channel: C) -> Weak<C>
    where
        C: Channel + 'static,
    {
        trace!("register channel: {}", channel.name());
        self.inner
            .channel_registry
            .write()
            .register_channel(channel)
    }

    pub fn remove_channel(&self, channel_name: &str) -> Option<Arc<dyn Channel>> {
        trace!("remove channel: {}", channel_name);
        self.inner
            .channel_registry
            .write()
            .remove_channel(channel_name)
    }

    pub fn with_channel<F>(&self, channel_name: &str, f: F)
    where
        F: FnOnce(&dyn Channel),
    {
        self.inner
            .channel_registry
            .read()
            .with_channel(channel_name, f)
    }

    pub fn downgrade(&self) -> FlutterEngineWeakRef {
        FlutterEngineWeakRef {
            inner: Arc::downgrade(&self.inner),
        }
    }

    pub fn assets(&self) -> &Path {
        &self.inner.assets
    }

    pub fn arguments(&self) -> &Vec<String> {
        &self.inner.arguments
    }

    pub fn run(&self) -> Result<(), FlutterEngineError> {
        if !self.is_platform_thread() {
            panic!("Not on platform thread");
        }

        let result = unsafe { flust_engine_sys::FlutterEngineRunInitialized(self.engine_ptr()) };
        FlutterEngineResult::from_ffi(result)
    }

    pub fn add_view(&self, view: FlutterView) {
        self.inner.view_registry.write().add_view(view);
    }

    pub fn remove_view(&self, view_id: FlutterViewId) {
        self.inner.view_registry.write().remove_view(view_id);
    }

    pub(crate) fn post_platform_callback(&self, callback: MainThreadCallback) {
        trace!("post_platform_callback");
        self.inner.platform_sender.send(callback).unwrap();
        self.inner.platform_runner.wake();
    }

    #[inline]
    pub fn is_platform_thread(&self) -> bool {
        self.inner.platform_runner.runs_task_on_current_thread()
    }

    pub fn run_on_platform_thread<F>(&self, f: F)
    where
        F: FnOnce(&FlutterEngine) + 'static + Send,
    {
        trace!("run_on_platform_thread");
        if self.is_platform_thread() {
            f(self);
        } else {
            self.post_platform_callback(MainThreadCallback::Engine(Box::new(f)));
        }
    }

    pub fn run_on_render_thread<F>(&self, f: F)
    where
        F: FnOnce(&FlutterEngine) + 'static + Send,
    {
        trace!("run_on_render_thread");
        // TODO: Reimplement render thread
        // if self.is_platform_thread() {
        //     f(self);
        // } else {
        self.post_platform_callback(MainThreadCallback::RenderThread(Box::new(f)));
        // }
    }

    pub fn on_vsync(
        &self,
        baton: isize,
        frame_start_time_nanos: u64,
        frame_target_time_nanos: u64,
    ) {
        trace!("on_vsync");
        if !self.is_platform_thread() {
            panic!("Not on platform thread");
        }

        unsafe {
            flust_engine_sys::FlutterEngineOnVsync(
                self.engine_ptr(),
                baton,
                frame_start_time_nanos,
                frame_target_time_nanos,
            );
        }
    }

    pub fn send_window_metrics_event(
        &self,
        view_id: FlutterViewId,
        width: usize,
        height: usize,
        pixel_ratio: f64,
        display_id: FlutterEngineDisplayId,
    ) {
        trace!("send_window_metrics_event");
        if !self.is_platform_thread() {
            panic!("Not on platform thread");
        }

        let event = flust_engine_sys::FlutterWindowMetricsEvent {
            struct_size: std::mem::size_of::<flust_engine_sys::FlutterWindowMetricsEvent>(),
            width,
            height,
            pixel_ratio,
            left: 0,
            top: 0,
            physical_view_inset_top: 0.0,
            physical_view_inset_right: 0.0,
            physical_view_inset_bottom: 0.0,
            physical_view_inset_left: 0.0,
            display_id,
            view_id,
            #[cfg(all(target_arch = "arm", target_os = "android"))]
            __bindgen_padding_0: 0,
        };
        unsafe {
            flust_engine_sys::FlutterEngineSendWindowMetricsEvent(self.engine_ptr(), &event);
        }
    }

    pub fn send_pointer_event(&self, event: FlutterPointerEvent) {
        if !self.is_platform_thread() {
            panic!("Not on platform thread");
        }

        unsafe {
            flust_engine_sys::FlutterEngineSendPointerEvent(self.engine_ptr(), &event.into(), 1);
        }
    }

    // TODO: Add support for key event callbacks
    pub fn send_key_event(&self, event: FlutterKeyEvent) {
        if !self.is_platform_thread() {
            panic!("Not on platform thread");
        }

        unsafe {
            flust_engine_sys::FlutterEngineSendKeyEvent(
                self.engine_ptr(),
                &event.as_ptr(),
                None,
                ptr::null_mut(),
            );
        }
    }

    pub fn notify_display_update(
        &self,
        update_type: FlutterEngineDisplaysUpdateType,
        displays: Vec<FlutterEngineDisplay>,
    ) {
        trace!("notify_display_update");
        if !self.is_platform_thread() {
            panic!("Not on platform thread");
        }

        let displays: Vec<flust_engine_sys::FlutterEngineDisplay> =
            displays.iter().map(|display| (*display).into()).collect();

        unsafe {
            flust_engine_sys::FlutterEngineNotifyDisplayUpdate(
                self.engine_ptr(),
                update_type.into(),
                displays.as_ptr(),
                displays.len(),
            );
        }
    }

    pub(crate) fn send_platform_message(&self, message: PlatformMessage) {
        trace!("Sending message on channel {}", message.channel);
        if !self.is_platform_thread() {
            panic!("Not on platform thread");
        }

        unsafe {
            flust_engine_sys::FlutterEngineSendPlatformMessage(self.engine_ptr(), &message.into());
        }
    }

    pub(crate) fn send_platform_message_response(
        &self,
        response_handle: PlatformMessageResponseHandle,
        bytes: &[u8],
    ) {
        trace!("Sending message response");
        if !self.is_platform_thread() {
            panic!("Not on platform thread");
        }

        unsafe {
            flust_engine_sys::FlutterEngineSendPlatformMessageResponse(
                self.engine_ptr(),
                response_handle.into(),
                bytes.as_ptr(),
                bytes.len(),
            );
        }
    }

    pub fn shutdown(&self) {
        trace!("shutdown");
        if !self.is_platform_thread() {
            panic!("Not on platform thread")
        }

        unsafe {
            flust_engine_sys::FlutterEngineShutdown(self.engine_ptr());
        }
    }

    pub fn execute_platform_tasks(&self) -> Option<Instant> {
        if !self.is_platform_thread() {
            panic!("Not on platform thread")
        }

        let next_task = self.inner.platform_runner.execute_tasks();

        let mut render_thread_fns = Vec::new();
        let callbacks: Vec<MainThreadCallback> = self.inner.platform_receiver.try_iter().collect();
        for cb in callbacks {
            match cb {
                MainThreadCallback::Engine(func) => func(self),
                MainThreadCallback::RenderThread(f) => render_thread_fns.push(f),
            }
        }
        if !render_thread_fns.is_empty() {
            let engine_copy = self.clone();
            self.post_render_thread_task(move || {
                for f in render_thread_fns {
                    f(&engine_copy);
                }
            });
        }

        next_task
    }

    pub(crate) fn run_task(&self, task: &FlutterTask) {
        trace!("run_task");
        unsafe {
            flust_engine_sys::FlutterEngineRunTask(self.engine_ptr(), task as *const FlutterTask);
        }
    }

    fn post_render_thread_task<F>(&self, f: F)
    where
        F: FnOnce() + 'static,
    {
        trace!("post_render_thread_task");
        unsafe {
            let cbk = CallbackBox { cbk: Box::new(f) };
            let b = Box::new(cbk);
            let ptr = Box::into_raw(b);
            flust_engine_sys::FlutterEnginePostRenderThreadTask(
                self.engine_ptr(),
                Some(render_thread_task),
                ptr as *mut c_void,
            );
        }

        struct CallbackBox {
            pub cbk: Box<dyn FnOnce()>,
        }

        unsafe extern "C" fn render_thread_task(user_data: *mut c_void) {
            let ptr = user_data as *mut CallbackBox;
            let b = Box::from_raw(ptr);
            (b.cbk)()
        }
    }

    pub fn create_texture(&self) -> Texture {
        self.inner.texture_registry.create_texture(self.clone())
    }
}

#[cfg(unix)]
fn path_to_cstring(path: &Path) -> CString {
    use std::os::unix::ffi::OsStrExt;
    CString::new(path.as_os_str().as_bytes()).unwrap()
}

#[cfg(not(unix))]
fn path_to_cstring(path: &Path) -> CString {
    CString::new(path.to_string_lossy().to_string()).unwrap()
}

#[derive(Error, Debug)]
pub enum CreateError {
    #[error("Invalid AOT data")]
    InvalidAOTData(#[from] FlutterEngineError),

    #[error("Engine pointer is null")]
    EnginePtrNull,
}

#[derive(Error, Debug)]
pub enum FlutterEngineError {
    #[error("Invalid library version")]
    InvalidLibraryVersion,

    #[error("Invalid arguments")]
    InvalidArguments,

    #[error("Internal inconsistency")]
    InternalInconsistency,
}
