//! Plugin for what the framework wants an assistive technology to do apart
//! from reading the semantics tree, such as announcing a message.
//! It handles flutter/accessibility type message.

use std::sync::{Arc, Weak};

use parking_lot::Mutex;
use serde::Deserialize;
use tracing::{debug, warn};

use flust_engine::{
    channel::{Message, MessageChannel, MessageHandler},
    codec::{value::from_value, Value, STANDARD_CODEC},
    plugins::Plugin,
    FlutterEngine,
};

pub const PLUGIN_NAME: &str = module_path!();
pub const CHANNEL_NAME: &str = "flutter/accessibility";

/// How much an announcement may get in the way of what an assistive
/// technology is saying.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Assertiveness {
    /// It can wait until there is nothing else to say.
    #[default]
    Polite,
    /// It interrupts.
    Assertive,
}

/// A message for an assistive technology to announce, from
/// `SemanticsService.sendAnnouncement`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announcement {
    /// The view the announcement is for.
    pub view_id: i64,
    pub message: String,
    pub assertiveness: Assertiveness,
}

pub trait AccessibilityHandler {
    fn announce(&mut self, announcement: Announcement);
}

pub struct AccessibilityPlugin {
    channel: Weak<MessageChannel>,
    handler: Arc<Mutex<dyn AccessibilityHandler + Send>>,
}

impl AccessibilityPlugin {
    pub fn new(handler: Arc<Mutex<dyn AccessibilityHandler + Send>>) -> Self {
        Self {
            channel: Weak::new(),
            handler,
        }
    }
}

impl Plugin for AccessibilityPlugin {
    fn plugin_name() -> &'static str {
        PLUGIN_NAME
    }

    fn init(&mut self, engine: &FlutterEngine) {
        self.channel = engine.register_channel(MessageChannel::new(
            CHANNEL_NAME,
            Handler {
                handler: self.handler.clone(),
            },
            &STANDARD_CODEC,
        ));
    }
}

#[derive(Deserialize)]
struct Event {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    data: EventData,
}

/// The fields of all kinds of events. An event has the ones of its kind.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct EventData {
    view_id: Option<i64>,
    message: Option<String>,
    assertiveness: Option<i64>,
}

impl Event {
    fn announcement(self) -> Option<Announcement> {
        // Matches `Assertiveness` in Flutter's `semantics_event.dart`.
        let assertiveness = match self.data.assertiveness {
            Some(1) => Assertiveness::Assertive,
            _ => Assertiveness::Polite,
        };

        Some(Announcement {
            view_id: self.data.view_id?,
            message: self.data.message?,
            assertiveness,
        })
    }
}

struct Handler {
    handler: Arc<Mutex<dyn AccessibilityHandler + Send>>,
}

impl MessageHandler for Handler {
    fn on_message(&mut self, msg: Message) {
        let value: Value = msg.value();

        match from_value::<Event>(&value) {
            Ok(event) => match event.kind.as_str() {
                "announce" => match event.announcement() {
                    Some(announcement) => self.handler.lock().announce(announcement),
                    None => warn!("accessibility announcement without a view or a message"),
                },
                // What a tooltip, a tap, a long press and a change of focus
                // are announced with on other platforms. There is nothing
                // to do for them here, as in Flutter's Linux embedder.
                kind => debug!("ignoring accessibility event: {kind}"),
            },
            Err(err) => warn!("invalid accessibility event: {err}"),
        }

        if msg.can_respond() {
            msg.respond(Value::Null);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn event(kind: &str, data: Vec<(&str, Value)>) -> Event {
        let data: HashMap<String, Value> = data
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect();
        let value = Value::Map(HashMap::from([
            ("type".to_owned(), Value::String(kind.to_owned())),
            ("data".to_owned(), Value::Map(data)),
        ]));
        from_value(&value).unwrap()
    }

    #[test]
    fn announcement() {
        let event = event(
            "announce",
            vec![
                ("viewId", Value::I32(0)),
                ("message", Value::String("Saved".into())),
                ("textDirection", Value::I32(1)),
                ("assertiveness", Value::I32(1)),
            ],
        );

        assert_eq!(event.kind, "announce");
        assert_eq!(
            event.announcement(),
            Some(Announcement {
                view_id: 0,
                message: "Saved".into(),
                assertiveness: Assertiveness::Assertive,
            })
        );
    }

    #[test]
    fn an_announcement_is_polite_unless_it_says_otherwise() {
        let event = event(
            "announce",
            vec![
                ("viewId", Value::I32(2)),
                ("message", Value::String("Saved".into())),
            ],
        );

        let announcement = event.announcement().unwrap();
        assert_eq!(announcement.view_id, 2);
        assert_eq!(announcement.assertiveness, Assertiveness::Polite);
    }

    #[test]
    fn an_announcement_needs_a_view_and_a_message() {
        let no_view = event("announce", vec![("message", Value::String("Saved".into()))]);
        assert_eq!(no_view.announcement(), None);

        let no_message = event("announce", vec![("viewId", Value::I32(0))]);
        assert_eq!(no_message.announcement(), None);
    }

    #[test]
    fn other_events() {
        let event = event(
            "tooltip",
            vec![
                ("message", Value::String("Send".into())),
                ("nodeId", Value::I32(4)),
            ],
        );

        assert_eq!(event.kind, "tooltip");
    }
}
