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
        FlutterOpenGLTargetType, FlutterPointerEvent, FlutterViewConstraints, FlutterViewId,
        FlutterWindowMetrics, IMPLICIT_VIEW_ID,
    },
    renderer::Renderer,
    view::FlutterView,
    FlutterEngineWeakRef,
};
use flust_engine_sys::{FlutterEngineDisplayId, FlutterEngineGetCurrentTime};
use flust_glutin::render::{RenderContext, WindowSurface};
use smithay_client_toolkit::{
    compositor::{CompositorState, SurfaceData},
    reexports::protocols::xdg::{
        dialog::v1::client::xdg_wm_dialog_v1::XdgWmDialogV1,
        shell::client::{xdg_popup::XdgPopup, xdg_surface::XdgSurface, xdg_toplevel::XdgToplevel},
    },
    seat::pointer::{PointerEvent, PointerEventKind},
    shell::xdg::XdgShell,
};
use thiserror::Error;
use tracing::{debug, error, trace};
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
    popup::{Popup, PopupInput, PopupKind, PopupPlacement},
    sync::{ContentSync, FrameAction, PlatformAction, SurfaceGeometry, SurfaceSync, WindowSync},
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
    window: Role,
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
    sync: Mutex<WindowSync>,
    /// The size of the frame the engine has drawn straight into the window's
    /// framebuffer and not presented yet. Only used with
    /// `FlutterOpenGLTargetType::Surface`.
    surface_frame: Mutex<Option<PhysicalSize<u32>>>,
    /// The window's state in the last configure.
    toplevel_state: Mutex<ToplevelConfigure>,
    /// The window this one is a modal dialog of.
    modal_parent: Option<FlutterViewId>,
    /// For a popup: the window it is a popup of, and how it takes input.
    popup_parent: Option<(FlutterViewId, PopupKind)>,
    /// For a window sized to its content: the sizes the content may take.
    content_constraints: Mutex<Option<SizeConstraints>>,
}

/// What a window's surface is to the compositor.
pub(crate) enum Role {
    Toplevel(Toplevel),
    Popup(Popup),
}

impl Role {
    fn wl_surface(&self) -> &WlSurface {
        match self {
            Self::Toplevel(toplevel) => toplevel.wl_surface(),
            Self::Popup(popup) => popup.wl_surface(),
        }
    }

    fn xdg_surface(&self) -> &XdgSurface {
        match self {
            Self::Toplevel(toplevel) => toplevel.xdg_surface(),
            Self::Popup(popup) => popup.xdg_surface(),
        }
    }

    fn toplevel(&self) -> Option<&Toplevel> {
        match self {
            Self::Toplevel(toplevel) => Some(toplevel),
            Self::Popup(_) => None,
        }
    }

    fn popup(&self) -> Option<&Popup> {
        match self {
            Self::Toplevel(_) => None,
            Self::Popup(popup) => Some(popup),
        }
    }
}

/// The smallest and largest size of a window's content, in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SizeConstraints {
    pub(crate) min: LogicalSize<f64>,
    pub(crate) max: LogicalSize<f64>,
}

/// The largest a window sized to its content gets, in buffer pixels, so
/// that content without a limit of its own does not ask for a buffer
/// larger than any output.
const MAX_CONTENT_SIZE: u32 = 8192;

impl SizeConstraints {
    /// In buffer pixels, at `scale`.
    fn to_physical(self, scale: f64) -> FlutterViewConstraints {
        let physical = |value: f64, at_least: u32| {
            let value = value * scale;
            match value.is_finite() {
                true => (value.round() as u32).clamp(at_least, MAX_CONTENT_SIZE),
                false => MAX_CONTENT_SIZE,
            }
        };
        let min_width = physical(self.min.width, 1);
        let min_height = physical(self.min.height, 1);
        FlutterViewConstraints {
            min_width: min_width as usize,
            min_height: min_height as usize,
            max_width: physical(self.max.width, min_width) as usize,
            max_height: physical(self.max.height, min_height) as usize,
        }
    }
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

