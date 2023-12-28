use crate::context::{Context, ResourceContext};
use crate::egl::create_window_contexts;
use crate::handler::{
    WinitOpenGLHandler, WinitPlatformHandler, WinitPlatformTaskHandler, WinitTextInputHandler,
    WinitWindowHandler,
};
use crate::keyboard::raw_key;
use crate::pointer::Pointers;
use flutter_engine::builder::FlutterEngineBuilder;
use flutter_engine::channel::Channel;
use flutter_engine::plugins::{Plugin, PluginRegistrar};
use flutter_engine::texture_registry::Texture;
use flutter_engine::{FlutterEngine, RunError};
use flutter_plugins::isolate::IsolatePlugin;
use flutter_plugins::keyevent::{KeyAction, KeyActionType, KeyEventPlugin};
use flutter_plugins::lifecycle::LifecyclePlugin;
use flutter_plugins::localization::LocalizationPlugin;
use flutter_plugins::navigation::NavigationPlugin;
use flutter_plugins::platform::PlatformPlugin;
use flutter_plugins::settings::SettingsPlugin;
use flutter_plugins::system::SystemPlugin;
use flutter_plugins::textinput::TextInputPlugin;
use flutter_plugins::window::WindowPlugin;
use parking_lot::{Mutex, RwLock};
use std::error::Error;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use sys_locale::get_locale;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, Event, KeyEvent, MouseScrollDelta, Touch, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop, EventLoopBuilder};
use winit::keyboard::{Key, NamedKey};
use winit::window::WindowBuilder;

pub enum FlutterEvent {
    WakePlatformThread,
    IsolateCreated,
}

pub struct FlutterWindow {
    event_loop: EventLoop<FlutterEvent>,
    context: Arc<Mutex<Context>>,
    resource_context: Arc<Mutex<ResourceContext>>,
    engine: FlutterEngine,
    close: Arc<AtomicBool>,
    plugins: Rc<RwLock<PluginRegistrar>>,
}

impl FlutterWindow {
    pub fn new(
        window: WindowBuilder,
        assets_path: PathBuf,
        icu_data_path: PathBuf,
        arguments: Vec<String>,
    ) -> Result<Self, Box<dyn Error>> {
        let event_loop = EventLoopBuilder::with_user_event().build()?;
        let proxy = event_loop.create_proxy();

        let (context, resource_context) = create_window_contexts(window, &event_loop)?;
        let context = Arc::new(Mutex::new(context));
        let resource_context = Arc::new(Mutex::new(resource_context));

        let platform_task_handler = Arc::new(WinitPlatformTaskHandler::new(proxy));

        let opengl_handler = WinitOpenGLHandler::new(context.clone(), resource_context.clone());

        let engine = FlutterEngineBuilder::new()
            .with_platform_handler(platform_task_handler)
            .with_opengl(opengl_handler)
            .with_asset_path(assets_path)
            .with_icu_data_path(icu_data_path)
            .with_args(arguments)
            .build()
            .expect("Failed to create engine");

        let proxy = event_loop.create_proxy();
        let isolate_cb = move || {
            proxy.send_event(FlutterEvent::IsolateCreated).ok();
        };
        let platform_handler = Arc::new(Mutex::new(WinitPlatformHandler::new(context.clone())?));
        let close = Arc::new(AtomicBool::new(false));
        let window_handler = Arc::new(Mutex::new(WinitWindowHandler::new(
            context.clone(),
            close.clone(),
        )));
        let textinput_handler = Arc::new(Mutex::new(WinitTextInputHandler::default()));

        let mut plugins = PluginRegistrar::new();
        plugins.add_plugin(&engine, IsolatePlugin::new(isolate_cb));
        plugins.add_plugin(&engine, KeyEventPlugin::default());
        plugins.add_plugin(&engine, LifecyclePlugin::default());
        plugins.add_plugin(&engine, LocalizationPlugin::default());
        plugins.add_plugin(&engine, NavigationPlugin::default());
        plugins.add_plugin(&engine, PlatformPlugin::new(platform_handler));
        plugins.add_plugin(&engine, SettingsPlugin::default());
        plugins.add_plugin(&engine, SystemPlugin::default());
        plugins.add_plugin(&engine, TextInputPlugin::new(textinput_handler));
        plugins.add_plugin(&engine, WindowPlugin::new(window_handler));

        Ok(Self {
            event_loop,
            context,
            resource_context,
            engine,
            close,
            plugins: Rc::new(RwLock::new(plugins)),
        })
    }

