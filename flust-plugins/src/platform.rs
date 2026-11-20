//! Plugin to work with clipboard and various system related functions.
//! It handles flutter/platform type message.
//!
//! It also takes part in Flutter's exit protocol, when it is given an
//! [`ExitHandler`]: Dart decides when the application exits. A request to
//! close the application from outside (the user closing its window) is
//! passed on to Dart with [`PlatformPlugin::request_app_exit`], and Dart can
//! cancel it. Dart can also ask for the application to exit itself
//! (`ServicesBinding.exitApplication`, `SystemNavigator.pop`).
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Weak};

use flust_engine::{
    channel::{MethodCallHandler, MethodChannel},
    codec::JSON_CODEC,
    plugins::Plugin,
    FlutterEngine,
};

use serde::{Deserialize, Serialize};

use flust_engine::channel::{MethodCall, MethodError};
use flust_engine::codec::Value;
use parking_lot::Mutex;
use tracing::{debug, warn};

pub const PLUGIN_NAME: &str = module_path!();
pub const CHANNEL_NAME: &str = "flutter/platform";

#[derive(Debug)]
pub struct MimeError;

impl std::fmt::Display for MimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Mime error")
    }
}

impl std::error::Error for MimeError {}

pub trait PlatformHandler {
    fn set_application_switcher_description(&mut self, description: AppSwitcherDescription);

    fn set_clipboard_data(&mut self, text: String);

    fn get_clipboard_data(&mut self, mime: &str) -> Result<String, MimeError>;
}

/// Ends the application, for the exit protocol of [`PlatformPlugin`].
pub trait ExitHandler {
    /// Ends the application with this exit code: the embedder leaves its
    /// event loop, shuts the engine down and exits. Called on the platform
    /// thread. Dart has had its say by then, if it could have one.
    fn exit_application(&mut self, exit_code: i32);
}

pub struct PlatformPlugin {
    handler: Arc<Mutex<dyn PlatformHandler + Send>>,
    exit: Rc<RefCell<ExitState>>,
}

impl PlatformPlugin {
    pub fn new(handler: Arc<Mutex<dyn PlatformHandler + Send>>) -> Self {
        Self {
            handler,
            exit: Default::default(),
        }
    }

    /// Takes part in the exit protocol, ending the application with this
    /// handler. Without one, Dart's requests to exit are answered as not
    /// implemented.
    pub fn with_exit_handler(self, exit_handler: Arc<Mutex<dyn ExitHandler + Send>>) -> Self {
        self.exit.borrow_mut().exit_handler = Some(exit_handler);
        self
    }

    /// Something outside of the application asks for it to exit (its window
    /// was asked to close). Dart is asked if it may (`System.requestAppExit`),
    /// and can cancel. The application exits right away if Dart can not be
    /// asked yet.
    pub fn request_app_exit(&self) {
        ExitState::request_app_exit(&self.exit, 0, None);
    }
}

#[derive(Default)]
struct ExitState {
    channel: Weak<MethodChannel>,
    exit_handler: Option<Arc<Mutex<dyn ExitHandler + Send>>>,
    /// Dart has said that it answers `System.requestAppExit`
    /// (`System.initializationComplete`).
    initialization_complete: bool,
}

impl ExitState {
    /// Asks Dart whether the application may exit, and exits with
    /// `exit_code` unless Dart cancels. `exit_call` is Dart's own call to
    /// exit, if that is where this comes from: it is answered with Dart's
    /// decision.
    fn request_app_exit(state: &RefCell<Self>, exit_code: i32, exit_call: Option<MethodCall>) {
        // Not borrowed while Dart runs: the engine runs it from within the
        // call that sends it a message.
        let (channel, exit_handler, initialization_complete) = {
            let state = state.borrow();
            (
                state.channel.upgrade(),
                state.exit_handler.clone(),
                state.initialization_complete,
            )
        };

        let Some(exit_handler) = exit_handler else {
            if let Some(call) = exit_call {
                call.not_implemented();
            }
            return;
        };

        let channel = channel.filter(|_| initialization_complete);
        let Some(channel) = channel else {
            debug!("exiting without asking Dart: it can not answer yet");
            exit(&exit_handler, exit_code, exit_call);
            return;
        };

        channel.invoke_method_with_result(
            "System.requestAppExit".into(),
            HashMap::from([("type", "cancelable")]),
            move |result: Result<Value, MethodError<Value>>| {
                let response = match &result {
                    Ok(Value::Map(map)) => map.get("response"),
                    _ => None,
                };
                match response {
                    Some(Value::String(response)) if response == "cancel" => {
                        debug!("Dart cancelled the exit");
                        if let Some(call) = exit_call {
                            call.success(exit_response("cancel"));
                        }
                    }
                    Some(Value::String(response)) if response == "exit" => {
                        exit(&exit_handler, exit_code, exit_call);
                    }
                    // Something went wrong: exit, as Flutter's own Linux
                    // embedder does.
                    _ => {
                        warn!("invalid response to System.requestAppExit, exiting");
                        exit(&exit_handler, exit_code, exit_call);
                    }
                }
            },
        );
    }
}

