use ashpd::desktop::settings::{ColorScheme, Settings};
use async_executor::LocalExecutor;
use flutter_engine::builder::FlutterEngineBuilder;
use flutter_engine::{CreateError, FlutterEngine, RunError};
use flutter_plugins::localization::LocalizationPlugin;
use flutter_plugins::settings::{PlatformBrightness, SettingsPlugin};
use flutter_runner_api::ApplicationAttributes;
use futures_lite::future;
use std::sync::Arc;
use sys_locale::get_locale;
use thiserror::Error;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::error::EventLoopError;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::window::{WindowAttributes, WindowId};

use crate::pointer::Pointers;
use crate::view::WinitControllerError;
use crate::window::{resize, FlutterEvent};
use crate::{FlutterViewWinit, WinitPlatformTaskHandler};

pub struct WinitApplication {
    event_loop: EventLoop<FlutterEvent>,
    state: WinitApplicationState,
}

pub struct WinitApplicationState {
    implicit_view: FlutterViewWinit,
    engine: FlutterEngine,
    pointers: Pointers,
}

impl WinitApplication {
    pub fn new(
        attributes: ApplicationAttributes,
    ) -> Result<WinitApplication, WinitApplicationBuildError> {
        let event_loop = EventLoop::with_user_event().build()?;

        let window_attributes = WinitWindowAttributes::from(attributes.clone()).0;

        let platform_task_handler =
            Arc::new(WinitPlatformTaskHandler::new(event_loop.create_proxy()));

        let engine = FlutterEngineBuilder::new()
            .with_platform_handler(platform_task_handler)
            .with_asset_path(attributes.assets_path)
            .with_icu_data_path(attributes.icu_data_path)
            .with_persistent_cache_path(attributes.persistent_cache_path.clone())
            .with_args(attributes.args)
            .build()?;

        let implicit_view =
            FlutterViewWinit::new_implicit(&event_loop, engine.clone(), window_attributes)?;

        let pointers = Pointers::new(engine.clone());

        engine.add_view(implicit_view.create_flutter_view());

        let state = WinitApplicationState {
            implicit_view,
            engine,
            pointers,
        };

        Ok(WinitApplication { event_loop, state })
    }

    pub fn run(self) -> Result<(), WinitApplicationRunError> {
        let mut state = self.state;

        // Warning: The current logic does not support `custom_task_runners`.
        //
        // TODO: Start event loop *prior* to running the engine. See
        // `FlutterEngineRun` comment in `embedder.h` for additional context.
        state.engine.run()?;

        let window = state.implicit_view.window();
        let context = window.context();

        resize(
            window.view_id(),
            &state.engine,
            &context,
            &window.window(),
            0,
        );

        window.with_plugin(|localization: &LocalizationPlugin| {
            let locale = get_locale().unwrap_or_else(|| String::from("en-US"));
            localization.send_locale(locale);
        });

        // TODO: Add support for monitoring `PlatformBrightness` changes and disable
        // this logic on non-Linux platforms.
        window.with_plugin(|settings: &SettingsPlugin| {
            let color_scheme = future::block_on(
                LocalExecutor::new().run(async { Settings::new().await?.color_scheme().await }),
            )
            .unwrap_or(ColorScheme::NoPreference);

            let platform_brightness = match color_scheme {
                ColorScheme::PreferDark => PlatformBrightness::Dark,
                ColorScheme::PreferLight => PlatformBrightness::Light,
                ColorScheme::NoPreference => PlatformBrightness::Light,
            };

            settings
                .start_message()
                .set_platform_brightness(platform_brightness)
                .set_use_24_hour_format(true)
                .set_text_scale_factor(1.0)
                .send();
        });

        Ok(self.event_loop.run_app(&mut state)?)
    }
}

impl ApplicationHandler<FlutterEvent> for WinitApplicationState {
    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        self.implicit_view
            .window()
            .handle_event(event, &mut self.pointers);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: FlutterEvent) {
        match event {
            FlutterEvent::WindowCloseRequested(_) => event_loop.exit(),
            FlutterEvent::WakePlatformThread => {} // no-op
            FlutterEvent::IsolateCreated => {}     // no-op
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.implicit_view.window().is_closing() {
            event_loop.exit();
            return;
        }

        let next_task_time = self.engine.execute_platform_tasks();
        let control_flow = next_task_time.map_or(ControlFlow::Wait, ControlFlow::WaitUntil);
        event_loop.set_control_flow(control_flow)
    }

    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {
        // not needed for current implementation
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.engine.shutdown()
    }
}

#[derive(Error, Debug)]
pub enum WinitApplicationBuildError {
    #[error(transparent)]
    CreateEngineError(#[from] CreateError),

    #[error(transparent)]
    WindowBuildFailure(#[from] WinitControllerError),

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

struct WinitWindowAttributes(WindowAttributes);

impl From<ApplicationAttributes> for WinitWindowAttributes {
    fn from(value: ApplicationAttributes) -> Self {
        let mut attributes =
            WindowAttributes::default().with_title(value.title.unwrap_or_default());

        attributes.inner_size = value.inner_size.as_ref().map(|size| {
            PhysicalSize::new(
                size.to_physical::<u32>(1.0).width,
                size.to_physical::<u32>(1.0).height,
            )
            .into()
        });

        let attributes = value.app_id.map_or(attributes.clone(), |app_id| {
            attributes.with_name(app_id, "")
        });

        Self(attributes)
    }
}
