use std::{
    num::NonZeroU32,
    sync::{Arc, Mutex},
};

use dpi::{LogicalSize, PhysicalSize, Size};
use flutter_engine::{
    view::{FlutterView, IMPLICIT_VIEW_ID},
    FlutterEngineWeakRef,
};
use flutter_glutin::{
    builder::FlutterEGLContext,
    context::{Context, ResourceContext},
    handler::GlutinOpenGLHandler,
};
use flutter_runner_api::ApplicationAttributes;
use log::{error, warn};
use smithay_client_toolkit::{
    compositor::CompositorState,
    seat::pointer::PointerEvent,
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

use crate::egl::{FlutterEGLContextWaylandExt, NonZeroU32PhysicalSize};
use crate::{application::SctkApplicationState, egl::CreateWaylandContextError};

pub struct SctkFlutterWindow {
    id: u32,
    window: Window,
    engine: FlutterEngineWeakRef,
    context: Arc<Mutex<Context>>,
    resource_context: Arc<Mutex<ResourceContext>>,
    current_size: Option<Size>,
    current_scale_factor: f64,
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

        let current_size = attributes
            .inner_size
            .unwrap_or(Size::Logical(LogicalSize::<f64>::new(1280.0, 720.0)));

        let (context, resource_context) = FlutterEGLContext::new_wayland_context(
            window.wl_surface(),
            current_size.to_physical::<u32>(1.0),
        )?;

        Ok(Self {
            id: IMPLICIT_VIEW_ID,
            window,
            engine,
            context: Arc::new(Mutex::new(context)),
            resource_context: Arc::new(Mutex::new(resource_context)),
            current_size: Some(current_size),
            current_scale_factor: 1.0,
        })
    }

    pub fn xdg_toplevel_id(&self) -> ObjectId {
        self.window.xdg_toplevel().id()
    }

    pub fn wl_surface_id(&self) -> ObjectId {
        self.window.wl_surface().id()
    }

    pub(crate) fn create_flutter_view(&self) -> FlutterView {
        let opengl_handler =
            GlutinOpenGLHandler::new(self.context.clone(), self.resource_context.clone());
        FlutterView::new(self.id, opengl_handler)
    }

    pub(crate) fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        surface: &WlSurface,
        new_scale_factor: i32,
    ) {
        self.current_scale_factor = new_scale_factor.into();

        self.current_size = self
            .current_size
            .map(|size| Size::from(size.to_logical::<u32>(self.current_scale_factor)));

        let Some(physical_size) = self.current_size.and_then(|size| {
            size.to_physical::<u32>(self.current_scale_factor)
                .non_zero()
        }) else {
            error!("Invalid physical size while handling `scale_factor_changed` event");
            return;
        };

        self.resize_egl_surface(physical_size);

        if let Some(engine) = self.engine.upgrade() {
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
        _configure: WindowConfigure,
        _serial: u32,
    ) {
        warn!("`configure` handler not implemented for window");
    }

    pub(crate) fn pointer_event(
        &mut self,
        _conn: &Connection,
        _pointer: &WlPointer,
        _event: &PointerEvent,
    ) {
        warn!("`pointer_event` not implemented for window");
    }

    fn resize_egl_surface(&self, size: PhysicalSize<NonZeroU32>) {
        self.context.lock().unwrap().resize(size);
    }
}

#[derive(Error, Debug)]
pub enum SctkFlutterWindowCreateError {
    #[error("Failed to create Wayland EGL context")]
    CreateWaylandContextError(#[from] CreateWaylandContextError),
}
