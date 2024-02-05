pub mod builder;
pub mod channel;
pub mod codec;
pub mod error;
pub mod ffi;
mod flutter_callbacks;
pub mod plugins;
pub mod tasks;
pub mod view;

pub mod texture_registry;

use crate::builder::FlutterEngineBuilder;
use crate::channel::{Channel, ChannelRegistry};
use crate::ffi::{
    FlutterPointerDeviceKind, FlutterPointerMouseButtons, FlutterPointerPhase,
    FlutterPointerSignalKind,
};

use crate::channel::platform_message::{PlatformMessage, PlatformMessageResponseHandle};
use crate::tasks::TaskRunner;
use crate::texture_registry::{Texture, TextureRegistry};
use crossbeam_channel::{unbounded, Receiver, Sender};
use flutter_engine_sys::{FlutterEngineResult, FlutterTask, VsyncCallback};
use log::trace;
use parking_lot::RwLock;
use std::ffi::CString;
use std::os::raw::{c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use std::{mem, ptr};
use thiserror::Error;
use view::{FlutterView, ViewRegistry};

pub(crate) type MainThreadEngineFn = Box<dyn FnOnce(&FlutterEngine) + Send>;
pub(crate) type MainThreadRenderThreadFn = Box<dyn FnOnce(&FlutterEngine) + Send>;

pub(crate) enum MainThreadCallback {
    Engine(MainThreadEngineFn),
    RenderThread(MainThreadRenderThreadFn),
}

struct FlutterEngineInner {
    view_registry: RwLock<ViewRegistry>,
    vsync_handler: Option<Box<dyn FlutterVsyncHandler + Send>>,
    engine_ptr: flutter_engine_sys::FlutterEngine,
    channel_registry: RwLock<ChannelRegistry>,
    platform_runner: TaskRunner,
    platform_receiver: Receiver<MainThreadCallback>,
    platform_sender: Sender<MainThreadCallback>,
    texture_registry: TextureRegistry,
    assets: PathBuf,
    icu_data: PathBuf,
    arguments: Vec<String>,
}

impl FlutterEngineInner {
    fn implicit_view_opengl_handler(&self) -> Option<Arc<dyn FlutterOpenGLHandler>> {
        self.view_registry.read().implicit_view_opengl_handler()
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

pub trait FlutterOpenGLHandler {
    fn swap_buffers(&self) -> bool;

    fn make_current(&self) -> bool;

    fn clear_current(&self) -> bool;

    fn fbo_callback(&self) -> u32;

    fn make_resource_current(&self) -> bool;

    fn gl_proc_resolver(&self, proc: *const c_char) -> *mut c_void;
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
        let mut args = Vec::with_capacity(builder.args.len() + 1);
        args.push(CString::new("flutter-rs").unwrap().into_raw());
        for arg in builder.args.iter() {
            args.push(CString::new(arg.as_str()).unwrap().into_raw());
        }

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
                assets: builder.assets,
                icu_data: builder.icu_data,
                arguments: builder.args,
            }),
        };

        let inner = &engine.inner;
        inner.channel_registry.write().init(engine.downgrade());
        inner.platform_runner.init(engine.downgrade());

        // Configure renderer
        let renderer_config = flutter_engine_sys::FlutterRendererConfig {
            type_: flutter_engine_sys::FlutterRendererType::kOpenGL,
            __bindgen_anon_1: flutter_engine_sys::FlutterRendererConfig__bindgen_ty_1 {
                open_gl: flutter_engine_sys::FlutterOpenGLRendererConfig {
                    struct_size: std::mem::size_of::<flutter_engine_sys::FlutterOpenGLRendererConfig>(
                    ),
                    make_current: Some(flutter_callbacks::make_current),
                    clear_current: Some(flutter_callbacks::clear_current),
                    present: Some(flutter_callbacks::present),
                    fbo_callback: Some(flutter_callbacks::fbo_callback),
                    make_resource_current: Some(flutter_callbacks::make_resource_current),
                    fbo_reset_after_present: false,
                    surface_transformation: None,
                    gl_proc_resolver: Some(flutter_callbacks::gl_proc_resolver),
                    gl_external_texture_frame_callback: Some(
                        flutter_callbacks::gl_external_texture_frame,
                    ),
                    fbo_with_frame_info_callback: None,
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

        let platform_task_runner = flutter_engine_sys::FlutterTaskRunnerDescription {
            struct_size: std::mem::size_of::<flutter_engine_sys::FlutterTaskRunnerDescription>(),
            user_data: runner_ptr,
            runs_task_on_current_thread_callback: Some(
                flutter_callbacks::runs_task_on_current_thread,
            ),
            post_task_callback: Some(flutter_callbacks::post_task),
            identifier: 0,
        };
        let custom_task_runners = flutter_engine_sys::FlutterCustomTaskRunners {
            struct_size: std::mem::size_of::<flutter_engine_sys::FlutterCustomTaskRunners>(),
            platform_task_runner: &platform_task_runner
                as *const flutter_engine_sys::FlutterTaskRunnerDescription,
            render_task_runner: std::ptr::null(),
            thread_priority_setter: None,
        };

        let vsync_callback: VsyncCallback = match inner.vsync_handler {
            Some(_) => Some(flutter_callbacks::vsync_callback),
            None => None,
        };

        // Configure engine
        let project_args = flutter_engine_sys::FlutterProjectArgs {
            struct_size: std::mem::size_of::<flutter_engine_sys::FlutterProjectArgs>(),
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
            persistent_cache_path: std::ptr::null(),
            is_persistent_cache_read_only: false,
            vsync_callback,
            custom_dart_entrypoint: std::ptr::null(),
            custom_task_runners: &custom_task_runners
                as *const flutter_engine_sys::FlutterCustomTaskRunners,
            shutdown_dart_vm_when_done: true,
            compositor: std::ptr::null(),
            dart_old_gen_heap_size: -1,
            aot_data: std::ptr::null_mut(),
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

            if flutter_engine_sys::FlutterEngineInitialize(
                1,
                &renderer_config,
                &project_args,
                inner_ptr,
                &inner.engine_ptr as *const flutter_engine_sys::FlutterEngine
                    as *mut flutter_engine_sys::FlutterEngine,
            ) != flutter_engine_sys::FlutterEngineResult::kSuccess
                || inner.engine_ptr.is_null()
            {
                Err(CreateError::EnginePtrNull)
            } else {
                Ok(engine)
            }
        }
    }

    #[inline]
    pub fn engine_ptr(&self) -> flutter_engine_sys::FlutterEngine {
        self.inner.engine_ptr
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

    pub fn run(&self) -> Result<(), RunError> {
        if !self.is_platform_thread() {
            panic!("Not on platform thread");
        }

        unsafe {
            match flutter_engine_sys::FlutterEngineRunInitialized(self.engine_ptr()) {
                FlutterEngineResult::kSuccess => Ok(()),
                FlutterEngineResult::kInvalidLibraryVersion => Err(RunError::InvalidLibraryVersion),
                FlutterEngineResult::kInvalidArguments => Err(RunError::InvalidArguments),
                FlutterEngineResult::kInternalInconsistency => Err(RunError::InternalInconsistency),
            }
        }
    }

    pub fn add_view(&self, view: FlutterView) {
        self.inner.view_registry.write().add_view(view);
    }

    pub fn remove_view(&self, view_id: u32) {
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
            flutter_engine_sys::FlutterEngineOnVsync(
                self.engine_ptr(),
                baton,
                frame_start_time_nanos,
                frame_target_time_nanos,
            );
        }
    }

    pub fn send_window_metrics_event(&self, width: usize, height: usize, pixel_ratio: f64) {
        trace!("send_window_metrics_event");
        if !self.is_platform_thread() {
            panic!("Not on platform thread");
        }

        let event = flutter_engine_sys::FlutterWindowMetricsEvent {
            struct_size: std::mem::size_of::<flutter_engine_sys::FlutterWindowMetricsEvent>(),
            width,
            height,
            pixel_ratio,
            left: 0,
            top: 0,
            physical_view_inset_top: 0.0,
            physical_view_inset_right: 0.0,
            physical_view_inset_bottom: 0.0,
            physical_view_inset_left: 0.0,
            display_id: 0,
            #[cfg(all(target_arch = "arm", target_os = "android"))]
            __bindgen_padding_0: 0,
        };
        unsafe {
            flutter_engine_sys::FlutterEngineSendWindowMetricsEvent(self.engine_ptr(), &event);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn send_pointer_event(
        &self,
        device: i32,
        phase: FlutterPointerPhase,
        (x, y): (f64, f64),
        signal_kind: FlutterPointerSignalKind,
        (scroll_delta_x, scroll_delta_y): (f64, f64),
        device_kind: FlutterPointerDeviceKind,
        buttons: FlutterPointerMouseButtons,
    ) {
        if !self.is_platform_thread() {
            panic!("Not on platform thread");
        }

        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        let buttons: flutter_engine_sys::FlutterPointerMouseButtons = buttons.into();
        let event = flutter_engine_sys::FlutterPointerEvent {
            struct_size: mem::size_of::<flutter_engine_sys::FlutterPointerEvent>(),
            timestamp: timestamp.as_micros() as usize,
            phase: phase.into(),
            x,
            y,
            device,
            signal_kind: signal_kind.into(),
            scroll_delta_x,
            scroll_delta_y,
            device_kind: device_kind.into(),
            buttons: buttons as i64,
            pan_x: 0.0,
            pan_y: 0.0,
            scale: 1.0,
            rotation: 0.0,
            #[cfg(all(target_arch = "arm", target_os = "android"))]
            __bindgen_padding_0: 0,
            #[cfg(all(target_arch = "arm", target_os = "android"))]
            __bindgen_padding_1: 0,
        };
        unsafe {
            flutter_engine_sys::FlutterEngineSendPointerEvent(self.engine_ptr(), &event, 1);
        }
    }

    pub(crate) fn send_platform_message(&self, message: PlatformMessage) {
        trace!("Sending message on channel {}", message.channel);
        if !self.is_platform_thread() {
            panic!("Not on platform thread");
        }

        unsafe {
            flutter_engine_sys::FlutterEngineSendPlatformMessage(
                self.engine_ptr(),
                &message.into(),
            );
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
            flutter_engine_sys::FlutterEngineSendPlatformMessageResponse(
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
            flutter_engine_sys::FlutterEngineShutdown(self.engine_ptr());
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
            flutter_engine_sys::FlutterEngineRunTask(self.engine_ptr(), task as *const FlutterTask);
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
            flutter_engine_sys::FlutterEnginePostRenderThreadTask(
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

#[derive(Debug, Eq, PartialEq)]
pub enum CreateError {
    NoHandler,
    EnginePtrNull,
}

impl core::fmt::Display for CreateError {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        let msg = match self {
            CreateError::NoHandler => "No handler set.",
            CreateError::EnginePtrNull => "Engine ptr is null.",
        };
        writeln!(f, "{}", msg)
    }
}

impl std::error::Error for CreateError {}

#[derive(Error, Debug)]
pub enum RunError {
    #[error("Invalid library version")]
    InvalidLibraryVersion,

    #[error("Invalid arguments")]
    InvalidArguments,

    #[error("Internal inconsistency")]
    InternalInconsistency,
}
