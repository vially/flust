use std::sync::{Arc, Mutex};

use dpi::PhysicalSize;
use flutter_engine::view::{FlutterView, IMPLICIT_VIEW_ID};
use flutter_glutin::{
    builder::FlutterEGLContext,
    context::{Context, ResourceContext},
    handler::GlutinOpenGLHandler,
};
use flutter_runner_api::ApplicationAttributes;
use log::warn;
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

use crate::egl::FlutterEGLContextWaylandExt;
use crate::{application::SctkApplicationState, egl::CreateWaylandContextError};

pub struct SctkFlutterWindow {
    id: u32,
    window: Window,
    context: Arc<Mutex<Context>>,
    resource_context: Arc<Mutex<ResourceContext>>,
}

impl SctkFlutterWindow {
    pub fn new(
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

        let size = attributes
            .inner_size
            .map_or(PhysicalSize::<u32>::new(1280, 720), |size| {
                size.to_physical::<u32>(1.0)
            });

        window.set_min_size(Some((256, 256)));
        window.commit();

        let (context, resource_context) =
            FlutterEGLContext::new_wayland_context(window.wl_surface(), size)?;

        Ok(Self {
            id: IMPLICIT_VIEW_ID,
            window,
            context: Arc::new(Mutex::new(context)),
            resource_context: Arc::new(Mutex::new(resource_context)),
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
        _surface: &WlSurface,
        _new_scale_factor: i32,
    ) {
        warn!("`scale_factor_changed` handler not implemented for window");
    }

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
}

#[derive(Error, Debug)]
pub enum SctkFlutterWindowCreateError {
    #[error("Failed to create Wayland EGL context")]
    CreateWaylandContextError(#[from] CreateWaylandContextError),
}
