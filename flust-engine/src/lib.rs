pub mod builder;
pub mod channel;
pub mod codec;
pub mod compositor;
pub mod error;
pub mod ffi;
mod flutter_callbacks;
pub mod plugins;
pub mod renderer;
pub mod semantics;
pub mod tasks;
pub mod view;

pub mod texture_registry;

use crate::builder::FlutterEngineBuilder;
use crate::channel::{Channel, ChannelRegistry};

use crate::channel::platform_message::{PlatformMessage, PlatformMessageResponseHandle};
use crate::renderer::Renderer;
use crate::tasks::TaskRunner;
use crate::texture_registry::{Texture, TextureRegistry};
use compositor::FlutterCompositorHandler;
use crossbeam_channel::{unbounded, Receiver, Sender};
use ffi::{
    FlutterEngineAOTData, FlutterEngineDisplay, FlutterEngineDisplaysUpdateType,
    FlutterEngineResult, FlutterEngineResultExt, FlutterKeyEvent, FlutterPointerEvent,
    FlutterViewId, FlutterWindowMetrics, IMPLICIT_VIEW_ID,
};
use flust_engine_api::FlutterOpenGLHandler;
use flust_engine_sys::{
    FlutterCompositor, FlutterEngineDisplayId, FlutterEngineGetCurrentTime,
    FlutterEngineRunsAOTCompiledDartCode, FlutterTask, VsyncCallback,
};
use parking_lot::{Mutex, RwLock};
use semantics::{FlutterAccessibilityFeatures, FlutterSemanticsAction, FlutterSemanticsHandler};
use std::collections::HashMap;
use std::ffi::{c_void, CString};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, LazyLock, Weak};
use std::time::{Duration, Instant};
use thiserror::Error;
use tracing::{info, trace};
use view::{
    FlutterView, FlutterViewFocusDirection, FlutterViewFocusHandler, FlutterViewFocusState,
    ViewRegistry,
};

pub(crate) type MainThreadEngineFn = Box<dyn FnOnce(&FlutterEngine) + Send>;
pub(crate) type MainThreadRenderThreadFn = Box<dyn FnOnce(&FlutterEngine) + Send>;

pub(crate) enum MainThreadCallback {
    Engine(MainThreadEngineFn),
    RenderThread(MainThreadRenderThreadFn),
}

/// The engines that exist, by their id: see [`FlutterEngine::engine_id`].
static ENGINES: LazyLock<Mutex<HashMap<i64, FlutterEngineWeakRef>>> =
    LazyLock::new(Default::default);

/// The id of the next engine. 0 is not used: the engine takes it to mean
/// that there is no id, and Dart then gets none.
static NEXT_ENGINE_ID: AtomicI64 = AtomicI64::new(1);

struct FlutterEngineInner {
    engine_id: i64,
    view_registry: RwLock<ViewRegistry>,
    /// The id of the next view: see [`FlutterEngine::allocate_view_id`].
    next_view_id: AtomicI64,
    vsync_handler: Option<Arc<Mutex<dyn FlutterVsyncHandler + Send>>>,
    semantics_handler: Option<Arc<dyn FlutterSemanticsHandler + Send + Sync>>,
    view_focus_handler: Option<Arc<dyn FlutterViewFocusHandler + Send + Sync>>,
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
    renderer: Renderer,
}

impl Drop for FlutterEngineInner {
    fn drop(&mut self) {
        ENGINES.lock().remove(&self.engine_id);
    }
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

/// What the engine hands back to the callback of an add or remove view
/// call: the view, and what to tell about the result.
pub(crate) struct ViewBaton {
    pub(crate) engine: FlutterEngineWeakRef,
    pub(crate) view_id: FlutterViewId,
    pub(crate) callback: Box<dyn FnOnce(bool) + Send>,
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
        // The renderer is passed explicitly, after everything else, so that
        // the engine does not fall back to a default of its own.
        let renderer = Renderer::resolve(builder.renderer, &builder.args);
        info!("rendering with {renderer:?}");
        let args = [
            dummy_args,
            FlutterEngine::args_from_env_vars(),
            builder.args.clone(),
            vec![renderer.engine_switch()],
        ]
        .concat();

