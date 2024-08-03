use std::{
    collections::HashMap,
    num::NonZeroU32,
    sync::{Arc, Mutex, RwLock},
};

use dpi::{LogicalSize, PhysicalSize, Size};
use flust_engine::{
    ffi::{FlutterPointerEvent, FlutterViewId, IMPLICIT_VIEW_ID},
    view::FlutterView,
    FlutterEngineWeakRef,
};
use flust_engine_sys::FlutterEngineDisplayId;
use flust_glutin::builder::FlutterEGLContext;
use flust_runner_api::ApplicationAttributes;
use smithay_client_toolkit::{
    compositor::{CompositorState, SurfaceData},
    reexports::protocols::xdg::shell::client::xdg_toplevel::XdgToplevel,
    seat::pointer::{PointerEvent, PointerEventKind},
    shell::{
        xdg::{
            window::{Window, WindowConfigure, WindowDecorations},
            XdgShell,
        },
        WaylandSurface,
    },
};
use thiserror::Error;
use tracing::{error, trace, warn};
use wayland_backend::client::ObjectId;
use wayland_client::{
    protocol::{wl_pointer::WlPointer, wl_surface::WlSurface},
    Connection, Proxy, QueueHandle,
};

use crate::{
    application::SctkApplicationState,
    egl::CreateWaylandContextError,
    handler::{SctkCompositorHandler, SctkOpenGLHandler, SctkVsyncHandler},
    pointer::SctkPointerEvent,
};
use crate::{
    egl::{FlutterEGLContextWaylandExt, NonZeroU32PhysicalSize},
    pointer::Pointer,
};

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
/// States a resize event can be in.
pub(crate) enum ResizeState {
    /// Default state for when no resize is in progress. Also used to indicate
    /// that during a resize event, a frame with the right size has been
    /// rendered and the buffers have been swapped.
    #[default]
    Done,
    /// When a resize event has started but is in progress.
    ResizeStarted,
    /// After a resize event starts and the framework has been notified to
    /// generate a frame for the right size.
    FrameGenerated,
}

pub(crate) struct SctkFlutterWindowInner {
    id: FlutterViewId,
    window: Window,
    engine: FlutterEngineWeakRef,
    current_size: RwLock<Option<Size>>,
    current_scale_factor: RwLock<f64>,
    default_size: Size,
    pointers: RwLock<HashMap<ObjectId, Pointer>>,
    opengl_handler: SctkOpenGLHandler,
    compositor_handler: SctkCompositorHandler,
    vsync_handler: Arc<parking_lot::Mutex<SctkVsyncHandler>>,
    resize_mutex: Mutex<()>,
    resize_status: RwLock<ResizeState>,
    pending_size: RwLock<Option<PhysicalSize<NonZeroU32>>>,
}

impl SctkFlutterWindowInner {
    pub(super) fn store_current_scale_factor(&self, new_scale_factor: f64) {
        let mut current_scale_factor = self.current_scale_factor.write().unwrap();
        *current_scale_factor = new_scale_factor;
    }

    pub(super) fn load_current_scale_factor(&self) -> f64 {
        *self.current_scale_factor.read().unwrap()
    }

    pub(super) fn store_current_size(&self, new_size: Size) {
        let mut current_size = self.current_size.write().unwrap();
        *current_size = Some(new_size);
    }

    fn store_resize_status(&self, new_resize_status: ResizeState) {
        let mut resize_status = self.resize_status.write().unwrap();
        *resize_status = new_resize_status;
    }

    pub(super) fn load_resize_status(&self) -> ResizeState {
        *self.resize_status.read().unwrap()
    }

    pub(super) fn store_pending_size(&self, new_pending_size: Option<PhysicalSize<NonZeroU32>>) {
        let mut pending_size = self.pending_size.write().unwrap();
        *pending_size = new_pending_size;
    }

    pub(super) fn load_pending_size(&self) -> Option<PhysicalSize<NonZeroU32>> {
        *self.pending_size.read().unwrap()
    }