        match action {
            // Not an error for the engine: there is nothing it could do
            // about it, and the frame that was asked for is on its way.
            FrameAction::Drop => return Ok(()),
            // The window gets the size of the frame first.
            FrameAction::Resize => {
                drop(sync);
                self.frame_clock.wake_platform_thread();
                return Ok(());
            }
            FrameAction::Commit { .. } | FrameAction::Present => {}
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

        let constraints = self
            .content_constraints
            .lock()
            .unwrap()
            .map(|constraints| constraints.to_physical(geometry.scale.into()));
        // The engine wants a size within the constraints.
        let (width, height) = match constraints {
            Some(constraints) => (
                (geometry.size.width as usize).clamp(constraints.min_width, constraints.max_width),
                (geometry.size.height as usize)
                    .clamp(constraints.min_height, constraints.max_height),
            ),
            None => (geometry.size.width as usize, geometry.size.height as usize),
        };

        engine.send_window_metrics(
            self.id,
            FlutterWindowMetrics {
                width,
                height,
                pixel_ratio: geometry.scale.into(),
                display_id: self.get_display_id().unwrap_or_default(),
                constraints,
            },
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
    /// For telling the compositor that a window is a modal dialog, if it
    /// offers xdg-dialog.
    pub(crate) xdg_wm_dialog: Option<XdgWmDialogV1>,
    pub(crate) frame_clock: FrameClock,
}

/// The window that a dialog is modal to.
#[derive(Clone, Debug)]
pub(crate) struct SctkModalParent {
    pub(crate) view_id: FlutterViewId,
    pub(crate) xdg_toplevel: XdgToplevel,
}

/// What one window is created with.
#[derive(Clone, Debug, Default)]
pub(crate) struct SctkWindowAttributes {
    pub(crate) title: Option<String>,
    pub(crate) app_id: Option<String>,
    pub(crate) inner_size: Option<Size>,
    /// The smallest and largest size the window can be resized to, in
    /// logical pixels. Unlimited if not set.
    pub(crate) min_size: Option<LogicalSize<u32>>,
    pub(crate) max_size: Option<LogicalSize<u32>>,
    /// Makes the window a modal dialog of another one.
    pub(crate) modal_parent: Option<SctkModalParent>,
    /// Makes the window a popup, sized to its content.
    pub(crate) popup: Option<SctkPopupAttributes>,
}

/// What a popup or a tooltip is created with.
#[derive(Clone, Debug)]
pub(crate) struct SctkPopupAttributes {
    pub(crate) parent_view_id: FlutterViewId,
    pub(crate) parent_xdg_surface: XdgSurface,
    pub(crate) placement: PopupPlacement,
    pub(crate) constraints: SizeConstraints,
    pub(crate) input: PopupInput,
}

/// Where a window's view is in its life in the engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

/// A window of the application. Clones are the same window.
#[derive(Clone)]
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
            xdg_wm_dialog,
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
        let popup_parent = attributes
            .popup
            .as_ref()
            .map(|popup| (popup.parent_view_id, popup.input.kind()));
        let (window, default_size, sync, content_constraints) = match attributes.popup {
            Some(popup) => {
                // Sized to its content: its first frame decides its size.
                let constraints = popup.constraints;
                let size = LogicalSize::new(
                    constraints.min.width.max(1.0).round() as i32,
                    constraints.min.height.max(1.0).round() as i32,
                );
                let window = Popup::new(
                    xdg_shell_state,
                    compositor_state,
                    surface,
                    &popup.parent_xdg_surface,
                    popup.placement,
                    (size.width, size.height),
                    popup.input,
                    qh,
                )
                .ok_or(SctkFlutterWindowCreateError::Popup)?;
                // The initial commit, without a buffer: the compositor
                // answers it with the first configure.
                window.wl_surface().commit();
                let physical = PhysicalSize::new(size.width as u32, size.height as u32);
                (
                    Role::Popup(window),
                    Size::Logical(size.cast()),
                    WindowSync::Content(ContentSync::new(physical, 1)),
                    Some(constraints),
                )
            }
            None => {
                let mut window = Toplevel::new(xdg_shell_state, surface, qh);

                if let Some(title) = attributes.title {
                    window.xdg_toplevel().set_title(title);
                }

                if let Some(app_id) = attributes.app_id {
                    window.xdg_toplevel().set_app_id(app_id);
                }

                set_size_limits(
                    window.xdg_toplevel(),
                    attributes.min_size,
                    attributes.max_size,
                );

                if let Some(parent) = &attributes.modal_parent {
                    window.set_modal_parent(&parent.xdg_toplevel, xdg_wm_dialog.as_ref(), qh);
                }

                // The initial commit, without a buffer: the compositor
                // answers it with the first configure.
                window.wl_surface().commit();

                let default_size = attributes
                    .inner_size
                    .unwrap_or(Size::Logical(LogicalSize::<f64>::new(1280.0, 720.0)));
                (
                    Role::Toplevel(window),
                    default_size,
                    WindowSync::Sized(SurfaceSync::default()),
                    None,
                )
            }
        };

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
            sync: Mutex::new(sync),
            surface_frame: Default::default(),
            pointers: Default::default(),
            current_size: Default::default(),
            current_scale_factor: RwLock::new(1.0),
            default_size,
            toplevel_state: Default::default(),
            modal_parent: attributes.modal_parent.map(|parent| parent.view_id),
            popup_parent,
            content_constraints: Mutex::new(content_constraints),
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

    pub(crate) fn set_view_state(&self, view_state: SctkViewState) {
        *self.view_state.lock() = view_state;
    }

    /// For the engine's callbacks, which report back on the view.
    pub(crate) fn shared_view_state(&self) -> Arc<parking_lot::Mutex<SctkViewState>> {
        self.view_state.clone()
    }

    /// What the engine is told about the window's view before the
    /// compositor has said how large it is: the size it asks for.
    pub(crate) fn initial_metrics(&self) -> FlutterWindowMetrics {
        let size = self.inner.default_size.to_physical::<u32>(1.0);
        let constraints = self
            .inner
            .content_constraints
            .lock()
            .unwrap()
            .map(|constraints| constraints.to_physical(1.0));
        FlutterWindowMetrics {
            width: size.width as usize,
            height: size.height as usize,
            pixel_ratio: 1.0,
            display_id: self.inner.get_display_id().unwrap_or_default(),
            constraints,
        }
    }

    pub fn view_id(&self) -> FlutterViewId {
        self.inner.id
    }

    /// The window this one is a modal dialog of, if it is one.
    pub(crate) fn modal_parent(&self) -> Option<FlutterViewId> {
        self.inner.modal_parent
    }

    /// For a popup: the window it is a popup of, and how it takes input.
    pub(crate) fn popup_parent(&self) -> Option<(FlutterViewId, PopupKind)> {
        self.inner.popup_parent
    }

    pub fn xdg_toplevel_id(&self) -> Option<ObjectId> {
        Some(self.inner.window.toplevel()?.xdg_toplevel().id())
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

    pub fn xdg_toplevel(&self) -> Option<XdgToplevel> {
        Some(self.inner.window.toplevel()?.xdg_toplevel().clone())
    }

    pub(crate) fn xdg_surface(&self) -> XdgSurface {
        self.inner.window.xdg_surface().clone()
    }

    /// Whether this is the window of `xdg_popup`.
    pub(crate) fn is_xdg_popup(&self, xdg_popup: &XdgPopup) -> bool {
        self.inner
            .window
            .popup()
            .is_some_and(|popup| popup.is_xdg_popup(xdg_popup))
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
                (PlatformAction::Resize(geometry), _) => {
                    drop(sync);
                    let size = geometry.size.to_logical::<f64>(geometry.scale.into());
                    self.inner.store_current_size(size.into());
                    if let Some(popup) = self.inner.window.popup() {
                        popup.reposition(
                            (size.width.round() as i32, size.height.round() as i32),
                            None,
                        );
                    }
                }
            }
        }
    }

