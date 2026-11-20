use std::{num::NonZeroU32, ptr::NonNull};

use dpi::PhysicalSize;
use flust_engine::renderer::Renderer;
use flust_glutin::{
    builder::ApiPreference,
    context::ResourceContext,
    render::{RenderContext, RenderContextError, WindowSurface},
};
use glutin::surface::SwapInterval;
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use thiserror::Error;
use wayland_client::{protocol::wl_surface, Connection, Proxy};

/// Creates the context the engine draws every window in, and its resource
/// context, for the display of this connection.
pub(crate) fn new_wayland_render_context(
    conn: &Connection,
    renderer: Renderer,
) -> Result<(RenderContext, ResourceContext), CreateWaylandContextError> {
    let display = NonNull::new(conn.backend().display_ptr().cast())
        .ok_or(CreateWaylandContextError::ConnectionClosed)?;
    let raw_display_handle = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(display));

    // Impeller draws with its OpenGL ES backend. It works on a desktop
    // OpenGL context as well, but only multisamples what it draws
    // offscreen (layers, backdrop filters) on OpenGL ES 3.
    let api_preference = match renderer {
        Renderer::Skia => ApiPreference::OpenGl,
        Renderer::Impeller => ApiPreference::OpenGlEs,
    };

    // The connection, and with it the display, outlives the contexts: the
    // application owns both, and drops the engine first.
    Ok(unsafe { RenderContext::new(raw_display_handle, api_preference)? })
}

/// Creates the EGL surface of a window, which frames are presented to with
/// the render context.
pub(crate) fn new_wayland_window_surface(
    render_context: &RenderContext,
    surface: &wl_surface::WlSurface,
    size: PhysicalSize<u32>,
) -> Result<WindowSurface, CreateWaylandContextError> {
    let surface = NonNull::new(surface.id().as_ptr().cast())
        .ok_or(CreateWaylandContextError::ConnectionClosed)?;
    let raw_window_handle = RawWindowHandle::Wayland(WaylandWindowHandle::new(surface));
    let size = size
        .non_zero()
        .ok_or(CreateWaylandContextError::InvalidSize)?;

    // The window outlives its EGL surface: see `SctkFlutterWindowInner`.
    Ok(unsafe {
        render_context.create_window_surface(
            raw_window_handle,
            size,
            Some(SwapInterval::DontWait),
        )?
    })
}

#[derive(Error, Debug)]
pub enum CreateWaylandContextError {
    #[error("Connection has been closed")]
    ConnectionClosed,

    #[error("A window surface can not be empty")]
    InvalidSize,

    #[error("Failed to create the render context or a window surface")]
    RenderContextError(#[from] RenderContextError),
}

pub trait NonZeroU32PhysicalSize {
    fn non_zero(self) -> Option<PhysicalSize<NonZeroU32>>;
}

impl NonZeroU32PhysicalSize for PhysicalSize<u32> {
    fn non_zero(self) -> Option<PhysicalSize<NonZeroU32>> {
        let w = NonZeroU32::new(self.width)?;
        let h = NonZeroU32::new(self.height)?;
        Some(PhysicalSize::new(w, h))
    }
}
