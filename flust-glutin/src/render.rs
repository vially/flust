//! A render context that is not tied to a window, for drawing several
//! windows.
//!
//! The engine draws every view in the one context it is given, without
//! saying which view it is drawing. [`RenderContext`] is made current
//! without a surface for that, and on a window's [`WindowSurface`] only to
//! put a frame into the window and present it.

use std::{
    ffi::{c_void, CStr},
    num::NonZeroU32,
};

use dpi::PhysicalSize;
use glutin::{
    api::egl::{
        config::Config, context::PossiblyCurrentContext, display::Display, surface::Surface,
    },
    config::{Api, ConfigSurfaceTypes, ConfigTemplateBuilder},
    context::{ContextApi, ContextAttributesBuilder},
    prelude::{GlDisplay, NotCurrentGlContext, PossiblyCurrentGlContext},
    surface::{GlSurface, SurfaceAttributesBuilder, SwapInterval, WindowSurface as Window},
};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use thiserror::Error;

use crate::{builder::ApiPreference, context::ResourceContext};

/// The context the engine draws in, and that frames are put into windows
/// from. It is current on one thread at a time.
pub struct RenderContext {
    display: Display,
    config: Config,
    /// Only `None` while it is being made not current.
    context: Option<PossiblyCurrentContext>,
}

impl RenderContext {
    /// Creates a render context and a resource context that shares with it,
    /// for windows of this display.
    ///
    /// # Safety
    ///
    /// The display handle has to stay valid for as long as the contexts and
    /// the surfaces made with them exist.
    pub unsafe fn new(
        raw_display_handle: RawDisplayHandle,
        api_preference: ApiPreference,
    ) -> Result<(RenderContext, ResourceContext), RenderContextError> {
        let display = Display::new(raw_display_handle)?;

        let template = ConfigTemplateBuilder::new()
            .with_surface_type(ConfigSurfaceTypes::WINDOW)
            .with_api(Api::GLES2 | Api::GLES3 | Api::OPENGL)
            .build();
        let config = display
            .find_configs(template)?
            .next()
            .ok_or(RenderContextError::NoConfig)?;

        let gl = ContextAttributesBuilder::new()
            .with_context_api(ContextApi::OpenGl(None))
            .build(None);
        let gles = ContextAttributesBuilder::new()
            .with_context_api(ContextApi::Gles(None))
            .build(None);
        let (first, second) = match api_preference {
            ApiPreference::OpenGl => (&gl, &gles),
            ApiPreference::OpenGlEs => (&gles, &gl),
        };
        let context = display
            .create_context(&config, first)
            .or_else(|_| display.create_context(&config, second))?;

        let resource_attributes = ContextAttributesBuilder::new()
            .with_sharing(&context)
            .build(None);
        let resource_context = display.create_context(&config, &resource_attributes)?;

        let render_context = RenderContext {
            display,
            config,
            context: Some(context.treat_as_possibly_current()),
        };
        let resource_context = ResourceContext::new(resource_context.treat_as_possibly_current());

        Ok((render_context, resource_context))
    }

    /// Makes the context current on this thread, without a surface.
    pub fn make_current(&self) -> bool {
        self.context()
            .is_some_and(|context| context.make_current_surfaceless().is_ok())
    }

    /// Makes the context current on this thread, drawing to and reading from
    /// the window's surface.
    pub fn make_current_with(&self, surface: &mut WindowSurface) -> bool {
        let Some(context) = self.context() else {
            return false;
        };
        if context.make_current(&surface.surface).is_err() {
            return false;
        }
        // The swap interval belongs to the surface, and is set with it
        // current.
        if let Some(interval) = surface.swap_interval.take() {
            let _ = surface.surface.set_swap_interval(context, interval);
        }
        true
    }

    pub fn make_not_current(&mut self) -> bool {
        let Some(context) = self.context.take() else {
            return false;
        };
        match context.make_not_current() {
            Ok(context) => {
                self.context = Some(context.treat_as_possibly_current());
                true
            }
            Err(_) => false,
        }
    }

    fn context(&self) -> Option<&PossiblyCurrentContext> {
        self.context.as_ref()
    }

    pub fn get_proc_address(&self, proc: &CStr) -> *const c_void {
        self.display.get_proc_address(proc)
    }

    /// Creates the surface of a window, to present frames to with this
    /// context. Does not need the context to be current.
    ///
    /// # Safety
    ///
    /// The window handle has to stay valid for as long as the surface
    /// exists.
    pub unsafe fn create_window_surface(
        &self,
        raw_window_handle: RawWindowHandle,
        size: PhysicalSize<NonZeroU32>,
        swap_interval: Option<SwapInterval>,
    ) -> Result<WindowSurface, RenderContextError> {
        let attributes = SurfaceAttributesBuilder::<Window>::new().build(
            raw_window_handle,
            size.width,
            size.height,
        );
        let surface = self
            .display
            .create_window_surface(&self.config, &attributes)?;
        Ok(WindowSurface {
            surface,
            swap_interval,
        })
    }

    /// Gives the window's surface this size, for the frame that is drawn to
    /// it next.
    pub fn resize(&self, surface: &WindowSurface, size: PhysicalSize<NonZeroU32>) {
        if let Some(context) = self.context() {
            surface.surface.resize(context, size.width, size.height);
        }
    }

    /// Presents what was drawn to the window's surface. The context has to
    /// be current with it.
    pub fn swap_buffers(&self, surface: &WindowSurface) -> bool {
        self.context()
            .is_some_and(|context| surface.surface.swap_buffers(context).is_ok())
    }
}

// The context is current on one thread at a time, which the engine and the
// embedder see to: see `Context` for the same.
unsafe impl Send for RenderContext {}

/// The surface of a window, which frames are presented to with a
/// [`RenderContext`].
pub struct WindowSurface {
    surface: Surface<Window>,
    /// Set the first time the surface is made current.
    swap_interval: Option<SwapInterval>,
}

unsafe impl Send for WindowSurface {}

#[derive(Error, Debug)]
pub enum RenderContextError {
    #[error("No suitable EGL config was found")]
    NoConfig,

    #[error(transparent)]
    GlutinError(#[from] glutin::error::Error),
}
