use std::ffi::CString;

use flutter_engine::{
    ffi::{FlutterKeyEvent, FlutterKeyEventDeviceType, FlutterKeyEventType, FlutterLogicalKey},
    FlutterEngine,
};
use flutter_plugins::keyevent::{KeyAction, KeyActionType};
use smithay_client_toolkit::seat::keyboard::{KeyCode, KeyEvent, Keysym, Modifiers};

#[derive(Clone, Debug)]
pub struct SctkKeyEvent {
    pub(crate) device_type: FlutterKeyEventDeviceType,
    pub(crate) event: KeyEvent,
    pub(crate) kind: FlutterKeyEventType,
    pub(crate) modifiers: Modifiers,
    pub(crate) synthesized: bool,

    /// For `Up` events, this field holds the corresponding down `Keysym`. For
    /// all other event kinds, this field will be `None`.
    pub(crate) latched_keydown: Option<Keysym>,
}

impl SctkKeyEvent {
    pub(crate) fn new(
        device_type: FlutterKeyEventDeviceType,
        event: KeyEvent,
        kind: FlutterKeyEventType,
        latched_keydown: Option<Keysym>,
        modifiers: Modifiers,
        synthesized: bool,
    ) -> Self {
        Self {
            device_type,
            event,
            latched_keydown,
            kind,
            modifiers,
            synthesized,
        }
    }
}

impl From<SctkKeyEvent> for FlutterKeyEvent {
    fn from(value: SctkKeyEvent) -> Self {
        // Since wl_keyboard::key's `time` argument has an undefined base [0]
        // there is no easy way to convert it to a Flutter timestamp (see
        // `FlutterEngineGetCurrentTime()`). Therefore, the current engine time
        // is used instead.
        //
        // [0]: https://wayland.app/protocols/wayland#wl_keyboard:event:key
        let timestamp = FlutterEngine::get_current_time_duration();

        let character = value.event.utf8.and_then(|utf8| CString::new(utf8).ok());

        let character = match value.kind {
            FlutterKeyEventType::Up => None,
            FlutterKeyEventType::Down => character,
            FlutterKeyEventType::Repeat => character,
        };

        // Flutter triggers an assertion failure when the *logical* key of an
        // `Up` event does not match *exactly* the logical key of its
        // corresponding `Down` event [0].
        //
        // However, there are legitimate reasons why the logical key would
        // change between the `Down` and `Up` events. This could happen, for
        // example, when a logical key changes case between the up and down
        // events.
        //
        // A common sequence of events that could lead to this scenario is:
        // - `XK_Shift` down
        // - `XK_A` down (upper-case `A`, due to shift being down)
        // - `XK_Shift` up
        // - `XK_a` up (lower-case `a`, due to shift no longer being down)
        //
        // Therefore, in order to avoid the failed assertion, the logical key of
        // the `Up` event that gets sent to the engine is built using the keysym
        // of its corresponding `Down` event (instead of using its own keysym,
        // which might be different).
        //
        // [0](https://github.com/flutter/flutter/blob/3.22.1/packages/flutter/lib/src/services/hardware_keyboard.dart#L512-L515)
        let keysym = match value.kind {
            FlutterKeyEventType::Up => value.latched_keydown.unwrap_or(value.event.keysym),
            _ => value.event.keysym,
        };

        Self::new(
            timestamp,
            value.kind,
            SctkPhysicalKey::new(value.event.raw_code).into(),
            SctkLogicalKey::new(keysym).into(),
            character,
            value.synthesized,
            value.device_type,
        )
    }
}

impl From<SctkKeyEvent> for KeyAction {
    fn from(event: SctkKeyEvent) -> Self {
        let event_type = match event.kind {
            FlutterKeyEventType::Up => KeyActionType::Keyup,
            FlutterKeyEventType::Down => KeyActionType::Keydown,
            FlutterKeyEventType::Repeat => KeyActionType::Keydown,
        };

        let modifiers: GtkKeyActionModifiers = event.modifiers.into();

        let logical: FlutterLogicalKey = SctkLogicalKey::new(event.event.keysym).into();
        let specified_logical_key: i64 = logical.raw().try_into().unwrap_or(0);

        let unicode_scalar_value: Option<SctkUnicodeScalarValue> = event.event.utf8.try_into().ok();
        let unicode_scalar_values = unicode_scalar_value
            .map(|value| value.0 as i64)
            .unwrap_or(0);

        Self {
            toolkit: "gtk".to_string(),
            key_code: event.event.keysym.raw() as i32,
            // Comment in `SctkPhysicalKey::new` provides some context about `+ 8`
            scan_code: (event.event.raw_code + 8) as i32,
            modifiers: modifiers.0,
            specified_logical_key,
            unicode_scalar_values,
            keymap: "linux".to_string(),
            _type: event_type,
        }
    }
}

struct GtkKeyActionModifiers(i32);

impl From<Modifiers> for GtkKeyActionModifiers {
    fn from(modifiers: Modifiers) -> Self {
        let ctrl: i32 = modifiers.ctrl.into();
        let alt: i32 = modifiers.alt.into();
        let shift: i32 = modifiers.shift.into();
        let caps_lock: i32 = modifiers.caps_lock.into();
        let logo: i32 = modifiers.logo.into();
        let num_lock: i32 = modifiers.num_lock.into();

        // These values need to be kept in sync with the same values on the framework side.
        // https://github.com/flutter/flutter/blob/1fa6f56b/packages/flutter/lib/src/services/raw_keyboard_linux.dart#L371-L411
        let raw_modifiers =
            shift | caps_lock << 1 | ctrl << 2 | alt << 3 | num_lock << 4 | logo << 26;

        Self(raw_modifiers)
    }
}

pub(crate) struct SctkPhysicalKey(KeyCode);

impl SctkPhysicalKey {
    pub fn new(raw_code: u32) -> Self {
        // Add 8 to the key event keycode to determine the xcb keycode:
        // https://wayland.app/protocols/wayland#wl_keyboard:enum:keymap_format:entry:xkb_v1
        Self(KeyCode::new(raw_code + 8))
    }

    pub fn raw(&self) -> KeyCode {
        self.0
    }
}

pub(crate) struct SctkLogicalKey(Keysym);

impl SctkLogicalKey {
    pub fn new(keysym: Keysym) -> Self {
        Self(keysym)
    }

    pub fn raw(&self) -> Keysym {
        self.0
    }
}

struct SctkUnicodeScalarValue(char);

impl TryFrom<Option<String>> for SctkUnicodeScalarValue {
    type Error = ();

    // Returns `Ok` if the value is *exactly* one `char` long or `Err` otherwise.
    fn try_from(value: Option<String>) -> Result<Self, Self::Error> {
        let Some(value) = value else {
            return Err(());
        };

        let mut chars = value.chars();
        let char = chars.next().ok_or(())?;

        match chars.next() {
            None => Ok(Self(char)),
            Some(_) => Err(()),
        }
    }
}

pub(crate) trait SctkFlutterStringExt {
    fn is_control_character(&self) -> bool;
}

impl SctkFlutterStringExt for String {
    // Implementation is based on similar logic found in the Flutter engine:
    // https://github.com/flutter/engine/blob/3.22.1/shell/platform/darwin/macos/framework/Source/FlutterEmbedderKeyResponder.mm#L30-L35
    fn is_control_character(&self) -> bool {
        let chars = self.as_bytes();
        if chars.len() != 1 {
            return false;
        }

        let character = &chars[0];
        (0x00..=0x1f).contains(character) || (0x7f..=0x9f).contains(character)
    }
}