    pub(crate) fn scale_factor_changed(&self, new_scale_factor: i32) {
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
        match &mut *self.inner.sync.lock().unwrap() {
            WindowSync::Sized(sync) => {
                if let Some(geometry) = self.inner.current_geometry() {
                    sync.set_geometry(geometry);
                }
            }
            WindowSync::Content(sync) => sync.set_scale(new_scale_factor.max(1) as u32),
            WindowSync::Closed => {}
        }
    }

    pub(crate) fn configure(&self, configure: ToplevelConfigure, serial: u32) {
        *self.inner.toplevel_state.lock().unwrap() = configure;

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
        if let WindowSync::Sized(sync) = &mut *self.inner.sync.lock().unwrap() {
            sync.configure(serial, geometry);
        }
    }

    /// The compositor placed the popup. It keeps the size of its content.
    pub(crate) fn popup_configured(&self, serial: u32) {
        // Acknowledged with the next frame, or right away if the window
        // already shows its content: see `poll`.
        if let WindowSync::Content(sync) = &mut *self.inner.sync.lock().unwrap() {
            sync.configure(serial);
        }
    }

    /// Where the compositor put the popup, relative to its parent, in
    /// logical pixels.
    pub(crate) fn popup_position(&self) -> Option<(i32, i32)> {
        let position = self.inner.window.popup()?.position();
        Some((position.x, position.y))
    }

