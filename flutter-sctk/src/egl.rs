use std::num::NonZeroU32;

use dpi::PhysicalSize;
use flutter_glutin::builder::{ContextBuildError, ContextBuilder, FlutterEGLContext};
use glutin::surface::SwapInterval;
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use thiserror::Error;
use wayland_client::{protocol::wl_surface, Proxy};

pub(crate) trait FlutterEGLContextWaylandExt {
    fn new_wayland_context(
        surface: &wl_surface::WlSurface,
        size: PhysicalSize<u32>,
    ) -> Result<FlutterEGLContext, CreateWaylandContextError>;
}

impl FlutterEGLContextWaylandExt for FlutterEGLContext {
    fn new_wayland_context(
        surface: &wl_surface::WlSurface,
        size: PhysicalSize<u32>,
    ) -> Result<FlutterEGLContext, CreateWaylandContextError> {
        let mut wl_display_handle = WaylandDisplayHandle::empty();
        wl_display_handle.display = surface
            .backend()
            .upgrade()
            .ok_or(CreateWaylandContextError::ConnectionClosed)?
            .display_ptr() as *mut _;
        let raw_display_handle = RawDisplayHandle::Wayland(wl_display_handle);

        let mut wl_window_handle = WaylandWindowHandle::empty();
        wl_window_handle.surface = surface.id().as_ptr() as *mut _;
        let raw_window_handle = RawWindowHandle::Wayland(wl_window_handle);

        let (context, resource_context) = ContextBuilder::new()
            .with_raw_display_handle(raw_display_handle)
            .with_raw_window_handle(raw_window_handle)
            .with_swap_interval(SwapInterval::DontWait)
            .with_size(size.non_zero())
            .build()?;

        Ok((context, resource_context))
    }
}

#[derive(Error, Debug)]
pub enum CreateWaylandContextError {
    #[error("Connection has been closed")]
    ConnectionClosed,

    #[error("Failed to build context")]
    ContextBuildError(#[from] ContextBuildError),
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
