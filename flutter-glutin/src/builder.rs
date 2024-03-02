use dpi::PhysicalSize;
use glutin::{
    config::{Api, Config, ConfigSurfaceTypes, ConfigTemplateBuilder},
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
        let raw_window_handle = self
            .attributes
            .raw_window_handle
            .ok_or(ContextBuildError::InvalidWindowHandle)?;

        // Get display from `raw_display_handle` if present (`sctk`), or from `config` otherwise (`winit`).
        let display = self
            .attributes
            .raw_display_handle
            .map_or_else(
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
            .ok_or(ContextBuildError::InvalidDisplayHandle)?;

        let size = self.attributes.size.ok_or(ContextBuildError::InvalidSize)?;

        // Use config from attributes if present (`winit`), or build a default one otherwise (`sctk`).
        let config = self
            .attributes
            .config
            .map_or_else(|| new_default_config(&display, raw_window_handle), Ok)?;

        let render_attributes_gl = ContextAttributesBuilder::new()
            .with_context_api(glutin::context::ContextApi::OpenGl(None))
            .build(Some(raw_window_handle));

        let render_attributes_gles = ContextAttributesBuilder::new()
            .with_context_api(glutin::context::ContextApi::Gles(None))
            .build(Some(raw_window_handle));

        // Create a context, trying OpenGL and then OpenGL ES.
        let render_context = unsafe {
            display
                .create_context(&config, &render_attributes_gl)
                .or_else(|_| display.create_context(&config, &render_attributes_gles))
        }?;

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
}

#[derive(Error, Debug)]
pub enum ContextBuildError {
    #[error("Invalid window handle attribute")]
    InvalidWindowHandle,

    #[error("Invalid display handle attribute")]
    InvalidDisplayHandle,

    #[error("No available config was found")]
    NoAvailableConfigFound,

    #[error("Invalid size attribute")]
    InvalidSize,

    #[error("Unexpected resource context API (expected EGL)")]
    InvalidResourceContextApi,

    #[error(transparent)]
    GlutinError(#[from] glutin::error::Error),
}

fn new_default_config(
    display: &Display,
    raw_window_handle: RawWindowHandle,
) -> Result<Config, ContextBuildError> {
    let config_template = ConfigTemplateBuilder::new()
        .compatible_with_native_window(raw_window_handle)
        .with_surface_type(ConfigSurfaceTypes::WINDOW)
        .with_api(Api::GLES2 | Api::GLES3 | Api::OPENGL)
        .build();

    unsafe { display.find_configs(config_template) }?
        .next()
        .ok_or(ContextBuildError::NoAvailableConfigFound)
}
