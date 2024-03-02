use std::{error::Error, num::NonZeroU32};

use dpi::PhysicalSize;
use flutter_glutin::{
    builder::ContextBuilder,
    context::{Context, ResourceContext},
};
use glutin::config::ConfigTemplateBuilder;
use glutin_winit::{ApiPreference, DisplayBuilder};
use raw_window_handle::HasRawWindowHandle;
use thiserror::Error;
use winit::{
    event_loop::EventLoop,
    window::{Window, WindowBuilder},
};

use crate::window::FlutterEvent;

pub(crate) fn create_window_contexts(
    window_builder: WindowBuilder,
    event_loop: &EventLoop<FlutterEvent>,
) -> Result<(Window, Context, ResourceContext), Box<dyn Error>> {
    let (window, config) = DisplayBuilder::new()
        .with_preference(ApiPreference::PreferEgl)
        .with_window_builder(Some(window_builder))
        .build(event_loop, ConfigTemplateBuilder::new(), |configs| {
            // TODO: Find out what's the correct way of choosing a config
            configs.last().unwrap()
        })?;

    let Some(window) = window else {
        return Err(ContextError::InvalidWindow.into());
    };

    let (context, resource_context) = ContextBuilder::new()
        .with_raw_window_handle(window.raw_window_handle())
        .with_config(config)
        .with_size(window.inner_size().non_zero())
        .build()?;

    Ok((window, context, resource_context))
}

#[derive(Error, Debug)]
pub enum ContextError {
    #[error("Invalid window")]
    InvalidWindow,
}

/// [`winit::dpi::PhysicalSize<u32>`] non-zero extensions.
trait NonZeroU32PhysicalSize {
    fn non_zero(self) -> Option<PhysicalSize<NonZeroU32>>;
}

impl NonZeroU32PhysicalSize for winit::dpi::PhysicalSize<u32> {
    fn non_zero(self) -> Option<PhysicalSize<NonZeroU32>> {
        let w = NonZeroU32::new(self.width)?;
        let h = NonZeroU32::new(self.height)?;
        Some(PhysicalSize::new(w, h))
    }
}
