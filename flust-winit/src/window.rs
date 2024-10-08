use crate::egl::create_window_contexts;
use crate::handler::{WinitPlatformHandler, WinitTextInputHandler, WinitWindowHandler};
use crate::keyboard::raw_key;
use crate::pointer::Pointers;
use dpi::PhysicalSize;
use flust_engine::channel::Channel;
use flust_engine::ffi::FlutterViewId;
use flust_engine::plugins::{Plugin, PluginRegistrar};
use flust_engine::texture_registry::Texture;
use flust_engine::{FlutterEngine, FlutterEngineWeakRef};
use flust_glutin::context::{Context, ResourceContext};
use flust_glutin::handler::GlutinOpenGLHandler;
use flust_plugins::isolate::IsolatePlugin;
use flust_plugins::keyevent::{KeyAction, KeyActionType, KeyEventPlugin};
use flust_plugins::lifecycle::LifecyclePlugin;
use flust_plugins::localization::LocalizationPlugin;
use flust_plugins::navigation::NavigationPlugin;
use flust_plugins::platform::PlatformPlugin;
use flust_plugins::settings::SettingsPlugin;
use flust_plugins::system::SystemPlugin;
use flust_plugins::textinput::TextInputPlugin;
use flust_plugins::window::WindowPlugin;
use flust_engine_sys::FlutterEngineDisplayId;
use parking_lot::{Mutex, RwLock};
use std::error::Error;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::trace;
use winit::event::{ElementState, KeyEvent, MouseScrollDelta, Touch, WindowEvent};
use winit::event_loop::{EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};

pub enum FlutterEvent {
    WakePlatformThread,
    IsolateCreated,
    WindowCloseRequested(WindowId),
}

pub struct FlutterWindow {
    view_id: FlutterViewId,
    event_loop: EventLoopProxy<FlutterEvent>,
    window: Arc<Mutex<Window>>,
    context: Arc<std::sync::Mutex<Context>>,
    resource_context: Arc<std::sync::Mutex<ResourceContext>>,
    engine: FlutterEngineWeakRef,
    close: Arc<AtomicBool>,
    plugins: Rc<RwLock<PluginRegistrar>>,
}