    pub(super) fn scale_internal_size(&self, new_scale_factor: f64) {
        self.store_current_scale_factor(new_scale_factor);

        let mut current_size = self.current_size.write().unwrap();
        *current_size = current_size.map(|size| size.to_logical::<u32>(new_scale_factor).into());
    }

    pub(super) fn non_zero_physical_size(&self) -> Option<PhysicalSize<NonZeroU32>> {
        let scale_factor = self.current_scale_factor.read().unwrap();
        self.current_size
            .read()
            .unwrap()
            .and_then(|size| size.to_physical::<u32>(*scale_factor).non_zero())
    }

    // Note: This callback is executed on the *render* thread.
    pub(super) fn on_frame_generated(&self, size: PhysicalSize<u32>) -> bool {
        trace!("window frame generated: {}x{}", size.width, size.height);
        let _resize_mutex = self.resize_mutex.lock().unwrap();

        let resize_status = self.load_resize_status();
        if resize_status != ResizeState::ResizeStarted {
            return true;
        }

        let Some(pending_size) = self.load_pending_size() else {
            error!("[on_frame_generated] Invalid resize state: pending size not found");
            return false;
        };

        if size.width != pending_size.width.get() || size.height != pending_size.height.get() {
            trace!(
                "[on_frame_generated]: Frame size does not match expected size: {}x{} != {}x{}",
                size.width,
                size.height,
                pending_size.width,
                pending_size.height
            );
            return false;
        }

        self.store_resize_status(ResizeState::FrameGenerated);
        true
    }

    // Note: This callback is executed on the *render* thread.
    pub(super) fn on_empty_frame_generated(&self) -> bool {
        trace!("window empty frame generated");
        let _resize_mutex = self.resize_mutex.lock().unwrap();

        let resize_status = self.load_resize_status();
        if resize_status != ResizeState::ResizeStarted {
            return true;
        }

        self.store_resize_status(ResizeState::FrameGenerated);
        true
    }

    // Note: This callback is executed on the *render* thread.
    pub(super) fn on_frame_presented(&self) {
        trace!("window frame presented");
        let _resize_mutex = self.resize_mutex.lock().unwrap();

        self.vsync_handler.lock().notify_present();

        let resize_status = self.load_resize_status();
        match resize_status {
            ResizeState::ResizeStarted => {
                // The caller must first call `on_frame_generated` before
                // calling this method. This indicates one of the following:
                //
                // 1. The caller did not call this method.
                // 2. The caller ignored this method's result.
                // 3. The platform thread started a resize after the caller
                //    called these methods. We might have presented a frame of
                //    the wrong size to the view.
                warn!("A frame of the wrong size might have been presented after a resize was started");
            }
            ResizeState::FrameGenerated => {
                // A frame was generated for a pending resize. Mark the resize as done.
                self.store_resize_status(ResizeState::Done);
            }
            ResizeState::Done => {}
        }
    }

    /// A surface can be present on multiple outputs, but currently Flutter only
    /// supports passing a single `display_id` as part of the window metrics
    /// event. Therefore, the current implementation just picks the id of the
    /// first output.
    fn get_display_id(&self) -> Option<FlutterEngineDisplayId> {
        let data = self.window.wl_surface().data::<SurfaceData>()?;
        let display_id = data.outputs().next()?.id().protocol_id();
        Some(display_id.into())
    }
}

pub struct SctkFlutterWindow {
    inner: Arc<SctkFlutterWindowInner>,
}