    /// Places the popup again.
    pub(crate) fn set_placement(&self, placement: PopupPlacement) {
        let Some(popup) = self.inner.window.popup() else {
            return;
        };
        let size = self.content_size();
        popup.reposition(
            (size.width.round() as i32, size.height.round() as i32),
            Some(placement),
        );
    }

    /// The sizes the content of a window sized to it may take.
    pub(crate) fn set_content_constraints(&self, constraints: SizeConstraints) {
        let mut current = self.inner.content_constraints.lock().unwrap();
        if current.is_none() {
            return;
        }
        *current = Some(constraints);
        drop(current);
        if let WindowSync::Content(sync) = &mut *self.inner.sync.lock().unwrap() {
            sync.request_frame();
        }
    }

    /// Takes a popup off the screen for good: nothing is committed to its
    /// surface after this. A popup with a grab has to be the topmost one.
    /// Its view may still draw until the engine removes it.
    pub(crate) fn dismiss(&self) {
        let Some(popup) = self.inner.window.popup() else {
            return;
        };
        // Under the lock: the raster thread does not commit in between.
        let mut sync = self.inner.sync.lock().unwrap();
        *sync = WindowSync::Closed;
        popup.dismiss();
    }

    pub(crate) fn surface_outputs_changed(&self) {
        // Only the display changed: tell the engine, for the size it is
        // drawing at.
        let geometry = self.inner.sync.lock().unwrap().engine_geometry();
        if let Some(geometry) = geometry {
            self.inner.send_window_metrics(geometry);
        }
    }

    /// The size of the window's content, in logical pixels.
    pub(crate) fn content_size(&self) -> LogicalSize<f64> {
        let scale_factor = self.inner.load_current_scale_factor();
        self.inner
            .load_current_size()
            .unwrap_or(self.inner.default_size)
            .to_logical(scale_factor)
    }

    /// The window's state in the last configure.
    pub(crate) fn toplevel_state(&self) -> ToplevelConfigure {
        *self.inner.toplevel_state.lock().unwrap()
    }

    /// The window's `xdg_toplevel`, for what only a toplevel has. A popup
    /// has none: what is asked of it is ignored.
    fn toplevel(&self) -> Option<&XdgToplevel> {
        let toplevel = self.inner.window.toplevel().map(Toplevel::xdg_toplevel);
        if toplevel.is_none() {
            debug!(
                "[view {}] ignoring what only a toplevel can do",
                self.inner.id
            );
        }
        toplevel
    }

    pub(crate) fn set_title(&self, title: String) {
        if let Some(toplevel) = self.toplevel() {
            toplevel.set_title(title);
        }
    }

