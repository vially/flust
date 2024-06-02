use std::{
    collections::HashMap,
    fmt::Debug,
    sync::{Arc, Weak},
};

use log::debug;
use parking_lot::Mutex;

use flutter_engine::{
    channel::{MethodCall, MethodCallHandler, MethodChannel},
    codec::STANDARD_CODEC,
    ffi::{FlutterLogicalKey, FlutterPhysicalKey},
    plugins::Plugin,
    FlutterEngine,
};

use flutter_engine::codec::Value;

pub const PLUGIN_NAME: &str = module_path!();
pub const CHANNEL_NAME: &str = "flutter/keyboard";

pub struct KeyboardPlugin {
    channel: Weak<MethodChannel>,
    handler: Arc<Mutex<dyn KeyboardStateHandler + Send>>,
}

impl Plugin for KeyboardPlugin {
    fn plugin_name() -> &'static str {
        PLUGIN_NAME
    }

    fn init(&mut self, engine: &FlutterEngine) {
        self.channel = engine.register_channel(MethodChannel::new(
            CHANNEL_NAME,
            Handler {
                handler: self.handler.clone(),
            },
            &STANDARD_CODEC,
        ));
    }
}

impl KeyboardPlugin {
    pub fn new(handler: Arc<Mutex<dyn KeyboardStateHandler + Send>>) -> Self {
        Self {
            channel: Default::default(),
            handler,
        }
    }
}

#[derive(Debug)]
pub struct KeyboardStateError;

impl std::fmt::Display for KeyboardStateError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Keyboard state error")
    }
}

impl std::error::Error for KeyboardStateError {}

pub trait KeyboardStateHandler {
    fn get_keyboard_state(
        &self,
    ) -> Result<HashMap<FlutterPhysicalKey, FlutterLogicalKey>, KeyboardStateError>;
}

struct Handler {
    handler: Arc<Mutex<dyn KeyboardStateHandler + Send>>,
}

impl MethodCallHandler for Handler {
    fn on_method_call(&mut self, call: MethodCall) {
        debug!(
            "got method call {} with args {:?}",
            call.method(),
            call.raw_args()
        );
        match call.method().as_str() {
            "getKeyboardState" => {
                match self.handler.lock().get_keyboard_state() {
                    Ok(state) => {
                        let state: HashMap<u64, u64> = state
                            .into_iter()
                            .map(|(physical, logical)| (physical.raw(), logical.raw()))
                            .collect();

                        call.success(state)
                    }
                    Err(error) => call.error(
                        "Get keyboard state failure",
                        format!("{}", error),
                        Value::Null,
                    ),
                };
            }
            _ => call.not_implemented(),
        }
    }
}
