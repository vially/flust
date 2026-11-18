use std::{
    collections::HashMap,
    sync::{Arc, Mutex, RwLock},
    time::Instant,
};

use dpi::{LogicalSize, PhysicalSize, Size};
use flust_engine::{
    compositor::CompositorPresentError,
    ffi::{FlutterPointerEvent, FlutterViewId, IMPLICIT_VIEW_ID},
    view::FlutterView,
    FlutterEngineWeakRef,
};
use flust_engine_sys::{FlutterEngineDisplayId, FlutterEngineGetCurrentTime};
use flust_glutin::{builder::FlutterEGLContext, context::Context};
use flust_runner_api::{ApplicationAttributes, BackendConfigSctk};
use smithay_client_toolkit::{
    compositor::{CompositorState, SurfaceData},
    reexports::protocols::xdg::shell::client::xdg_toplevel::XdgToplevel,
    seat::pointer::{PointerEvent, PointerEventKind},
    shell::xdg::XdgShell,
};
use thiserror::Error;
use tracing::{error, trace};
use wayland_backend::client::ObjectId;
use wayland_client::{
    protocol::{wl_pointer::WlPointer, wl_surface::WlSurface},
    Proxy, QueueHandle,
};

use crate::{
    application::SctkApplicationState,
    egl::CreateWaylandContextError,
    frame_clock::FrameClock,
    handler::{SctkCompositorHandler, SctkOpenGLHandler},
    pointer::SctkPointerEvent,
    sync::{FrameAction, PlatformAction, SurfaceGeometry, SurfaceSync},
    toplevel::{Toplevel, ToplevelConfigure},
};
use crate::{
    egl::{FlutterEGLContextWaylandExt, NonZeroU32PhysicalSize},
    pointer::Pointer,
};

pub(crate) struct SctkFlutterWindowInner {
    id: FlutterViewId,
    window: Toplevel,
    engine: FlutterEngineWeakRef,
    qh: QueueHandle<SctkApplicationState>,
    current_size: RwLock<Option<Size>>,
    current_scale_factor: RwLock<f64>,
    default_size: Size,
    pointers: RwLock<HashMap<ObjectId, Pointer>>,
    opengl_handler: SctkOpenGLHandler,
    compositor_handler: SctkCompositorHandler,
    context: Arc<Mutex<Context>>,
    frame_clock: FrameClock,
    /// What the surface shows and what it was asked to show. The raster
    /// thread holds this lock for the whole of a present, so that a frame,
    /// the surface state it was drawn for and the acknowledgement of the
    /// configure it answers reach the compositor in one commit.
    sync: Mutex<SurfaceSync>,
}

impl SctkFlutterWindowInner {
    fn store_current_scale_factor(&self, new_scale_factor: f64) {
        let mut current_scale_factor = self.current_scale_factor.write().unwrap();
        *current_scale_factor = new_scale_factor;
    }

    fn load_current_scale_factor(&self) -> f64 {
        *self.current_scale_factor.read().unwrap()
    }

    fn store_current_size(&self, new_size: Size) {
        let mut current_size = self.current_size.write().unwrap();
        *current_size = Some(new_size);
    }

    fn load_current_size(&self) -> Option<Size> {
        *self.current_size.read().unwrap()
    }

    /// The geometry of a buffer for the window's current size and scale.
    fn current_geometry(&self) -> Option<SurfaceGeometry> {
        let scale_factor = self.load_current_scale_factor();
        let size = self.load_current_size()?.to_physical::<u32>(scale_factor);
        size.non_zero()?;

        Some(SurfaceGeometry {
            size,
            scale: scale_factor as u32,
        })
    }

    /// Presents a frame of the engine, or drops it if it is not one the
    /// surface can show: see `SurfaceSync::frame`.
    ///
    /// `size` is the size of the frame, `None` for a frame without content.
    /// `draw` puts the frame into the window's framebuffer, which has that
    /// size by then.
    ///
    /// Note: This is executed on the *render* thread.
    pub(super) fn present_frame(
        &self,
        size: Option<PhysicalSize<u32>>,
        draw: impl FnOnce() -> Result<(), CompositorPresentError>,
    ) -> Result<(), CompositorPresentError> {
        let present_failed = |reason: &str| CompositorPresentError::PresentFailed(reason.into());

        let mut sync = self.sync.lock().unwrap();
        let action = sync.frame(size);
        trace!("present frame {:?}: {:?}", size, action);

        if action == FrameAction::Drop {
            // Not an error for the engine: there is nothing it could do
            // about it, and the frame that was asked for is on its way.
            return Ok(());
        }

        let mut context = self.context.lock().unwrap();

        if let FrameAction::Commit { geometry, .. } = action {
            // The window's framebuffer gets the size of the frame. Nothing
            // has been drawn into it since the last swap, so this takes
            // effect for the frame that is about to be drawn.
            //
            // Mesa picks the buffer for a frame, at the size the window has
            // by then, when the frame is first drawn to or when the context
            // is made current while it was not: so this comes first, and
            // the context is left current after a present.
            if let Some(size) = geometry.size.non_zero() {
                context.resize(size);
            }
        }

        if !context.make_current() {
            return Err(present_failed("Unable to make context current"));
        }

        let surface = self.window.wl_surface();

        // Everything below is pending surface state until `eglSwapBuffers`
        // commits it, together with the buffer.
        if let FrameAction::Commit {
            geometry,
            acknowledge,
        } = action
        {
            surface.set_buffer_scale(geometry.scale as i32);

            if let Some(serial) = acknowledge {
                self.window.xdg_surface().ack_configure(serial);
            }
        }

        draw()?;

        // The engine's next vsync: see `FrameClock`.
        surface.frame(&self.qh, surface.clone());
        self.frame_clock
            .frame_callback_requested(unsafe { FlutterEngineGetCurrentTime() });

        if !context.present() {
            return Err(present_failed("Present failed"));
        }

        drop(context);
        drop(sync);

        if matches!(action, FrameAction::Commit { .. }) {
            // The platform thread may have a newer size to ask for.
            self.frame_clock.wake_platform_thread();
        }

        Ok(())
    }