    /// Asks for another size. The compositor has the last word: a
    /// maximized, fullscreen or tiled window keeps the size it was given.
    /// A window sized to its content keeps that size.
    pub(crate) fn set_size(&self, size: LogicalSize<f64>) {
        let mut sync = self.inner.sync.lock().unwrap();
        let WindowSync::Sized(sync) = &mut *sync else {
            return;
        };
        self.inner.store_current_size(size.into());
        // The engine is asked for frames of this size, and the window takes
        // it with the first of them: see `poll`.
        if let Some(geometry) = self.inner.current_geometry() {
            sync.set_geometry(geometry);
        }
    }

    pub(crate) fn set_size_limits(
        &self,
        min_size: Option<LogicalSize<u32>>,
        max_size: Option<LogicalSize<u32>>,
    ) {
        if let Some(toplevel) = self.toplevel() {
            set_size_limits(toplevel, min_size, max_size);
            self.commit_state();
        }
    }

    pub(crate) fn set_maximized(&self, maximized: bool) {
        match (self.toplevel(), maximized) {
            (Some(toplevel), true) => toplevel.set_maximized(),
            (Some(toplevel), false) => toplevel.unset_maximized(),
            (None, _) => {}
        }
    }

    /// Wayland has no way to bring a minimized window back, and compositors
    /// do not say whether a window is minimized.
    pub(crate) fn set_minimized(&self) {
        if let Some(toplevel) = self.toplevel() {
            toplevel.set_minimized();
        }
    }

    pub(crate) fn set_fullscreen(&self, fullscreen: bool) {
        match (self.toplevel(), fullscreen) {
            (Some(toplevel), true) => toplevel.set_fullscreen(None),
            (Some(toplevel), false) => toplevel.unset_fullscreen(),
            (None, _) => {}
        }
    }

    /// Commits state of the window that waits for a commit without a new
    /// frame, such as its size limits. The raster thread is kept from
    /// committing in between by the lock.
    fn commit_state(&self) {
        let _sync = self.inner.sync.lock().unwrap();
        self.inner.window.wl_surface().commit();
    }

    /// Sends a pointer event on the window to the engine.
    ///
    /// While the window is `blocked` (it has a modal dialog open), presses
    /// and scrolling are dropped. Moving the pointer is not, and neither is
    /// the release of a press that started before: Flutter expects it. A
    /// release whose press was dropped is dropped too.
    pub(crate) fn pointer_event(&self, pointer: &WlPointer, event: &PointerEvent, blocked: bool) {
        let sctk_pointer_event = {
            let mut pointers = self.inner.pointers.write().unwrap();
            let pointer = pointers
                .entry(pointer.id())
                .or_insert_with(|| Pointer::new(pointer.id().protocol_id() as i32));

            match event.kind {
                PointerEventKind::Press { .. } | PointerEventKind::Axis { .. } if blocked => {
                    return;
                }
                PointerEventKind::Release { .. } if pointer.pressed == 0 => return,
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

/// The limits are in the window's geometry, which is its surface's size: in
/// logical pixels. Zero is no limit.
fn set_size_limits(
    xdg_toplevel: &XdgToplevel,
    min_size: Option<LogicalSize<u32>>,
    max_size: Option<LogicalSize<u32>>,
) {
    let (min_width, min_height) = min_size.map_or((0, 0), |size| (size.width, size.height));
    let (max_width, max_height) = max_size.map_or((0, 0), |size| (size.width, size.height));
    let clamp = |value: u32| i32::try_from(value).unwrap_or(i32::MAX);
    xdg_toplevel.set_min_size(clamp(min_width), clamp(min_height));
    xdg_toplevel.set_max_size(clamp(max_width), clamp(max_height));
}

#[derive(Error, Debug)]
pub enum SctkFlutterWindowCreateError {
    #[error("Failed to create Wayland EGL context")]
    CreateWaylandContextError(#[from] CreateWaylandContextError),

    #[error("The {0:?} OpenGL target type is not supported with {1:?}")]
    UnsupportedTargetType(FlutterOpenGLTargetType, Renderer),

    #[error("Failed to create a popup")]
    Popup,
}
