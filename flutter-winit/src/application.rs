use std::{error::Error as StdError, sync::Arc};

use ashpd::desktop::settings::{ColorScheme, Settings};
use async_executor::LocalExecutor;
use flutter_engine::builder::FlutterEngineBuilder;
use flutter_engine::{CreateError, FlutterEngine, RunError};
use flutter_plugins::keyevent::{KeyAction, KeyActionType, KeyEventPlugin};
use flutter_plugins::localization::LocalizationPlugin;
use flutter_plugins::settings::{PlatformBrightness, SettingsPlugin};
use flutter_plugins::textinput::TextInputPlugin;
use flutter_runner_api::ApplicationAttributes;
use futures_lite::future;
use sys_locale::get_locale;
use thiserror::Error;
use winit::dpi::PhysicalSize;
use winit::error::EventLoopError;
use winit::event::{ElementState, Event, KeyEvent, MouseScrollDelta, Touch, WindowEvent};
use winit::event_loop::EventLoopBuilder;
use winit::event_loop::{ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::platform::wayland::WindowBuilderExtWayland;
use winit::window::WindowBuilder;

use crate::keyboard::raw_key;
use crate::pointer::Pointers;
use crate::window::{resize, FlutterEvent};
use crate::{FlutterWindow, WinitPlatformTaskHandler};

pub struct WinitApplication {
    event_loop: EventLoop<FlutterEvent>,
    window: FlutterWindow,
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

        let window = FlutterWindow::new(&event_loop, engine.clone(), builder)?;

        Ok(WinitApplication {
            event_loop,
            window,
            engine,
        })
    }

    pub fn run(self) -> Result<(), WinitApplicationRunError> {
        // Warning: The current logic does not support `custom_task_runners`.
        //
        // TODO: Start event loop *prior* to running the engine. See
        // `FlutterEngineRun` comment in `embedder.h` for additional context.
        self.engine.run()?;

        let window = &self.window;
        let context = window.context();

        resize(&self.engine, &context);

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

        let mut pointers = Pointers::new(self.engine.clone());
        Ok(self.event_loop.run(move |event, elwt| match event {
            Event::WindowEvent { event, .. } => {
                match event {
                    WindowEvent::CloseRequested => elwt.exit(),
                    WindowEvent::Resized(_) => resize(&self.engine, &context),
                    WindowEvent::ScaleFactorChanged { .. } => resize(&self.engine, &context),
                    WindowEvent::CursorEntered { device_id } => pointers.enter(device_id),
                    WindowEvent::CursorLeft { device_id } => pointers.leave(device_id),
                    WindowEvent::CursorMoved {
                        device_id,
                        position,
                        ..
                    } => {
                        pointers.moved(device_id, position.into());
                    }
                    WindowEvent::MouseInput {
                        device_id,
                        state,
                        button,
                        ..
                    } => {
                        pointers.input(device_id, state, button);
                    }
                    WindowEvent::MouseWheel {
                        device_id, delta, ..
                    } => {
                        let delta = match delta {
                            MouseScrollDelta::LineDelta(_, _) => (0.0, 0.0), // TODO
                            MouseScrollDelta::PixelDelta(position) => {
                                let (dx, dy): (f64, f64) = position.into();
                                (-dx, dy)
                            }
                        };
                        pointers.wheel(device_id, delta);
                    }
                    WindowEvent::Touch(Touch {
                        device_id,
                        phase,
                        location,
                        ..
                    }) => {
                        pointers.touch(device_id, phase, location.into());
                    }
                    WindowEvent::KeyboardInput {
                        event:
                            KeyEvent {
                                state, logical_key, ..
                            },
                        ..
                    } => {
                        let Some(raw_key) = raw_key(logical_key.clone()) else {
                            return;
                        };

                        // TODO(vially): Bring back modifiers
                        //let shift: u32 = modifiers.shift().into();
                        //let ctrl: u32 = modifiers.ctrl().into();
                        //let alt: u32 = modifiers.alt().into();
                        //let logo: u32 = modifiers.logo().into();
                        //let raw_modifiers = shift | ctrl << 1 | alt << 2 | logo << 3;
                        let raw_modifiers = 0;

                        match state {
                            ElementState::Pressed => {
                                window.with_plugin_mut(
                                    // TODO(vially): Fix text input logic to handle *all* named keys
                                    |text_input: &mut TextInputPlugin| match logical_key {
                                        Key::Named(key) => match key {
                                            NamedKey::Enter => {
                                                text_input.with_state(|state| {
                                                    state.add_characters("\n");
                                                });
                                                text_input.notify_changes();
                                            }
                                            NamedKey::Backspace => {
                                                text_input.with_state(|state| {
                                                    state.backspace();
                                                });
                                                text_input.notify_changes();
                                            }
                                            _ => {}
                                        },
                                        Key::Character(ch) => {
                                            text_input.with_state(|state| {
                                                state.add_characters(&ch.to_string());
                                            });
                                            text_input.notify_changes();
                                        }
                                        _ => {}
                                    },
                                );

                                window.with_plugin(|keyevent: &KeyEventPlugin| {
                                    keyevent.key_action(KeyAction {
                                        toolkit: "glfw".to_string(),
                                        key_code: raw_key as _,
                                        // TODO(vially): Fix scan code
                                        scan_code: 0,
                                        modifiers: raw_modifiers as _,
                                        keymap: "linux".to_string(),
                                        _type: KeyActionType::Keydown,
                                    });
                                });
                            }
                            ElementState::Released => {
                                window.with_plugin(|keyevent: &KeyEventPlugin| {
                                    keyevent.key_action(KeyAction {
                                        toolkit: "glfw".to_string(),
                                        key_code: raw_key as _,
                                        // TODO(vially): Fix scan code
                                        scan_code: 0,
                                        modifiers: raw_modifiers as _,
                                        keymap: "linux".to_string(),
                                        _type: KeyActionType::Keyup,
                                    });
                                });
                            }
                        }
                    }
                    _ => {}
                }
            }
            Event::LoopExiting => {
                self.engine.shutdown();
            }
            _ => {
                if window.is_closing() {
                    elwt.exit();
                    return;
                }

                let next_task_time = self.engine.execute_platform_tasks();
                let control_flow = next_task_time.map_or(ControlFlow::Wait, ControlFlow::WaitUntil);
                elwt.set_control_flow(control_flow)
            }
        })?)
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
