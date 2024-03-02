use dpi::PhysicalSize;
use glutin::{
    config::Config,
    context::{ContextAttributesBuilder, NotCurrentContext},
    display::{Display, DisplayApiPreference, GetGlDisplay},
    prelude::{GlDisplay, NotCurrentGlContext},
    surface::{SurfaceAttributesBuilder, WindowSurface},
};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use std::num::NonZeroU32;
use thiserror::Error;

use crate::context::{Context, ResourceContext};

pub type FlutterEGLContext = (Context, ResourceContext);

#[derive(Debug, Clone, Default)]
pub struct ContextBuilderAttributes {
    pub raw_window_handle: Option<RawWindowHandle>,
    pub raw_display_handle: Option<RawDisplayHandle>,
    pub config: Option<Config>,
    pub size: Option<PhysicalSize<NonZeroU32>>,
}

impl ContextBuilderAttributes {
    pub fn new() -> Self {
        Default::default()
    }
}

#[derive(Clone, Default)]
pub struct ContextBuilder {
    attributes: ContextBuilderAttributes,
}

impl ContextBuilder {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn build(self) -> Result<FlutterEGLContext, ContextBuildError> {
        let Some(raw_window_handle) = self.attributes.raw_window_handle else {
            return Err(ContextBuildError::InvalidWindowHandle);
        };

        let Some(display) = self.display() else {
            return Err(ContextBuildError::InvalidDisplayHandle);
        };

        let Some(config) = self.attributes.config else {
            return Err(ContextBuildError::MissingConfig);
        };

        let Some(size) = self.attributes.size else {
            return Err(ContextBuildError::InvalidSize);
        };

        let render_attributes = ContextAttributesBuilder::new().build(Some(raw_window_handle));
        let render_context = unsafe { display.create_context(&config, &render_attributes)? };
        let surface_attributes = SurfaceAttributesBuilder::<WindowSurface>::default().build(
            raw_window_handle,
            size.width,
            size.height,
        );
        let surface = unsafe { display.create_window_surface(&config, &surface_attributes)? };

        let resource_attributes = ContextAttributesBuilder::new()
            .with_sharing(&render_context)
            .build(Some(raw_window_handle));
        let resource_context = unsafe { display.create_context(&config, &resource_attributes)? };

        let NotCurrentContext::Egl(resource_context) = resource_context else {
            return Err(ContextBuildError::InvalidResourceContextApi);
        };
        let resource_context = ResourceContext::new(resource_context.treat_as_possibly_current());

        let context = Context::new(display, surface, render_context.treat_as_possibly_current());

        Ok((context, resource_context))
    }

    pub fn with_raw_display_handle(mut self, raw_display_handle: RawDisplayHandle) -> Self {
        self.attributes.raw_display_handle = Some(raw_display_handle);
        self
    }

    pub fn with_raw_window_handle(mut self, raw_window_handle: RawWindowHandle) -> Self {
        self.attributes.raw_window_handle = Some(raw_window_handle);
        self
    }

    pub fn with_config(mut self, config: Config) -> Self {
        self.attributes.config = Some(config);
        self
    }

    pub fn with_size(mut self, size: Option<PhysicalSize<NonZeroU32>>) -> Self {
        self.attributes.size = size;
        self
    }

    /// Get display from `raw_display_handle` if present, or from `config` otherwise.
    fn display(&self) -> Option<Display> {
        self.attributes.raw_display_handle.map_or_else(
            || {
                self.attributes
                    .config
                    .as_ref()
                    .map(|config| config.display())
            },
            |raw_display_handle| unsafe {
                Display::new(raw_display_handle, DisplayApiPreference::Egl).ok()
            },
        )
    }
}

#[derive(Error, Debug)]
pub enum ContextBuildError {
    #[error("Invalid window handle attribute")]
    InvalidWindowHandle,

    #[error("Invalid display handle attribute")]
    InvalidDisplayHandle,

    #[error("Missing config attribute")]
    MissingConfig,

    #[error("Invalid size attribute")]
    InvalidSize,

    #[error("Unexpected resource context API (expected EGL)")]
    InvalidResourceContextApi,

    #[error(transparent)]
    GlutinError(#[from] glutin::error::Error),
}