    /// Note: Must be called on the *platform* thread.
    fn send_window_metrics(&self, geometry: SurfaceGeometry) {
        let Some(engine) = self.engine.upgrade() else {
            return;
        };

        trace!(
            "sending window metrics event: {}x{} (scale: {})",
            geometry.size.width,
            geometry.size.height,
            geometry.scale
        );

        engine.send_window_metrics_event(
            self.id,
            usize::try_from(geometry.size.width).unwrap(),
            usize::try_from(geometry.size.height).unwrap(),
            geometry.scale.into(),
            self.get_display_id().unwrap_or_default(),
        );
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
    pub(crate) fn new(
        engine: FlutterEngineWeakRef,
        qh: &QueueHandle<SctkApplicationState>,
        compositor_state: &CompositorState,
        xdg_shell_state: &XdgShell,
        frame_clock: FrameClock,
        attributes: ApplicationAttributes,
        config: BackendConfigSctk,
    ) -> Result<Self, SctkFlutterWindowCreateError> {
        let surface = compositor_state.create_surface(qh);
        let window = Toplevel::new(xdg_shell_state, surface, qh);

        if let Some(title) = attributes.title {
            window.xdg_toplevel().set_title(title);
        }

        if let Some(app_id) = attributes.app_id {
            window.xdg_toplevel().set_app_id(app_id);
        }

        window.xdg_toplevel().set_min_size(256, 256);
        // The initial commit, without a buffer: the compositor answers it
        // with the first configure.
        window.wl_surface().commit();

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
            qh: qh.clone(),
            opengl_handler: SctkOpenGLHandler::new(
                inner.clone(),
                context.clone(),
                resource_context,
            ),
            compositor_handler: SctkCompositorHandler::new(
                inner.clone(),
                context.clone(),
                config.opengl_target_type,
            ),
            context,
            frame_clock,
            sync: Default::default(),
            pointers: Default::default(),
            current_size: Default::default(),
            current_scale_factor: RwLock::new(1.0),
            default_size,
        });

        Ok(Self { inner })
    }

    pub fn xdg_toplevel_id(&self) -> ObjectId {
        self.inner.window.xdg_toplevel().id()
    }

    pub(crate) fn xdg_surface_id(&self) -> ObjectId {
        self.inner.window.xdg_surface().id()
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

    /// Does what the surface's state asks of the platform thread: sends the
    /// engine a new size, or acknowledges a configure that needs no new
    /// frame. Returns when to call this again, if nothing else happens
    /// before then.
    ///
    /// A new size is only sent if `may_request` is set: see
    /// `SurfaceSync::poll`.
    ///
    /// Must only be called while the engine is running.
    pub(crate) fn poll(&self, now: Instant, may_request: bool) -> Option<Instant> {
        loop {
            let mut sync = self.inner.sync.lock().unwrap();

            match sync.poll(now, may_request) {
                (PlatformAction::None, deadline) => return deadline,
                (PlatformAction::Request(geometry), _) => {
                    drop(sync);
                    self.inner.send_window_metrics(geometry);
                }
                (PlatformAction::Acknowledge(serial), _) => {
                    // The surface already shows what this configure asked
                    // for (only the window's state changed, or its size
                    // changed back), so no frame will come to acknowledge
                    // it with. The raster thread is kept from committing in
                    // between by the lock.
                    trace!("acknowledging configure {} without a new frame", serial);
                    self.inner.window.xdg_surface().ack_configure(serial);
                    self.inner.window.wl_surface().commit();
                }
            }
        }
    }

    pub(crate) fn scale_factor_changed(&mut self, new_scale_factor: i32) {
        // Keep the size the same in surface coordinates.
        let scale_factor = self.inner.load_current_scale_factor();
        if let Some(size) = self.inner.load_current_size() {
            self.inner
                .store_current_size(size.to_logical::<f64>(scale_factor).into());
        }
        self.inner
            .store_current_scale_factor(new_scale_factor.into());

        // The buffer scale is set by the raster thread, in the commit of the
        // first frame that was drawn for it.
        if let Some(geometry) = self.inner.current_geometry() {
            self.inner.sync.lock().unwrap().set_geometry(geometry);
        }
    }

    pub(crate) fn configure(&mut self, configure: ToplevelConfigure, serial: u32) {
        // A configure without a size leaves it to the window.
        let new_logical_size = configure
            .size
            .map(|(width, height)| LogicalSize::new(width.get(), height.get()).into())
            .or(self.inner.load_current_size())
            .unwrap_or(self.inner.default_size);

        self.inner.store_current_size(new_logical_size);

        let Some(geometry) = self.inner.current_geometry() else {
            error!("Unable to convert window configure event to a physical size");
            return;
        };

        // Neither acknowledged nor sent to the engine here: see `poll`.
        self.inner.sync.lock().unwrap().configure(serial, geometry);
    }

    pub(crate) fn surface_outputs_changed(&mut self) {
        // Only the display changed: tell the engine, for the size it is
        // drawing at.
        let geometry = self.inner.sync.lock().unwrap().engine_geometry();
        if let Some(geometry) = geometry {
            self.inner.send_window_metrics(geometry);
        }
    }

    pub(crate) fn pointer_event(&mut self, pointer: &WlPointer, event: &PointerEvent) {
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
