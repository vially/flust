//! A plugin to handle mouse cursor.
//! It handles flutter/mousecursor type message.
use std::{
    str::FromStr,
    sync::{Arc, Weak},
};

use flutter_engine::{
    channel::{MethodCall, MethodCallHandler, MethodChannel},
    codec::STANDARD_CODEC,
    plugins::Plugin,
    FlutterEngine,
};

use flutter_engine::codec::Value;
use log::debug;
use parking_lot::Mutex;
use strum::EnumString;

pub const PLUGIN_NAME: &str = module_path!();
pub const CHANNEL_NAME: &str = "flutter/mousecursor";

// Note: This enum must be kept in sync with the `SystemMouseCursor` from Flutter:
// https://api.flutter.dev/flutter/services/SystemMouseCursors-class.html#constants
#[derive(Debug, Eq, PartialEq, strum::Display, EnumString)]
#[strum(serialize_all = "camelCase")]
pub enum SystemMouseCursor {
    /// A cursor indicating that the current operation will create an alias of, or a shortcut of the item.
    Alias,

    /// A cursor indicating scrolling in any direction.
    AllScroll,

    /// The platform-dependent basic cursor.
    Basic,

    /// A cursor indicating selectable table cells.
    Cell,

    /// A cursor that emphasizes an element being clickable, such as a hyperlink.
    Click,

    /// A cursor indicating somewhere the user can trigger a context menu.
    ContextMenu,

    /// A cursor indicating that the current operation will copy the item.
    Copy,

    /// A cursor indicating that the current operation will result in the disappearance of the item.
    Disappearing,

    /// A cursor indicating an operation that will not be carried out.
    Forbidden,

    /// A cursor indicating something that can be dragged.
    Grab,

    /// A cursor indicating something that is being dragged.
    Grabbing,

    /// A cursor indicating help information.
    Help,

    /// A cursor indicating moving something.
    Move,

    /// A cursor indicating somewhere that the current item may not be dropped.
    NoDrop,

    /// Hide the cursor.
    None,

    /// A cursor indicating precise selection, such as selecting a pixel in a bitmap.
    Precise,

    /// A cursor indicating the status that the program is busy but can still be interacted with.
    Progress,

    /// A cursor indicating resizing a column, or an item horizontally.
    ResizeColumn,

    /// A cursor indicating resizing an object from its bottom edge.
    ResizeDown,

    /// A cursor indicating resizing an object from its bottom-left corner.
    ResizeDownLeft,

    /// A cursor indicating resizing an object from its bottom-right corner.
    ResizeDownRight,

    /// A cursor indicating resizing an object from its left edge.
    ResizeLeft,

    /// A cursor indicating resizing an object bidirectionally from its left or right edge.
    ResizeLeftRight,

    /// A cursor indicating resizing an object from its right edge.
    ResizeRight,

    /// A cursor indicating resizing a row, or an item vertically.
    ResizeRow,

    /// A cursor indicating resizing an object from its top edge.
    ResizeUp,

    /// A cursor indicating resizing an object bidirectionally from its top or bottom edge.
    ResizeUpDown,

    /// A cursor indicating resizing an object from its top-left corner.
    ResizeUpLeft,

    /// A cursor indicating resizing an object bidirectionally from its top left or bottom right corner.
    ResizeUpLeftDownRight,

    /// A cursor indicating resizing an object from its top-right corner.
    ResizeUpRight,

    /// A cursor indicating resizing an object bidirectionally from its top right or bottom left corner.
    ResizeUpRightDownLeft,

    /// A cursor indicating selectable text.
    Text,

    /// A cursor indicating selectable vertical text.
    VerticalText,

    /// A cursor indicating the status that the program is busy and therefore can not be interacted with.
    Wait,

    /// A cursor indicating zooming in.
    ZoomIn,

    /// A cursor indicating zooming out.
    ZoomOut,
}

#[derive(Debug)]
pub struct MouseCursorError;

impl std::fmt::Display for MouseCursorError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Mouse cursor error")
    }
}

impl std::error::Error for MouseCursorError {}

pub trait MouseCursorHandler {
    fn activate_system_cursor(&mut self, kind: SystemMouseCursor) -> Result<(), MouseCursorError>;
}

pub struct MouseCursorPlugin {
    channel: Weak<MethodChannel>,
    handler: Arc<Mutex<dyn MouseCursorHandler + Send>>,
}

impl MouseCursorPlugin {
    pub fn new(handler: Arc<Mutex<dyn MouseCursorHandler + Send>>) -> Self {
        Self {
            channel: Weak::new(),
            handler,
        }
    }
}

impl Plugin for MouseCursorPlugin {
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

struct Handler {
    handler: Arc<Mutex<dyn MouseCursorHandler + Send>>,
}

impl MethodCallHandler for Handler {
    fn on_method_call(&mut self, call: MethodCall) {
        debug!(
            "got method call {} with args {:?}",
            call.method(),
            call.raw_args()
        );
        match call.method().as_str() {
            "activateSystemCursor" => {
                let Value::Map(v) = &call.args() else {
                    return call.error("unknown-data", "Unknown data type", Value::Null);
                };

                let Some(Value::String(kind)) = &v.get("kind") else {
                    return call.error("unknown-data", "Unknown data type", Value::Null);
                };

                let Ok(kind) = SystemMouseCursor::from_str(kind) else {
                    return call.error("unknown-data", "Unknown data type", Value::Null);
                };

                match self.handler.lock().activate_system_cursor(kind) {
                    Ok(_) => call.success_empty(),
                    Err(_) => call.error("unknown-data", "Unknown data type", Value::Null),
                };
            }
            _ => call.not_implemented(),
        }
    }
}
