use winit::keyboard::{Key, NamedKey};

// Emulates glfw key numbers
// https://github.com/flutter/flutter/blob/9a72e1c699f2b277b85110c89cbbb851f4de6935/packages/flutter/lib/src/services/keyboard_maps.g.dart#L1607-L1723
pub fn raw_key(key: Key) -> Option<u32> {
    if key >= Key::Character("a".into()) && key <= Key::Character("z".into()) {
        return Some(key.to_text()?.chars().next()? as u32);
    }

    if key >= Key::Character("0".into()) && key <= Key::Character("9".into()) {
        return Some(key.to_text()?.chars().next()? as u32);
    }

    // TODO(vially): Map remaining keys and modifiers state
    let code = match key {
        Key::Named(key) => match key {
            NamedKey::Space => 32,
            NamedKey::Escape => 256,
            NamedKey::Enter => 257,
            NamedKey::Tab => 258,
            NamedKey::Insert => 260,
            NamedKey::Delete => 261,
            NamedKey::ArrowRight => 262,
            NamedKey::ArrowLeft => 263,
            NamedKey::ArrowDown => 264,
            NamedKey::ArrowUp => 265,
            NamedKey::PageUp => 266,
            NamedKey::PageDown => 267,
            NamedKey::Home => 268,
            NamedKey::End => 269,
            NamedKey::Pause => 284,
            NamedKey::F1 => 290,
            NamedKey::F2 => 291,
            NamedKey::F3 => 292,
            NamedKey::F4 => 293,
            NamedKey::F5 => 294,
            NamedKey::F6 => 295,
            NamedKey::F7 => 296,
            NamedKey::F8 => 297,
            NamedKey::F9 => 298,
            NamedKey::F10 => 299,
            NamedKey::F11 => 300,
            NamedKey::F12 => 301,
            NamedKey::F13 => 302,
            NamedKey::F14 => 303,
            NamedKey::F15 => 304,
            NamedKey::F16 => 305,
            NamedKey::F17 => 306,
            NamedKey::F18 => 307,
            NamedKey::F19 => 308,
            NamedKey::F20 => 309,
            NamedKey::F21 => 310,
            NamedKey::F22 => 311,
            NamedKey::F23 => 312,
            _ => return None,
        },
        Key::Character(key) => match key.as_str() {
            "'" => 39,
            "," => 44,
            "-" => 45,
            "." => 46,
            "/" => 47,
            ";" => 59,
            "=" => 61,
            "[" => 91,
            "\\" => 92,
            "]" => 93,
            "`" => 96,
            _ => return None,
        },
        _ => return None,
    };
    Some(code)
}