    // TODO(vially): bring back `with_resource_context` method
    //pub fn with_resource_context(self) -> Result<Self, Box<dyn Error>> {
    //    {
    //        let window = WindowBuilder::new().with_visible(false);
    //        let context = self.context.lock();
    //        let resource_context = ContextBuilder::new()
    //            .with_shared_lists(context.context().unwrap())
    //            .build_windowed(window, &self.event_loop)?;

    //        let resource_context = unsafe { resource_context.make_current().unwrap() };
    //        gl::load_with(|s| resource_context.get_proc_address(s));
    //        let resource_context = unsafe { resource_context.make_not_current().unwrap() };

    //        let mut guard = self.resource_context.lock();
    //        *guard = Context::from_context(resource_context);
    //    }
    //    Ok(self)
    //}

    pub fn engine(&self) -> FlutterEngine {
        self.engine.clone()
    }

    pub fn context(&self) -> Arc<Mutex<Context>> {
        self.context.clone()
    }

    pub fn resource_context(&self) -> Arc<Mutex<ResourceContext>> {
        self.resource_context.clone()
    }

    pub fn create_texture(&self) -> Texture {
        self.engine.create_texture()
    }

    pub fn add_plugin<P>(&self, plugin: P) -> &Self
    where
        P: Plugin + 'static,
    {
        self.plugins.write().add_plugin(&self.engine, plugin);
        self
    }

    pub fn with_plugin<F, P>(&self, f: F)
    where
        F: FnOnce(&P),
        P: Plugin + 'static,
    {
        self.plugins.read().with_plugin(f)
    }

    pub fn with_plugin_mut<F, P>(&self, f: F)
    where
        F: FnOnce(&mut P),
        P: Plugin + 'static,
    {
        self.plugins.write().with_plugin_mut(f)
    }

    pub fn remove_channel(&self, channel_name: &str) -> Option<Arc<dyn Channel>> {
        self.engine.remove_channel(channel_name)
    }

    pub fn with_channel<F>(&self, channel_name: &str, f: F)
    where
        F: FnMut(&dyn Channel),
    {
        self.engine.with_channel(channel_name, f)
    }

    pub fn start_engine(&self) -> Result<(), RunError> {
        self.engine.run()
    }

    pub fn run(self) -> Result<(), winit::error::EventLoopError> {
        let engine = self.engine.clone();
        let context = self.context.clone();
        let plugins = self.plugins.clone();
        let close = self.close.clone();

        resize(&engine, &context);

        self.with_plugin(|localization: &LocalizationPlugin| {
            let locale = get_locale().unwrap_or_else(|| String::from("en-US"));
            localization.send_locale(locale);
        });

        let mut pointers = Pointers::new(engine.clone());
        self.event_loop.run(move |event, elwt| match event {
            Event::WindowEvent { event, .. } => {
                match event {
                    WindowEvent::CloseRequested => elwt.exit(),
                    WindowEvent::Resized(_) => resize(&engine, &context),
                    WindowEvent::ScaleFactorChanged { .. } => resize(&engine, &context),
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
                                plugins.write().with_plugin_mut(
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

                                plugins
                                    .write()
                                    .with_plugin_mut(|keyevent: &mut KeyEventPlugin| {
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
                                plugins
                                    .write()
                                    .with_plugin_mut(|keyevent: &mut KeyEventPlugin| {
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
                engine.shutdown();
            }
            _ => {
                if close.load(Ordering::Relaxed) {
                    elwt.exit();
                    return;
                }

                let next_task_time = engine.execute_platform_tasks();

                if let Some(next_task_time) = next_task_time {
                    elwt.set_control_flow(ControlFlow::WaitUntil(next_task_time))
                } else {
                    elwt.set_control_flow(ControlFlow::Wait)
                }
            }
        })
    }
}

fn resize(engine: &FlutterEngine, context: &Arc<Mutex<Context>>) {
    let mut context = context.lock();
    let dpi = context.hidpi_factor();
    let size = context.size();
    log::trace!(
        "resize width: {} height: {} scale {}",
        size.width,
        size.height,
        dpi
    );
    let context_size = PhysicalSize::new(
        NonZeroU32::new(size.width).expect("Resize width needs to be higher than 0"),
        NonZeroU32::new(size.height).expect("Resize height needs to be higher than 0"),
    );
    context.resize(context_size);
    engine.send_window_metrics_event(size.width as usize, size.height as usize, dpi);
}
