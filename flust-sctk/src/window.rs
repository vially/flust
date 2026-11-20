use std::{
    collections::HashMap,
    num::NonZeroU32,
    sync::{Arc, Mutex, RwLock},
    time::Instant,
};

use dpi::{LogicalSize, PhysicalSize, Size};
use flust_engine::{
    compositor::CompositorPresentError,
    ffi::{
        FlutterOpenGLTargetType, FlutterPointerEvent, FlutterViewId, FlutterWindowMetrics,
        IMPLICIT_VIEW_ID,
    },
    renderer::Renderer,
    view::FlutterView,
    FlutterEngineWeakRef,
};
use flust_engine_sys::{FlutterEngineDisplayId, FlutterEngineGetCurrentTime};
use flust_glutin::render::{RenderContext, WindowSurface};
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
    handler::{SctkCompositorHandler, SctkOpenGLCompositorHandlerFramebuffer},
    pointer::SctkPointerEvent,
    sync::{FrameAction, PlatformAction, SurfaceGeometry, SurfaceSync},
    toplevel::{Toplevel, ToplevelConfigure},
};
use crate::{
    egl::{new_wayland_window_surface, NonZeroU32PhysicalSize},
    pointer::Pointer,
};

pub(crate) struct SctkFlutterWindowInner {
    id: FlutterViewId,
    /// The window's EGL surface. Declared before `window`, so that it is
    /// dropped first: it is backed by the window's `wl_surface`.
    surface: Mutex<WindowSurface>,
    window: Toplevel,
    engine: FlutterEngineWeakRef,
    qh: QueueHandle<SctkApplicationState>,
    current_size: RwLock<Option<Size>>,
    current_scale_factor: RwLock<f64>,
    default_size: Size,
    pointers: RwLock<HashMap<ObjectId, Pointer>>,
    compositor_handler: SctkCompositorHandler,
    /// The context the engine draws every window in. Made current on the
    /// window's surface only while a frame is put into it and presented.
    render_context: Arc<Mutex<RenderContext>>,
    frame_clock: FrameClock,
    /// What the surface shows and what it was asked to show. The raster
    /// thread holds this lock for the whole of a present, so that a frame,
    /// the surface state it was drawn for and the acknowledgement of the
    /// configure it answers reach the compositor in one commit.
    sync: Mutex<SurfaceSync>,
    /// The size of the frame the engine has drawn straight into the window's
    /// framebuffer and not presented yet. Only used with
    /// `FlutterOpenGLTargetType::Surface`.
    surface_frame: Mutex<Option<PhysicalSize<u32>>>,
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

        let render_context = self.render_context.lock().unwrap();
        let mut window_surface = self.surface.lock().unwrap();

        if let FrameAction::Commit { geometry, .. } = action {
            // The window's framebuffer gets the size of the frame. Nothing
            // has been drawn into it since the last swap, so this takes
            // effect for the frame that is about to be drawn.
            //
            // Mesa picks the buffer for a frame, at the size the window has
            // by then, when the frame is first drawn to or when the surface
            // is made current while it was not: so this comes first.
            if let Some(size) = geometry.size.non_zero() {
                render_context.resize(&window_surface, size);
            }
        }

        if !render_context.make_current_with(&mut window_surface) {
            return Err(present_failed("Unable to make context current"));
        }

        let presented = self.present_current_frame(&render_context, &window_surface, action, draw);

        // The engine draws the next view's frame in the render context, as
        // it was before: current, without a surface.
        render_context.make_current();

        drop(window_surface);
        drop(render_context);
        drop(sync);
        presented?;

        if matches!(action, FrameAction::Commit { .. }) {
            // The platform thread may have a newer size to ask for.
            self.frame_clock.wake_platform_thread();
        }

