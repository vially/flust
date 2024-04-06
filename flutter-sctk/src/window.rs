use std::{
    collections::HashMap,
    num::NonZeroU32,
    sync::{Arc, RwLock},
};

use dpi::{LogicalSize, PhysicalSize, Size};
use flutter_engine::{
    ffi::FlutterPointerEvent,
    view::{FlutterView, IMPLICIT_VIEW_ID},
    FlutterEngineWeakRef,
};
use flutter_glutin::builder::FlutterEGLContext;
use flutter_runner_api::ApplicationAttributes;
use log::{error, trace};
use smithay_client_toolkit::{
    compositor::CompositorState,
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
use wayland_backend::client::ObjectId;
use wayland_client::{
    protocol::{wl_pointer::WlPointer, wl_surface::WlSurface},
    Connection, Proxy, QueueHandle,
};

use crate::{
    application::SctkApplicationState, egl::CreateWaylandContextError, handler::SctkOpenGLHandler,
    pointer::SctkPointerEvent,
};
use crate::{
    egl::{FlutterEGLContextWaylandExt, NonZeroU32PhysicalSize},
    pointer::Pointer,
};

pub struct SctkFlutterWindowInner {
    id: u32,
    window: Window,
    engine: FlutterEngineWeakRef,
    current_size: RwLock<Option<Size>>,
    current_scale_factor: RwLock<f64>,
    default_size: Size,
    pointers: RwLock<HashMap<ObjectId, Pointer>>,
    opengl_handler: SctkOpenGLHandler,
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

        // not implemented
        true
    }

    // Note: This callback is executed on the *render* thread.
    pub(super) fn on_frame_presented(&self) {
        trace!("window frame presented");

        // not implemented
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

        let inner = Arc::new_cyclic(|inner| SctkFlutterWindowInner {
            id: IMPLICIT_VIEW_ID,
            window,
            engine,
            opengl_handler: SctkOpenGLHandler::new(inner.clone(), context, resource_context),
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

    pub fn wl_surface_id(&self) -> ObjectId {
        self.inner.window.wl_surface().id()
    }

    pub fn xdg_toplevel(&self) -> XdgToplevel {
        self.inner.window.xdg_toplevel().clone()
    }

    pub(crate) fn create_flutter_view(&self) -> FlutterView {
        FlutterView::new(self.inner.id, self.inner.opengl_handler.clone())
    }

    pub(crate) fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        surface: &WlSurface,
        new_scale_factor: i32,
    ) {
        self.inner.scale_internal_size(new_scale_factor.into());

        let Some(physical_size) = self.inner.non_zero_physical_size() else {
            error!("Invalid physical size while handling `scale_factor_changed` event");
            return;
        };

        self.inner.opengl_handler.resize(physical_size);

        if let Some(engine) = self.inner.engine.upgrade() {
            engine.send_window_metrics_event(
                usize::try_from(physical_size.width.get()).unwrap(),
                usize::try_from(physical_size.height.get()).unwrap(),
                new_scale_factor as f64,
            );
        }

        // Warning: This can cause crashes until `FlutterResizeSynchronizer` is implemented
        // TODO: Fix this by implementing proper synchronization logic
        surface.set_buffer_scale(new_scale_factor);
    }

    // TODO: Implement `FlutterResizeSynchronizer`
    pub(crate) fn configure(
        &mut self,
        _conn: &Connection,
        configure: WindowConfigure,
        _serial: u32,
    ) {
        let new_logical_size = WindowLogicalSize::try_from(configure.new_size)
            .map(|size| size.into())
            .unwrap_or(self.inner.default_size);

        self.inner.store_current_size(new_logical_size);

        let scale_factor = self.inner.load_current_scale_factor();

        let Some(physical_size) = new_logical_size.to_physical(scale_factor).non_zero() else {
            error!("Unable to convert window configure event to a physical size");
            return;
        };

        self.inner.opengl_handler.resize(physical_size);

        if let Some(engine) = self.inner.engine.upgrade() {
            engine.send_window_metrics_event(
                usize::try_from(physical_size.width.get()).unwrap(),
                usize::try_from(physical_size.height.get()).unwrap(),
                scale_factor,
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
            SctkPointerEvent::new(event.clone(), *pointer, scale_factor)
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
