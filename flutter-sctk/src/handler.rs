use flutter_engine::tasks::TaskRunnerHandler;
use flutter_plugins::platform::{AppSwitcherDescription, MimeError, PlatformHandler};
use log::error;
use smithay_client_toolkit::reexports::{
    calloop::LoopSignal, protocols::xdg::shell::client::xdg_toplevel::XdgToplevel,
};

pub struct SctkPlatformTaskHandler {
    signal: LoopSignal,
}

impl SctkPlatformTaskHandler {
    pub fn new(signal: LoopSignal) -> Self {
        Self { signal }
    }
}

impl TaskRunnerHandler for SctkPlatformTaskHandler {
    fn wake(&self) {
        self.signal.wakeup();
    }
}

// TODO(multi-view): Add support for multi-view once the `flutter/platform`
// plugin supports it.
pub struct SctkPlatformHandler {
    implicit_xdg_toplevel: XdgToplevel,
}

impl SctkPlatformHandler {
    pub fn new(xdg_toplevel: XdgToplevel) -> Self {
        Self {
            implicit_xdg_toplevel: xdg_toplevel,
        }
    }
}

impl PlatformHandler for SctkPlatformHandler {
    fn set_application_switcher_description(&mut self, description: AppSwitcherDescription) {
        self.implicit_xdg_toplevel.set_title(description.label);
    }

    fn set_clipboard_data(&mut self, _text: String) {
        error!(
            "Attempting to set the contents of the clipboard, which hasn't yet been implemented \
             on this platform."
        );
    }

    fn get_clipboard_data(&mut self, _mime: &str) -> Result<String, MimeError> {
        error!(
            "Attempting to get the contents of the clipboard, which hasn't yet been implemented \
             on this platform."
        );
        Ok("".to_string())
    }
}
