use std::sync::Weak;

use serde::{Deserialize, Serialize};

use flust_engine::{
    channel::{MessageChannel, MessageHandler},
    codec::JSON_CODEC,
    plugins::Plugin,
    FlutterEngine,
};

use flust_engine::channel::Message;
use flust_engine::codec::Value;

pub const PLUGIN_NAME: &str = module_path!();
pub const CHANNEL_NAME: &str = "flutter/keyevent";

#[derive(Default)]
pub struct KeyEventPlugin {
    channel: Weak<MessageChannel>,
}

struct Handler;

impl Plugin for KeyEventPlugin {
    fn plugin_name() -> &'static str {
        PLUGIN_NAME
    }

    fn init(&mut self, engine: &FlutterEngine) {
        self.channel =
            engine.register_channel(MessageChannel::new(CHANNEL_NAME, Handler, &JSON_CODEC));
    }
}

impl KeyEventPlugin {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Serialize, Deserialize)]
pub struct KeyAction {
    pub toolkit: String,
    #[serde(rename = "keyCode")]
    pub key_code: i32,
    #[serde(rename = "scanCode")]
    pub scan_code: i32,
    pub modifiers: i32,
    #[serde(rename = "specifiedLogicalKey")]
    pub specified_logical_key: i64,
    #[serde(rename = "unicodeScalarValues")]
    pub unicode_scalar_values: i64,
    pub keymap: String,
    #[serde(rename = "type")]
    pub _type: KeyActionType,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeyActionType {
    Keydown,
    Keyup,
}

impl KeyEventPlugin {
    fn with_channel<F>(&self, f: F)
    where
        F: FnOnce(&MessageChannel),
    {
        if let Some(channel) = self.channel.upgrade() {
            f(&channel);
        }
    }

    pub fn key_action(&self, action: KeyAction) {
        self.with_channel(|channel| {
            channel.send(action);
        });
    }
}

impl MessageHandler for Handler {
    fn on_message(&mut self, msg: Message) {
        msg.respond(Value::Null)
    }
}