        Ok(())
    }

    /// Puts the frame into the window's surface, with what the surface
    /// state needs along with it, and presents it. The render context is
    /// current on the surface.
    fn present_current_frame(
        &self,
        render_context: &RenderContext,
        window_surface: &WindowSurface,
        action: FrameAction,
        draw: impl FnOnce() -> Result<(), CompositorPresentError>,
    ) -> Result<(), CompositorPresentError> {
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

        if !render_context.swap_buffers(window_surface) {
            return Err(CompositorPresentError::PresentFailed(
                "Present failed".into(),
            ));
        }
        Ok(())
    }

    /// The engine is about to draw a frame of this size straight into the
    /// window's framebuffer (`FlutterOpenGLTargetType::Surface`). Returns
    /// false if it should not: the frame would not be shown.
    ///
    /// The framebuffer has to have the frame's size before the frame is
    /// drawn, which is here. A frame that has been drawn can not be dropped
    /// any more (it would be in the way of the next one), so whether it is
    /// shown is decided here as well, and `present_frame` follows.
    ///
    /// Note: This is executed on the *render* thread.
    pub(super) fn begin_surface_frame(&self, size: PhysicalSize<NonZeroU32>) -> bool {
        let frame_size = PhysicalSize::new(size.width.get(), size.height.get());

        let admitted = self.sync.lock().unwrap().admit(frame_size, Instant::now());
        *self.surface_frame.lock().unwrap() = admitted.then_some(frame_size);
        trace!(
            "begin surface frame {:?}: admitted: {}",
            frame_size,
            admitted
        );

        if !admitted {
            return false;
        }

        let render_context = self.render_context.lock().unwrap();
        let mut window_surface = self.surface.lock().unwrap();
        // Before the surface is made current: see `present_frame`.
        render_context.resize(&window_surface, size);
        render_context.make_current_with(&mut window_surface)
    }

    /// The size of the frame that was drawn into the window's framebuffer
    /// since the last call, if one was.
    ///
    /// Note: This is executed on the *render* thread.
    pub(super) fn take_surface_frame(&self) -> Option<PhysicalSize<u32>> {
        self.surface_frame.lock().unwrap().take()
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

/// What every window is drawn with, the same for all of them.
pub(crate) struct SctkRendering {
    pub(crate) render_context: Arc<Mutex<RenderContext>>,
    pub(crate) framebuffer: SctkOpenGLCompositorHandlerFramebuffer,
    pub(crate) opengl_target_type: FlutterOpenGLTargetType,
    pub(crate) renderer: Renderer,
}

/// What a window is created with on the Wayland side.
pub(crate) struct SctkShell {
    pub(crate) compositor_state: CompositorState,
    pub(crate) xdg_shell_state: XdgShell,
    pub(crate) frame_clock: FrameClock,
}

/// What one window is created with.
#[derive(Clone, Debug, Default)]
pub(crate) struct SctkWindowAttributes {
    pub(crate) title: Option<String>,
    pub(crate) app_id: Option<String>,
    pub(crate) inner_size: Option<Size>,
}

/// Where a window's view is in its life in the engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(feature = "testing"), allow(dead_code))]
pub(crate) enum SctkViewState {
    /// The engine was asked to add the view, and has not said whether it
    /// did. The view can not be sent anything yet.
    Adding,
    /// The engine has the view.
    Added,
    /// The engine did not add the view. The window is of no use.
    Failed,
    /// The engine was asked to remove the view, and may still draw to the
    /// window until it is done.
    Removing,
    /// The engine is done with the view. The window can go.
    Removed,
}

pub struct SctkFlutterWindow {
    inner: Arc<SctkFlutterWindowInner>,
    /// Set by the platform thread, and by the engine's callbacks for adding
    /// and removing the view, which run on the platform thread too.
    view_state: Arc<parking_lot::Mutex<SctkViewState>>,
}

impl SctkFlutterWindow {
    pub(crate) fn new(
        view_id: FlutterViewId,
        engine: FlutterEngineWeakRef,
        qh: &QueueHandle<SctkApplicationState>,
        shell: &SctkShell,
        rendering: &SctkRendering,
        attributes: SctkWindowAttributes,
    ) -> Result<Self, SctkFlutterWindowCreateError> {
        let SctkShell {
            compositor_state,
            xdg_shell_state,
            frame_clock,
        } = shell;
        let renderer = rendering.renderer;
        let opengl_target_type = rendering.opengl_target_type;
        // The engine has no Impeller path for these backing stores
        // (https://github.com/flutter/flutter/issues/151670): it would draw
        // nothing at all.
        if renderer == Renderer::Impeller
            && matches!(opengl_target_type, FlutterOpenGLTargetType::Surface)
        {
            return Err(SctkFlutterWindowCreateError::UnsupportedTargetType(
                opengl_target_type,
                renderer,
            ));
        }

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

        let render_context = rendering.render_context.clone();
        let surface = new_wayland_window_surface(
            &render_context.lock().unwrap(),
            window.wl_surface(),
            default_size.to_physical::<u32>(1.0),
        )?;

        let inner = Arc::new_cyclic(|inner| SctkFlutterWindowInner {
            id: view_id,
            surface: Mutex::new(surface),
            window,
            engine,
            qh: qh.clone(),
            compositor_handler: SctkCompositorHandler::new(
                inner.clone(),
                &rendering.framebuffer,
                opengl_target_type,
            ),
            render_context,
            frame_clock: frame_clock.clone(),
            sync: Default::default(),
            surface_frame: Default::default(),
            pointers: Default::default(),
            current_size: Default::default(),
            current_scale_factor: RwLock::new(1.0),
            default_size,
        });

        // The startup window is the implicit view, which the engine always
        // has.
        let view_state = match view_id {
            IMPLICIT_VIEW_ID => SctkViewState::Added,
            _ => SctkViewState::Adding,
        };

        Ok(Self {
            inner,
            view_state: Arc::new(parking_lot::Mutex::new(view_state)),
        })
    }

    pub(crate) fn view_state(&self) -> SctkViewState {
        *self.view_state.lock()
    }

    #[cfg_attr(not(feature = "testing"), allow(dead_code))]
    pub(crate) fn set_view_state(&self, view_state: SctkViewState) {
        *self.view_state.lock() = view_state;
    }

    #[cfg_attr(not(feature = "testing"), allow(dead_code))]
    /// For the engine's callbacks, which report back on the view.
    pub(crate) fn shared_view_state(&self) -> Arc<parking_lot::Mutex<SctkViewState>> {
        self.view_state.clone()
    }

    #[cfg_attr(not(feature = "testing"), allow(dead_code))]
    /// What the engine is told about the window's view before the
    /// compositor has said how large it is: the size it asks for.
    pub(crate) fn initial_metrics(&self) -> FlutterWindowMetrics {
        let size = self.inner.default_size.to_physical::<u32>(1.0);
        FlutterWindowMetrics {
            width: size.width as usize,
            height: size.height as usize,
            pixel_ratio: 1.0,
            display_id: self.inner.get_display_id().unwrap_or_default(),
            constraints: None,
        }
    }

    pub fn view_id(&self) -> FlutterViewId {
        self.inner.id
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
        FlutterView::new(self.inner.id, self.inner.compositor_handler.clone())
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

    #[error("The {0:?} OpenGL target type is not supported with {1:?}")]
    UnsupportedTargetType(FlutterOpenGLTargetType, Renderer),
}