        let mut args: Vec<_> = args
            .iter()
            .map(|arg| CString::new(arg.as_str()).unwrap().into_raw())
            .collect();

        let (main_tx, main_rx) = unbounded();

        let engine_id = NEXT_ENGINE_ID.fetch_add(1, Ordering::Relaxed);
        let engine = Self {
            #[allow(clippy::arc_with_non_send_sync)]
            inner: Arc::new(FlutterEngineInner {
                engine_id,
                view_registry: RwLock::new(ViewRegistry::default()),
                next_view_id: AtomicI64::new(IMPLICIT_VIEW_ID + 1),
                vsync_handler: builder.vsync_handler,
                semantics_handler: builder.semantics_handler,
                view_focus_handler: builder.view_focus_handler,
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
                renderer,
            }),
        };

        let inner = &engine.inner;
        ENGINES.lock().insert(engine_id, engine.downgrade());
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
            destruction_callback: None,
        };
        // The engine merges the UI thread into the platform thread when both
        // are given task runners with the same identifier.
        let ui_task_runner = match builder.merged_platform_ui_thread {
            true => &platform_task_runner as *const flust_engine_sys::FlutterTaskRunnerDescription,
            false => std::ptr::null(),
        };
        let custom_task_runners = flust_engine_sys::FlutterCustomTaskRunners {
            struct_size: std::mem::size_of::<flust_engine_sys::FlutterCustomTaskRunners>(),
            platform_task_runner: &platform_task_runner
                as *const flust_engine_sys::FlutterTaskRunnerDescription,
            render_task_runner: std::ptr::null(),
            thread_priority_setter: None,
            ui_task_runner,
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

        let update_semantics_callback2: flust_engine_sys::FlutterUpdateSemanticsCallback2 =
            match inner.semantics_handler {
                Some(_) => Some(flutter_callbacks::update_semantics_callback),
                None => None,
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
            update_semantics_callback2,
            channel_update_callback: None,
            view_focus_change_request_callback: Some(
                flutter_callbacks::view_focus_change_request_callback,
            ),
            engine_id: inner.engine_id,
            enable_wide_gamut: false,
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

    /// The id of this engine, which Dart sees as `PlatformDispatcher.engineId`.
    /// Native code that Dart calls can find the engine by it, with
    /// [`Self::from_engine_id`]. Never 0.
    pub fn engine_id(&self) -> i64 {
        self.inner.engine_id
    }

    /// The engine with this id, if it still exists: see [`Self::engine_id`].
    pub fn from_engine_id(engine_id: i64) -> Option<FlutterEngine> {
        ENGINES.lock().get(&engine_id)?.upgrade()
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

    /// The renderer the engine draws with.
    pub fn renderer(&self) -> Renderer {
        self.inner.renderer
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

    /// Gives the engine's callbacks for the implicit view somewhere to go.
    /// The engine always has the implicit view: it is not added or removed
    /// like the others.
    pub fn add_implicit_view(&self, view: FlutterView) {
        assert_eq!(view.id(), IMPLICIT_VIEW_ID, "not the implicit view");
        if self.inner.view_registry.write().add_view(view).is_err() {
            panic!("The implicit view was added twice");
        }
    }

    /// An id for a view that is then added with [`Self::add_view`]. Ids are
    /// not used twice, and are never the implicit view's.
    pub fn allocate_view_id(&self) -> FlutterViewId {
        self.inner.next_view_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Adds a view to the running engine, with its first metrics.
    ///
    /// The view is not to be used (sent metrics or input) until `on_added`
    /// is called with `true`. `on_added` is called on the platform thread,
    /// with whether the engine added the view, if this returns `Ok`. The
    /// engine's callbacks for the view go to its handlers from now on, and
    /// until it is removed again.
    pub fn add_view(
        &self,
        view: FlutterView,
        metrics: FlutterWindowMetrics,
        on_added: impl FnOnce(bool) + Send + 'static,
    ) -> Result<(), ViewError> {
        if !self.is_platform_thread() {
            panic!("Not on platform thread");
        }

        let view_id = view.id();
        if view_id == IMPLICIT_VIEW_ID {
            return Err(ViewError::ImplicitView);
        }
        if self.inner.view_registry.write().add_view(view).is_err() {
            return Err(ViewError::AlreadyExists(view_id));
        }

        let baton = Box::into_raw(Box::new(ViewBaton {
            engine: self.downgrade(),
            view_id,
            callback: Box::new(on_added),
        }));
        let metrics = metrics.to_ffi(view_id);
        let info = flust_engine_sys::FlutterAddViewInfo {
            struct_size: std::mem::size_of::<flust_engine_sys::FlutterAddViewInfo>(),
            view_id,
            view_metrics: &metrics,
            user_data: baton as *mut c_void,
            add_view_callback: Some(flutter_callbacks::add_view_callback),
        };

        let result = unsafe { flust_engine_sys::FlutterEngineAddView(self.engine_ptr(), &info) };
        if let Err(err) = FlutterEngineResult::from_ffi(result) {
            // The callback is not called: the baton is still this call's.
            drop(unsafe { Box::from_raw(baton) });
            let view = self.inner.view_registry.write().remove_view(view_id);
            drop(view);
            return Err(err.into());
        }
        Ok(())
    }

    /// Removes a view from the running engine.
    ///
    /// The view's handlers keep getting the engine's callbacks until the
    /// engine is done with the view: what they draw to has to outlive them.
    /// `on_removed` is called on the platform thread after that, with
    /// whether the engine removed the view, if this returns `Ok`. The view
    /// is dropped right before.
    pub fn remove_view(
        &self,
        view_id: FlutterViewId,
        on_removed: impl FnOnce(bool) + Send + 'static,
    ) -> Result<(), ViewError> {
        if !self.is_platform_thread() {
            panic!("Not on platform thread");
        }

        if view_id == IMPLICIT_VIEW_ID {
            return Err(ViewError::ImplicitView);
        }
        if !self.inner.view_registry.read().contains(view_id) {
            return Err(ViewError::UnknownView(view_id));
        }

        let baton = Box::into_raw(Box::new(ViewBaton {
            engine: self.downgrade(),
            view_id,
            callback: Box::new(on_removed),
        }));
        let info = flust_engine_sys::FlutterRemoveViewInfo {
            struct_size: std::mem::size_of::<flust_engine_sys::FlutterRemoveViewInfo>(),
            view_id,
            user_data: baton as *mut c_void,
            remove_view_callback: Some(flutter_callbacks::remove_view_callback),
        };

        let result = unsafe { flust_engine_sys::FlutterEngineRemoveView(self.engine_ptr(), &info) };
        if let Err(err) = FlutterEngineResult::from_ffi(result) {
            // The callback is not called: the baton is still this call's.
            drop(unsafe { Box::from_raw(baton) });
            return Err(err.into());
        }
        Ok(())
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
        self.send_window_metrics(
            view_id,
            FlutterWindowMetrics {
                width,
                height,
                pixel_ratio,
                display_id,
                constraints: None,
            },
        );
    }

    /// Tells the engine the size of a view, and the display it is on.
    pub fn send_window_metrics(&self, view_id: FlutterViewId, metrics: FlutterWindowMetrics) {
        trace!("send_window_metrics: view {view_id}: {metrics:?}");
        if !self.is_platform_thread() {
            panic!("Not on platform thread");
        }

        let event = metrics.to_ffi(view_id);
        unsafe {
            flust_engine_sys::FlutterEngineSendWindowMetricsEvent(self.engine_ptr(), &event);
        }
    }

    /// Tells the engine that a view got or lost the platform's focus. Keys
    /// go to the view that has it: key events carry no view.
    pub fn send_view_focus_event(
        &self,
        view_id: FlutterViewId,
        state: FlutterViewFocusState,
        direction: FlutterViewFocusDirection,
    ) -> Result<(), FlutterEngineError> {
        trace!("send_view_focus_event: view {view_id}: {state:?} {direction:?}");
        if !self.is_platform_thread() {
            panic!("Not on platform thread");
        }

        let event = flust_engine_sys::FlutterViewFocusEvent {
            struct_size: std::mem::size_of::<flust_engine_sys::FlutterViewFocusEvent>(),
            view_id,
            state: state.into(),
            direction: direction.into(),
        };
        let result =
            unsafe { flust_engine_sys::FlutterEngineSendViewFocusEvent(self.engine_ptr(), &event) };
        FlutterEngineResult::from_ffi(result)
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

    /// Turns the semantics tree of the views on or off.
    ///
    /// While it is on, the [`FlutterSemanticsHandler`] of the engine
    /// receives the tree and the changes to it, starting with all of it.
    /// Keeping it up to date costs the framework time on every frame, so it
    /// should only be on while something reads it.
    pub fn update_semantics_enabled(&self, enabled: bool) -> Result<(), FlutterEngineError> {
        trace!("update_semantics_enabled: {enabled}");
        if !self.is_platform_thread() {
            panic!("Not on platform thread");
        }

        let result = unsafe {
            flust_engine_sys::FlutterEngineUpdateSemanticsEnabled(self.engine_ptr(), enabled)
        };
        FlutterEngineResult::from_ffi(result)
    }

    /// Tells the framework which accessibility settings of the platform are
    /// on. This replaces the ones set before.
    pub fn update_accessibility_features(
        &self,
        features: FlutterAccessibilityFeatures,
    ) -> Result<(), FlutterEngineError> {
        trace!("update_accessibility_features: {features:?}");
        if !self.is_platform_thread() {
            panic!("Not on platform thread");
        }

        let result = unsafe {
            flust_engine_sys::FlutterEngineUpdateAccessibilityFeatures(
                self.engine_ptr(),
                features.into(),
            )
        };
        FlutterEngineResult::from_ffi(result)
    }

    /// Asks a semantics node to perform one of its actions.
    ///
    /// `data` are the arguments of the action, encoded with the standard
    /// message codec. It is empty for the actions that take none.
    pub fn send_semantics_action(
        &self,
        view_id: FlutterViewId,
        node_id: i32,
        action: FlutterSemanticsAction,
        data: &[u8],
    ) -> Result<(), FlutterEngineError> {
        trace!("send_semantics_action: {action:?} to node {node_id} of view {view_id}");
        if !self.is_platform_thread() {
            panic!("Not on platform thread");
        }

        let node_id = u64::try_from(node_id).map_err(|_| FlutterEngineError::InvalidArguments)?;

        let info = flust_engine_sys::FlutterSendSemanticsActionInfo {
            struct_size: std::mem::size_of::<flust_engine_sys::FlutterSendSemanticsActionInfo>(),
            view_id,
            node_id,
            action: action.into(),
            data: if data.is_empty() {
                ptr::null()
            } else {
                data.as_ptr()
            },
            data_length: data.len(),
        };

        let result =
            unsafe { flust_engine_sys::FlutterEngineSendSemanticsAction(self.engine_ptr(), &info) };
        FlutterEngineResult::from_ffi(result)
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
pub enum ViewError {
    #[error("The implicit view can be neither added nor removed")]
    ImplicitView,

    #[error("A view with id {0} exists already")]
    AlreadyExists(FlutterViewId),

    #[error("There is no view with id {0}")]
    UnknownView(FlutterViewId),

    #[error(transparent)]
    Engine(#[from] FlutterEngineError),
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
