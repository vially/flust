use crate::context::{Context, ResourceContext};
use crate::egl::create_window_contexts;
use crate::handler::{
    WinitOpenGLHandler, WinitPlatformHandler, WinitTextInputHandler, WinitWindowHandler,
};
use flutter_engine::channel::Channel;
use flutter_engine::plugins::{Plugin, PluginRegistrar};
use flutter_engine::texture_registry::Texture;
use flutter_engine::{FlutterEngine, FlutterEngineWeakRef};
use flutter_plugins::isolate::IsolatePlugin;
use flutter_plugins::keyevent::KeyEventPlugin;
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
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use winit::dpi::PhysicalSize;
use winit::event_loop::EventLoop;
use winit::window::WindowBuilder;

pub enum FlutterEvent {
    WakePlatformThread,
    IsolateCreated,
}

pub struct FlutterWindow {
    context: Arc<Mutex<Context>>,
    resource_context: Arc<Mutex<ResourceContext>>,
    engine: FlutterEngineWeakRef,
    close: Arc<AtomicBool>,
    plugins: Rc<RwLock<PluginRegistrar>>,
}

impl FlutterWindow {
    pub fn new(
        event_loop: &EventLoop<FlutterEvent>,
        engine: FlutterEngine,
        window: WindowBuilder,
    ) -> Result<Self, Box<dyn Error>> {
        let (context, resource_context) = create_window_contexts(window, event_loop)?;
        let context = Arc::new(Mutex::new(context));
        let resource_context = Arc::new(Mutex::new(resource_context));

        #[allow(deprecated)]
        engine.replace_opengl_handler(Box::new(WinitOpenGLHandler::new(
            context.clone(),
            resource_context.clone(),
        )));

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

    pub fn context(&self) -> Arc<Mutex<Context>> {
        self.context.clone()
    }

    pub fn is_closing(&self) -> bool {
        self.close.load(Ordering::Relaxed)
    }

    pub fn resource_context(&self) -> Arc<Mutex<ResourceContext>> {
        self.resource_context.clone()
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
}

pub(crate) fn resize(engine: &FlutterEngine, context: &Arc<Mutex<Context>>) {
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