impl SctkFlutterWindow {
    pub fn new(
        engine: FlutterEngineWeakRef,
        qh: &QueueHandle<SctkApplicationState>,
        compositor_state: &CompositorState,
        xdg_shell_state: &XdgShell,
        vsync_handler: Arc<parking_lot::Mutex<SctkVsyncHandler>>,
        attributes: ApplicationAttributes,
    ) -> Result<Self, SctkFlutterWindowCreateError> {
        let surface = compositor_state.create_surface(qh);
        let window = xdg_shell_state.create_window(surface, WindowDecorations::ServerDefault, qh);

        if let Some(title) = attributes.title {
            window.set_title(title);
        }

        if let Some(app_id) = attributes.app_id {
            window.set_app_id(app_id);
        }

        window.set_min_size(Some((256, 256)));
        window.commit();

        let default_size = attributes
            .inner_size
            .unwrap_or(Size::Logical(LogicalSize::<f64>::new(1280.0, 720.0)));

        let (context, resource_context) = FlutterEGLContext::new_wayland_context(
            window.wl_surface(),
            default_size.to_physical::<u32>(1.0),
        )?;

        let context = Arc::new(Mutex::new(context));
        let resource_context = Arc::new(Mutex::new(resource_context));

        let inner = Arc::new_cyclic(|inner| SctkFlutterWindowInner {
            id: IMPLICIT_VIEW_ID,
            window,
            engine,
            opengl_handler: SctkOpenGLHandler::new(
                inner.clone(),
                context.clone(),
                resource_context,
            ),
            compositor_handler: SctkCompositorHandler::new(inner.clone(), context),
            vsync_handler,
            resize_mutex: Default::default(),
            resize_status: Default::default(),
            pointers: Default::default(),
            current_size: Default::default(),
            current_scale_factor: RwLock::new(1.0),
            pending_size: Default::default(),
            default_size,
        });

        Ok(Self { inner })
    }

    pub fn xdg_toplevel_id(&self) -> ObjectId {
        self.inner.window.xdg_toplevel().id()
    }

    pub fn wl_surface(&self) -> WlSurface {
        self.inner.window.wl_surface().clone()
    }

    pub fn wl_surface_id(&self) -> ObjectId {
        self.inner.window.wl_surface().id()
    }

    pub fn xdg_toplevel(&self) -> XdgToplevel {
        self.inner.window.xdg_toplevel().clone()
    }

    pub(crate) fn create_flutter_view(&self) -> FlutterView {
        FlutterView::new_with_compositor(
            self.inner.id,
            self.inner.opengl_handler.clone(),
            self.inner.compositor_handler.clone(),
        )
    }

