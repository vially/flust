use std::{error::Error as StdError, sync::Arc};

use flutter_engine::builder::FlutterEngineBuilder;
use flutter_engine::{CreateError, FlutterEngine, RunError};
use flutter_runner_api::ApplicationAttributes;
use thiserror::Error;
use winit::dpi::PhysicalSize;
use winit::error::EventLoopError;
use winit::event_loop::EventLoopBuilder;
use winit::platform::wayland::WindowBuilderExtWayland;
use winit::window::WindowBuilder;

use crate::{FlutterWindow, WinitPlatformTaskHandler};

pub struct WinitApplication {
    window: Option<FlutterWindow>,
    engine: FlutterEngine,
}

impl WinitApplication {
    pub fn new(
        attributes: ApplicationAttributes,
    ) -> Result<WinitApplication, WinitApplicationBuildError> {
        let event_loop = EventLoopBuilder::with_user_event().build()?;

        let builder = WindowBuilder::new();
        let builder = attributes
            .title
            .map_or(builder.clone(), |title| builder.with_title(title));

        let builder = attributes
            .app_id
            .map_or(builder.clone(), |app_id| builder.with_name(app_id, ""));

        let builder = attributes.inner_size.map_or(builder.clone(), |size| {
            builder.with_inner_size(PhysicalSize::new(
                size.to_physical::<u32>(1.0).width,
                size.to_physical::<u32>(1.0).height,
            ))
        });

        let platform_task_handler =
            Arc::new(WinitPlatformTaskHandler::new(event_loop.create_proxy()));

        let engine = FlutterEngineBuilder::new()
            .with_platform_handler(platform_task_handler)
            .with_asset_path(attributes.assets_path)
            .with_icu_data_path(attributes.icu_data_path)
            .with_args(attributes.args)
            .build()?;

        let window = FlutterWindow::new(event_loop, engine.clone(), builder)?;

        Ok(WinitApplication {
            window: Some(window),
            engine,
        })
    }

    pub fn run(&mut self) -> Result<(), WinitApplicationRunError> {
        let Some(window) = self.window.take() else {
            return Err(RunError::InternalInconsistency.into());
        };

        self.engine.run()?;
        window.run()?;

        Ok(())
    }
}

#[derive(Error, Debug)]
pub enum WinitApplicationBuildError {
    #[error(transparent)]
    CreateEngineError(#[from] CreateError),

    #[error(transparent)]
    WindowBuildFailure(#[from] Box<dyn StdError>),

    #[error(transparent)]
    InvalidEventError(#[from] EventLoopError),
}

#[derive(Error, Debug)]
pub enum WinitApplicationRunError {
    #[error(transparent)]
    WinitStartEngineError(#[from] RunError),

    #[error(transparent)]
    WinitEventLoopError(#[from] EventLoopError),
}