impl FlutterWindow {
    pub fn new(
        view_id: FlutterViewId,
        event_loop: &EventLoop<FlutterEvent>,
        engine: FlutterEngine,
        attributes: WindowAttributes,
    ) -> Result<Self, Box<dyn Error>> {
        let (window, context, resource_context) = create_window_contexts(attributes, event_loop)?;
        let context = Arc::new(std::sync::Mutex::new(context));
        let resource_context = Arc::new(std::sync::Mutex::new(resource_context));
        let window = Arc::new(Mutex::new(window));

        let proxy = event_loop.create_proxy();
        let isolate_cb = move || {
            proxy.send_event(FlutterEvent::IsolateCreated).ok();
        };
        let platform_handler = Arc::new(Mutex::new(WinitPlatformHandler::new(window.clone())?));
        let close = Arc::new(AtomicBool::new(false));
        let window_handler = Arc::new(Mutex::new(WinitWindowHandler::new(
            window.clone(),
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
            view_id,
            event_loop: event_loop.create_proxy(),
            window,
            context,
            resource_context,
            engine: engine.downgrade(),
            close,
            plugins: Rc::new(RwLock::new(plugins)),
        })
    }

    pub fn engine(&self) -> FlutterEngineWeakRef {
        self.engine.clone()
    }

    pub fn context(&self) -> Arc<std::sync::Mutex<Context>> {
        self.context.clone()
    }

    pub fn view_id(&self) -> FlutterViewId {
        self.view_id
    }

    pub fn window(&self) -> Arc<Mutex<Window>> {
        self.window.clone()
    }

    pub fn window_id(&self) -> WindowId {
        self.window.lock().id()
    }

    pub fn is_closing(&self) -> bool {
        self.close.load(Ordering::Relaxed)
    }

    pub fn resource_context(&self) -> Arc<std::sync::Mutex<ResourceContext>> {
        self.resource_context.clone()
    }

    pub fn create_opengl_handler(&self) -> GlutinOpenGLHandler {
        GlutinOpenGLHandler::new(self.context.clone(), self.resource_context.clone())
    }

    pub fn create_texture(&self) -> Option<Texture> {
        let engine = self.engine.upgrade()?;
        Some(engine.create_texture())
    }

    pub fn add_plugin<P>(&self, plugin: P) -> &Self
    where
        P: Plugin + 'static,
    {
        if let Some(engine) = self.engine.upgrade() {
            self.plugins.write().add_plugin(&engine, plugin);
        }
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
        self.engine.upgrade()?.remove_channel(channel_name)
    }

    pub fn with_channel<F>(&self, channel_name: &str, f: F)
    where
        F: FnMut(&dyn Channel),
    {
        if let Some(engine) = self.engine.upgrade() {
            engine.with_channel(channel_name, f)
        }
    }

    pub fn handle_event(&self, event: WindowEvent, pointers: &mut Pointers) {
        let engine = self.engine.upgrade().unwrap();
        match event {
            WindowEvent::CloseRequested => {
                let _ = self
                    .event_loop
                    .send_event(FlutterEvent::WindowCloseRequested(self.window_id()));
            }
            WindowEvent::Resized(_) => {
                resize(self.view_id, &engine, &self.context, &self.window, 0)
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                resize(self.view_id, &engine, &self.context, &self.window, 0)
            }
            WindowEvent::CursorEntered { device_id } => pointers.enter(self.view_id, device_id),
            WindowEvent::CursorLeft { device_id } => pointers.leave(self.view_id, device_id),
            WindowEvent::CursorMoved {
                device_id,
                position,
                ..
            } => {
                pointers.moved(self.view_id, device_id, position.into());
            }
            WindowEvent::MouseInput {
                device_id,
                state,
                button,
                ..
            } => {
                pointers.input(self.view_id, device_id, state, button);
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
                pointers.wheel(self.view_id, device_id, delta);
            }
            WindowEvent::Touch(Touch {
                device_id,
                phase,
                location,
                ..
            }) => {
                pointers.touch(self.view_id, device_id, phase, location.into());
            }
            WindowEvent::KeyboardInput {
                event: KeyEvent {
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
                        self.with_plugin_mut(
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

                        self.with_plugin(|keyevent: &KeyEventPlugin| {
                            keyevent.key_action(KeyAction {
                                toolkit: "glfw".to_string(),
                                key_code: raw_key as _,
                                specified_logical_key: 0,
                                unicode_scalar_values: 0,
                                // TODO(vially): Fix scan code
                                scan_code: 0,
                                modifiers: raw_modifiers as _,
                                keymap: "linux".to_string(),
                                _type: KeyActionType::Keydown,
                            });
                        });
                    }
                    ElementState::Released => {
                        self.with_plugin(|keyevent: &KeyEventPlugin| {
                            keyevent.key_action(KeyAction {
                                toolkit: "glfw".to_string(),
                                key_code: raw_key as _,
                                specified_logical_key: 0,
                                unicode_scalar_values: 0,
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
        };
    }
}

pub(crate) fn resize(
    view_id: FlutterViewId,
    engine: &FlutterEngine,
    context: &Arc<std::sync::Mutex<Context>>,
    window: &Arc<Mutex<Window>>,
    display_id: FlutterEngineDisplayId,
) {
    let (dpi, size) = {
        let window = window.lock();
        (window.scale_factor(), window.inner_size())
    };
    trace!(
        "resize width: {} height: {} scale {}",
        size.width,
        size.height,
        dpi
    );
    let context_size = PhysicalSize::new(
        NonZeroU32::new(size.width).expect("Resize width needs to be higher than 0"),
        NonZeroU32::new(size.height).expect("Resize height needs to be higher than 0"),
    );
    context.lock().unwrap().resize(context_size);
    engine.send_window_metrics_event(
        view_id,
        size.width as usize,
        size.height as usize,
        dpi,
        display_id,
    );
}