    pub(crate) fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        surface: &WlSurface,
        new_scale_factor: i32,
    ) {
        let _resize_mutex = self.inner.resize_mutex.lock().unwrap();

        self.inner.scale_internal_size(new_scale_factor.into());

        let Some(physical_size) = self.inner.non_zero_physical_size() else {
            error!("Invalid physical size while handling `scale_factor_changed` event");
            return;
        };

        self.inner.store_resize_status(ResizeState::ResizeStarted);
        self.inner.store_pending_size(Some(physical_size));

        // Note: Comment related to `opengl_handler.resize()` call from the
        // `SctkFlutterWindow.configure()` method also applies here.
        self.inner.opengl_handler.resize(physical_size);
        surface.set_buffer_scale(new_scale_factor);

        let display_id = self.inner.get_display_id().unwrap_or_default();

        if let Some(engine) = self.inner.engine.upgrade() {
            engine.send_window_metrics_event(
                self.inner.id,
                usize::try_from(physical_size.width.get()).unwrap(),
                usize::try_from(physical_size.height.get()).unwrap(),
                new_scale_factor as f64,
                display_id,
            );
        }
    }

    pub(crate) fn configure(
        &mut self,
        _conn: &Connection,
        configure: WindowConfigure,
        _serial: u32,
    ) {
        let _resize_mutex = self.inner.resize_mutex.lock().unwrap();

        let new_logical_size = WindowLogicalSize::try_from(configure.new_size)
            .map(|size| size.into())
            .unwrap_or(self.inner.default_size);

        self.inner.store_current_size(new_logical_size);

        let scale_factor = self.inner.load_current_scale_factor();

        let Some(physical_size) = new_logical_size.to_physical(scale_factor).non_zero() else {
            error!("Unable to convert window configure event to a physical size");
            return;
        };

        self.inner.store_resize_status(ResizeState::ResizeStarted);
        self.inner.store_pending_size(Some(physical_size));

        // The resize logic is based on Flutter's Windows embedder
        // implementation. However, one notable difference between the two is
        // that the Windows implementation resizes the EGL surface *after*
        // sending the window metrics event to the engine (e.g.: as part of the
        // `CompositorOpenGL::Present` callback [0]), while flust-sctk's
        // implementation resizes it *prior* to sending the window metrics
        // event.
        //
        // This change in flust-sctk's implementation was done to avoid some
        // visual glitches that were observed when the EGL surface was resized
        // too late (e.g.: as part of the `on_frame_generated` callback).
        //
        // TODO: Investigate when is the *correct* time to resize the EGL
        // surface and update the implementation if needed.
        //
        // [0]: https://github.com/flutter/engine/blob/605b3f3/shell/platform/windows/flutter_windows_view.cc#L701-L711
        self.inner.opengl_handler.resize(physical_size);

        let display_id = self.inner.get_display_id().unwrap_or_default();

        if let Some(engine) = self.inner.engine.upgrade() {
            engine.send_window_metrics_event(
                self.inner.id,
                usize::try_from(physical_size.width.get()).unwrap(),
                usize::try_from(physical_size.height.get()).unwrap(),
                scale_factor,
                display_id,
            );
        }
    }

    pub(crate) fn surface_outputs_changed(&mut self, _conn: &Connection, _surface: &WlSurface) {
        let scale_factor = self.inner.load_current_scale_factor();

        let Some(physical_size) = self.inner.non_zero_physical_size() else {
            error!("Invalid physical size while handling `surface_outputs_changed` event");
            return;
        };

        let display_id = self.inner.get_display_id().unwrap_or_default();

        if let Some(engine) = self.inner.engine.upgrade() {
            engine.send_window_metrics_event(
                self.inner.id,
                usize::try_from(physical_size.width.get()).unwrap(),
                usize::try_from(physical_size.height.get()).unwrap(),
                scale_factor,
                display_id,
            );
        }
    }

    pub(crate) fn pointer_event(
        &mut self,
        _conn: &Connection,
        pointer: &WlPointer,
        event: &PointerEvent,
    ) {
        let sctk_pointer_event = {
            let mut pointers = self.inner.pointers.write().unwrap();
            let pointer = pointers
                .entry(pointer.id())
                .or_insert_with(|| Pointer::new(pointer.id().protocol_id() as i32));

            match event.kind {
                PointerEventKind::Press { .. } => pointer.increment_pressed(),
                PointerEventKind::Release { .. } => pointer.decrement_pressed(),
                _ => {}
            }

            let scale_factor = self.inner.load_current_scale_factor();
            SctkPointerEvent::new(self.inner.id, event.clone(), *pointer, scale_factor)
        };

        let Ok(event) = FlutterPointerEvent::try_from(sctk_pointer_event) else {
            error!("Unable to convert wayland pointer event to flutter pointer event");
            return;
        };

        let Some(engine) = self.inner.engine.upgrade() else {
            error!("Unable to upgrade weak engine while sending pointer event");
            return;
        };

        engine.send_pointer_event(event);
    }
}

#[derive(Error, Debug)]
pub enum SctkFlutterWindowCreateError {
    #[error("Failed to create Wayland EGL context")]
    CreateWaylandContextError(#[from] CreateWaylandContextError),
}

type ConfigureSize = (Option<NonZeroU32>, Option<NonZeroU32>);

struct WindowLogicalSize(LogicalSize<u32>);

impl TryFrom<ConfigureSize> for WindowLogicalSize {
    type Error = SizeConversionError;

    fn try_from(value: ConfigureSize) -> Result<Self, Self::Error> {
        let (Some(width), Some(height)) = value else {
            return Err(SizeConversionError::Invalid);
        };

        Ok(Self(LogicalSize::new(width.get(), height.get())))
    }
}

impl From<WindowLogicalSize> for Size {
    fn from(val: WindowLogicalSize) -> Self {
        val.0.into()
    }
}

#[derive(Error, Debug)]
pub enum SizeConversionError {
    #[error("Invalid size")]
    Invalid,
}
