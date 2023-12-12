use std::error::Error;

use glutin::{
    config::ConfigTemplateBuilder,
    context::{ContextAttributesBuilder, NotCurrentContext, NotCurrentGlContext},
    display::{GetGlDisplay, GlDisplay},
    surface::SurfaceAttributesBuilder,
};
use glutin_winit::{ApiPreference, DisplayBuilder, GlWindow};
use raw_window_handle::HasRawWindowHandle;
use thiserror::Error;
use winit::{event_loop::EventLoop, window::WindowBuilder};

use crate::{
    context::{Context, ResourceContext},
    window::FlutterEvent,
};

pub(crate) fn create_window_contexts(
    window_builder: WindowBuilder,
    event_loop: &EventLoop<FlutterEvent>,
) -> Result<(Context, ResourceContext), Box<dyn Error>> {
    let template_builder = ConfigTemplateBuilder::new();

    let (window, config) = DisplayBuilder::new()
        .with_preference(ApiPreference::PreferEgl)
        .with_window_builder(Some(window_builder))
        .build(event_loop, template_builder, |configs| {
            // TODO: Find out what's the correct way of choosing a config
            configs.last().unwrap()
        })?;

    let Some(window) = window else {
        return Err(ContextError::InvalidWindow.into());
    };
    let raw_window_handle = window.raw_window_handle();

    let display = config.display();

    let render_attributes = ContextAttributesBuilder::new().build(Some(raw_window_handle));
    let render_context = unsafe { display.create_context(&config, &render_attributes)? };

    let resource_attributes = ContextAttributesBuilder::new()
        .with_sharing(&render_context)
        .build(Some(raw_window_handle));
    let resource_context = unsafe { display.create_context(&config, &resource_attributes)? };

    let surface_attributes = window.build_surface_attributes(SurfaceAttributesBuilder::new());
    let surface = unsafe { display.create_window_surface(&config, &surface_attributes)? };

    let context = Context::new(
        window,
        display,
        surface,
        render_context.treat_as_possibly_current(),
    );

    let NotCurrentContext::Egl(resource_context) = resource_context else {
        return Err(ContextError::InvalidEGLContext.into());
    };

    let resource_context = ResourceContext::new(resource_context.treat_as_possibly_current());

    Ok((context, resource_context))
}

#[derive(Error, Debug)]
pub enum ContextError {
    #[error("Invalid window")]
    InvalidWindow,

    #[error("Invalid EGL context")]
    InvalidEGLContext,
}
