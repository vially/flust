//! This plugin is used by TextField to edit text and control caret movement.
//! It handles flutter/textinput type message.

use tracing::debug;
use std::sync::{Arc, RwLock, Weak};

use serde::{Deserialize, Serialize};

use flust_engine::codec::value::VecExt;

use flust_engine::{
    channel::{MethodCallHandler, MethodChannel},
    codec::JSON_CODEC,
    plugins::Plugin,
    FlutterEngine,
};

use self::text_editing_state::TextEditingState;
use flust_engine::channel::MethodCall;
use flust_engine::codec::Value;
use parking_lot::Mutex;

mod text_editing_state;
pub(crate) mod utils;

pub const PLUGIN_NAME: &str = module_path!();
pub const CHANNEL_NAME: &str = "flutter/textinput";

const MULTILINE_INPUT_TYPE: &str = "TextInputType.multiline";
const INPUT_ACTION_NEWLINE: &str = "TextInputAction.newline";

pub trait TextInputHandler {
    fn show(&mut self);

    fn hide(&mut self);
}

pub struct TextInputPlugin {
    channel: Weak<MethodChannel>,
    data: Arc<RwLock<Data>>,
    handler: Arc<Mutex<dyn TextInputHandler + Send>>,
}

struct Handler {
    data: Arc<RwLock<Data>>,
    handler: Arc<Mutex<dyn TextInputHandler + Send>>,
}

struct Data {
    client_id: Option<i64>,
    client_args: Option<SetClientArgsText>,
    editing_state: Option<TextEditingState>,
}

impl Plugin for TextInputPlugin {
    fn plugin_name() -> &'static str {
        PLUGIN_NAME
    }

    fn init(&mut self, engine: &FlutterEngine) {
        self.channel = engine.register_channel(MethodChannel::new(
            CHANNEL_NAME,
            Handler {
                data: self.data.clone(),
                handler: self.handler.clone(),
            },
            &JSON_CODEC,
        ));
    }
}

impl TextInputPlugin {
    pub fn new(handler: Arc<Mutex<dyn TextInputHandler + Send>>) -> Self {
        let data = Arc::new(RwLock::new(Data {
            client_id: None,
            client_args: None,
            editing_state: None,
        }));
        Self {
            channel: Weak::new(),
            handler,
            data,
        }
    }

    fn with_channel<F>(&self, f: F)
    where
        F: FnOnce(&MethodChannel),
    {
        if let Some(channel) = self.channel.upgrade() {
            f(&channel);
        }
    }

    pub fn with_state(&mut self, cbk: impl FnOnce(&mut TextEditingState)) {
        let mut data = self.data.write().unwrap();
        if let Some(state) = &mut data.editing_state {
            cbk(state);
        }
    }

    pub fn perform_action(&self, action: &str) {
        let data = self.data.read().unwrap();
        self.with_channel(|channel| {
            let mut args: Vec<Value> = Vec::new();
            args.push_as_value(data.client_id);
            args.push_as_value("TextInputAction.".to_owned() + action);
            channel.invoke_method("TextInputClient.performAction", args)
        });
    }

    pub fn notify_changes(&mut self) {
        let mut data = self.data.write().unwrap();
        let client_id = data.client_id;
        if let Some(state) = &mut (data.editing_state) {
            if let Some(channel) = self.channel.upgrade() {
                let mut args: Vec<Value> = Vec::new();
                args.push_as_value(client_id);
                args.push_as_value(state);
                channel.invoke_method("TextInputClient.updateEditingState", args)
            }
        };
    }

    // This implementation is based on the official Windows embedder implementation:
    // https://github.com/flutter/engine/blob/3.22.0/shell/platform/windows/text_input_plugin.cc#L473-L493
    pub fn enter_pressed(&mut self) {
        let mut data = self.data.write().unwrap();
        let client_id = data.client_id;

        let is_multiline_newline_action = data
            .client_args
            .as_ref()
            .map(|args| args.is_multiline_newline_action())
            .unwrap_or_default();

        if is_multiline_newline_action {
            if let Some(state) = &mut (data.editing_state) {
                state.add_characters("\n");

                if let Some(channel) = self.channel.upgrade() {
                    let mut args: Vec<Value> = Vec::new();
                    args.push_as_value(client_id);
                    args.push_as_value(state);
                    channel.invoke_method("TextInputClient.updateEditingState", args)
                }
            }
        }

        if let Some(input_action) = data
            .client_args
            .as_ref()
            .map(|args| args.input_action.clone())
        {
            self.with_channel(|channel| {
                let mut args: Vec<Value> = Vec::new();
                args.push_as_value(client_id);
                args.push_as_value(input_action);
                channel.invoke_method("TextInputClient.performAction", args)
            });
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
            "TextInput.setClient" => {
                let mut data = self.data.write().unwrap();
                let args: SetClientArgs = call.args();
                data.client_id = Some(args.0);
                data.client_args = Some(args.1);
                call.success_empty()
            }
            "TextInput.clearClient" => {
                let mut data = self.data.write().unwrap();
                data.client_id = None;
                data.editing_state.take();
                call.success_empty()
            }
            "TextInput.setEditingState" => {
                let mut data = self.data.write().unwrap();
                let state: TextEditingState = call.args();
                data.editing_state.replace(state);
                call.success_empty()
            }
            "TextInput.show" => {
                self.handler.lock().show();
                call.success_empty()
            }
            "TextInput.hide" => {
                self.handler.lock().hide();
                call.success_empty()
            }
            _ => call.not_implemented(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct SetClientArgs(i64, SetClientArgsText);

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetClientArgsText {
    autocorrect: bool,
    input_action: String,
    obscure_text: bool,
    keyboard_appearance: String,
    action_label: Option<String>,
    text_capitalization: String,
    input_type: SetClientArgsInputType,
}

impl SetClientArgsText {
    fn is_multiline_newline_action(&self) -> bool {
        self.input_type.name.as_str() == MULTILINE_INPUT_TYPE
            && self.input_action.as_str() == INPUT_ACTION_NEWLINE
    }
}

#[derive(Serialize, Deserialize)]
struct SetClientArgsInputType {
    signed: Option<bool>,
    name: String,
    decimal: Option<bool>,
}