/// Answers Dart's call to exit, if there is one, and exits.
fn exit(
    exit_handler: &Mutex<dyn ExitHandler + Send>,
    exit_code: i32,
    exit_call: Option<MethodCall>,
) {
    if let Some(call) = exit_call {
        call.success(exit_response("exit"));
    }
    debug!("exiting with exit code {exit_code}");
    exit_handler.lock().exit_application(exit_code);
}

fn exit_response(response: &str) -> HashMap<&str, &str> {
    HashMap::from([("response", response)])
}

impl Plugin for PlatformPlugin {
    fn plugin_name() -> &'static str {
        PLUGIN_NAME
    }

    fn init(&mut self, engine: &FlutterEngine) {
        let channel = engine.register_channel(MethodChannel::new(
            CHANNEL_NAME,
            Handler {
                handler: self.handler.clone(),
                exit: self.exit.clone(),
            },
            &JSON_CODEC,
        ));
        self.exit.borrow_mut().channel = channel;
    }
}

struct Handler {
    handler: Arc<Mutex<dyn PlatformHandler + Send>>,
    exit: Rc<RefCell<ExitState>>,
}

impl Handler {
    fn exit_application(&mut self, call: MethodCall) {
        let Value::Map(args) = call.raw_args() else {
            return call.error("bad-arguments", "Invalid exit request", Value::Null);
        };
        let exit_code = match args.get("exitCode") {
            Some(Value::I32(code)) => *code,
            Some(Value::I64(code)) => *code as i32,
            _ => 0,
        };
        let required = match args.get("type") {
            Some(Value::String(exit_type)) if exit_type == "required" => true,
            Some(Value::String(exit_type)) if exit_type == "cancelable" => false,
            _ => return call.error("bad-arguments", "Invalid exit type", Value::Null),
        };

        if required {
            let exit_handler = self.exit.borrow().exit_handler.clone();
            match exit_handler {
                Some(exit_handler) => exit(&exit_handler, exit_code, Some(call)),
                None => call.not_implemented(),
            }
        } else {
            ExitState::request_app_exit(&self.exit, exit_code, Some(call));
        }
    }
}

impl MethodCallHandler for Handler {
    fn on_method_call(&mut self, call: MethodCall) {
        debug!(
            "got method call {} with args {:?}",
            call.method(),
            call.raw_args()
        );
        match call.method().as_str() {
            "SystemChrome.setApplicationSwitcherDescription" => {
                let args: AppSwitcherDescription = call.args();
                self.handler
                    .lock()
                    .set_application_switcher_description(args);
                call.success_empty()
            }
            "Clipboard.setData" => {
                if let Value::Map(v) = &call.args() {
                    if let Some(Value::String(text)) = &v.get("text") {
                        let text = text.clone();
                        self.handler.lock().set_clipboard_data(text);
                        return call.success_empty();
                    }
                }
                call.error("unknown-data", "Unknown data type", Value::Null)
            }
            "Clipboard.getData" => {
                if let Value::String(mime) = call.raw_args() {
                    match self.handler.lock().get_clipboard_data(mime) {
                        Ok(text) => call.success(ClipboardData { text }),
                        Err(_) => call.error("unknown-data", "Unknown data type", Value::Null),
                    }
                } else {
                    call.error("unknown-data", "Unknown data type", Value::Null)
                }
            }
            "System.exitApplication" => self.exit_application(call),
            "System.initializationComplete" => {
                self.exit.borrow_mut().initialization_complete = true;
                call.success_empty()
            }
            "SystemNavigator.pop" => {
                let exit_handler = self.exit.borrow().exit_handler.clone();
                match exit_handler {
                    Some(exit_handler) => {
                        call.success_empty();
                        exit(&exit_handler, 0, None);
                    }
                    None => call.not_implemented(),
                }
            }
            _ => call.not_implemented(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct ClipboardData {
    text: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSwitcherDescription {
    pub primary_color: i64,
    pub label: String,
}
