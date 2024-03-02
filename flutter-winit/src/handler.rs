use crate::window::FlutterEvent;
use copypasta::nop_clipboard::NopClipboardContext;
use copypasta::ClipboardProvider;
use flutter_engine::tasks::TaskRunnerHandler;
use flutter_engine_api::FlutterOpenGLHandler;
use flutter_glutin::context::{Context, ResourceContext};
use flutter_plugins::platform::{AppSwitcherDescription, MimeError, PlatformHandler};
use flutter_plugins::textinput::TextInputHandler;
use flutter_plugins::window::{PositionParams, WindowHandler};
use parking_lot::Mutex;
use std::error::Error;
use std::ffi::{c_void, CStr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use winit::event_loop::EventLoopProxy;
use winit::window::Window;

// TODO: Investigate removing mutex
pub struct WinitPlatformTaskHandler {
    proxy: Mutex<EventLoopProxy<FlutterEvent>>,
}

impl WinitPlatformTaskHandler {
    pub fn new(proxy: EventLoopProxy<FlutterEvent>) -> Self {
        Self {
            proxy: Mutex::new(proxy),
        }
    }
}

impl TaskRunnerHandler for WinitPlatformTaskHandler {
    fn wake(&self) {
        self.proxy
            .lock()
            .send_event(FlutterEvent::WakePlatformThread)
            .ok();
    }
}

pub struct WinitOpenGLHandler {
    context: Arc<std::sync::Mutex<Context>>,
    resource_context: Arc<std::sync::Mutex<ResourceContext>>,
}

impl WinitOpenGLHandler {
    pub fn new(
        context: Arc<std::sync::Mutex<Context>>,
        resource_context: Arc<std::sync::Mutex<ResourceContext>>,
    ) -> Self {
        Self {
            context,
            resource_context,
        }
    }
}

impl FlutterOpenGLHandler for WinitOpenGLHandler {
    fn swap_buffers(&self) -> bool {
        self.context.lock().unwrap().present()
    }

    fn make_current(&self) -> bool {
        self.context.lock().unwrap().make_current()
    }

    fn clear_current(&self) -> bool {
        self.context.lock().unwrap().make_not_current()
    }

    fn fbo_callback(&self) -> u32 {
        0
    }

    fn make_resource_current(&self) -> bool {
        self.resource_context.lock().unwrap().make_current()
    }

    fn gl_proc_resolver(&self, proc: &CStr) -> *mut c_void {
        self.context.lock().unwrap().get_proc_address(proc) as _
    }
}

pub struct WinitPlatformHandler {
    // TODO(vially): Bring back clipboard context implementation
    clipboard: NopClipboardContext,
    window: Arc<Mutex<Window>>,
}

impl WinitPlatformHandler {
    pub fn new(window: Arc<Mutex<Window>>) -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            clipboard: NopClipboardContext,
            window,
        })
    }
}

impl PlatformHandler for WinitPlatformHandler {
    fn set_application_switcher_description(&mut self, description: AppSwitcherDescription) {
        self.window.lock().set_title(&description.label);
    }

    fn set_clipboard_data(&mut self, text: String) {
        if let Err(err) = self.clipboard.set_contents(text) {
            log::error!("{}", err);
        }
    }

    fn get_clipboard_data(&mut self, mime: &str) -> Result<String, MimeError> {
        if mime != "text/plain" {
            return Err(MimeError);
        }
        let result = self.clipboard.get_contents();
        if let Err(err) = &result {
            log::error!("{}", err);
        }
        Ok(result.unwrap_or_default())
    }
}

pub struct WinitWindowHandler {
    window: Arc<Mutex<Window>>,
    maximized: bool,
    visible: bool,
    close: Arc<AtomicBool>,
}

impl WinitWindowHandler {
    pub fn new(window: Arc<Mutex<Window>>, close: Arc<AtomicBool>) -> Self {
        Self {
            window,
            maximized: false,
            visible: false,
            close,
        }
    }
}

impl WindowHandler for WinitWindowHandler {
    fn close(&mut self) {
        self.close.store(true, Ordering::Relaxed);
    }

    fn show(&mut self) {
        self.visible = true;
        self.window.lock().set_visible(self.visible);
    }

    fn hide(&mut self) {
        self.visible = false;
        self.window.lock().set_visible(self.visible);
    }

    fn is_visible(&mut self) -> bool {
        self.visible
    }

    fn maximize(&mut self) {
        self.maximized = true;
        self.window.lock().set_maximized(self.maximized);
    }

    fn restore(&mut self) {
        self.maximized = false;
        self.window.lock().set_maximized(self.maximized);
    }

    fn is_maximized(&mut self) -> bool {
        self.maximized
    }

    fn iconify(&mut self) {}

    fn is_iconified(&mut self) -> bool {
        false
    }

    fn set_pos(&mut self, _pos: PositionParams) {}

    fn get_pos(&mut self) -> PositionParams {
        PositionParams { x: 0.0, y: 0.0 }
    }

    fn start_drag(&mut self) {}

    fn end_drag(&mut self) {}
}

#[derive(Default)]
pub struct WinitTextInputHandler {}

impl TextInputHandler for WinitTextInputHandler {
    fn show(&mut self) {}

    fn hide(&mut self) {}
}
