//! SendKeys string parsing, mirroring `windows_use/uia/core.py` and
//! `windows_use/agent/desktop/utils.py`.
//!
//! The parser is verified against the Python original: `{Ctrl}a` emits Ctrl-down,
//! A-down, A-up, Ctrl-up; `{Shift}(ab)` holds Shift across both letters.

use crate::uia::input::{send_chord, tap_key, type_char, KeyChord, VK_CONTROL, VK_LWIN, VK_MENU, VK_SHIFT};

/// Escapes text so the SendKeys grammar types it literally.
pub fn escape_text_for_sendkeys(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '{' => result.push_str("{{}"),
            '}' => result.push_str("{}}"),
            '\n' => result.push_str("{Enter}"),
            '\t' => result.push_str("{Tab}"),
            '\r' => {}
            _ => result.push(character),
        }
    }
    result
}

/// Hold keys, matching the Python `holdKeys` tuple.
const HOLD_KEYS: [&str; 14] = [
    "WIN", "LWIN", "RWIN", "SHIFT", "LSHIFT", "RSHIFT", "CTRL", "CONTROL", "LCTRL", "RCTRL",
    "ALT", "LALT", "RALT", "LCONTROL",
];

fn modifier_for(upper: &str) -> Option<u16> {
    match upper {
        "WIN" | "LWIN" | "RWIN" => Some(VK_LWIN),
        "SHIFT" | "LSHIFT" | "RSHIFT" => Some(VK_SHIFT),
        "CTRL" | "CONTROL" | "LCTRL" | "RCTRL" | "LCONTROL" => Some(VK_CONTROL),
        "ALT" | "LALT" | "RALT" => Some(VK_MENU),
        _ => None,
    }
}

/// One key event in the flattened SendKeys program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendKeysAction {
    /// Virtual key, when the event targets a named key.
    pub vk: Option<u16>,
    /// Literal character, when the event types text.
    pub character: Option<char>,
    /// True for key-down, false for key-up.
    pub down: bool,
}

impl SendKeysAction {
    fn down_vk(vk: u16) -> Self {
        Self { vk: Some(vk), character: None, down: true }
    }

    fn up_vk(vk: u16) -> Self {
        Self { vk: Some(vk), character: None, down: false }
    }

    fn character(character: char) -> Self {
        Self { vk: None, character: Some(character), down: true }
    }
}

/// Parses a SendKeys string into its flattened key events.
///
/// Mirrors the Python grammar exactly, including `{a 3}` repetition and the
/// `(` `)` group extension of a held modifier.
pub fn parse_send_keys(text: &str) -> Result<Vec<SendKeysAction>, String> {
    let characters: Vec<char> = text.chars().collect();
    let mut keys: Vec<SendKeysAction> = Vec::new();
    let mut index = 0usize;
    let mut insert_index = 0usize;
    let mut hold = false;
    let mut include = false;

    while index < characters.len() {
        let character = characters[index];

        if character == '{' {
            let Some(offset) = characters[index..].iter().position(|c| *c == '}') else {
                return Err(invalid_braces());
            };
            let mut close = index + offset;
            if close == index + 1 {
                match characters[index + 2..].iter().position(|c| *c == '}') {
                    Some(next) => close = index + 2 + next,
                    None => return Err(invalid_braces()),
                }
            }

            let key_str: String = characters[index + 1..close].iter().collect();
            let parts: Vec<&str> = key_str.split(' ').filter(|part| !part.is_empty()).collect();
            if parts.is_empty() {
                return Err("Use \"{Space}\" or \" \" for \" \"".to_string());
            }
            let count = if parts.len() > 1 {
                parts[1]
                    .parse::<usize>()
                    .map_err(|_| format!("{{{key_str}}} is not valid"))?
            } else {
                1
            };
            let name = parts[0];
            let upper = name.to_uppercase();

            for _ in 0..count {
                if hold {
                    if let Some(modifier) = modifier_for(&upper) {
                        if keys.last() == Some(&SendKeysAction::down_vk(modifier)) {
                            insert_index += 1;
                        }
                        keys.insert(insert_index, SendKeysAction::down_vk(modifier));
                        keys.insert(insert_index + 1, SendKeysAction::up_vk(modifier));
                    } else if let Some(vk) = named_key(name) {
                        keys.insert(insert_index, SendKeysAction::down_vk(vk));
                        keys.insert(insert_index + 1, SendKeysAction::up_vk(vk));
                    } else if let Some(character) = single_character(name) {
                        match character_key_code(character) {
                            Some(vk) => {
                                keys.insert(insert_index, SendKeysAction::down_vk(vk));
                                keys.insert(insert_index + 1, SendKeysAction::up_vk(vk));
                            }
                            None => {
                                keys.insert(insert_index, SendKeysAction::character(character));
                            }
                        }
                    } else {
                        for part in name.chars() {
                            keys.insert(insert_index, SendKeysAction::character(part));
                        }
                    }
                    if include {
                        insert_index += 1;
                    } else {
                        hold = false;
                    }
                } else if let Some(character) = single_character(name) {
                    keys.push(SendKeysAction::character(character));
                } else if let Some(vk) = named_key(name) {
                    keys.push(SendKeysAction::down_vk(vk));
                    keys.push(SendKeysAction::up_vk(vk));
                    if HOLD_KEYS.contains(&upper.as_str()) {
                        hold = true;
                        // Insert the next key before this modifier's key-up.
                        insert_index = keys.len() - 1;
                    } else {
                        hold = false;
                    }
                } else {
                    for part in name.chars() {
                        keys.push(SendKeysAction::character(part));
                    }
                }
            }
            index = close + 1;
        } else if character == '(' {
            if hold {
                include = true;
            } else {
                keys.push(SendKeysAction::character('('));
            }
            index += 1;
        } else if character == ')' {
            if hold {
                include = false;
                hold = false;
            } else {
                keys.push(SendKeysAction::character(')'));
            }
            index += 1;
        } else if hold {
            // Under a held modifier, characters become virtual-key taps through
            // the `CharacterCodes` table (letters map to their uppercase VK).
            if let Some(vk) = character_key_code(character) {
                keys.insert(insert_index, SendKeysAction::down_vk(vk));
                keys.insert(insert_index + 1, SendKeysAction::up_vk(vk));
            } else {
                keys.insert(insert_index, SendKeysAction::character(character));
            }
            if include {
                insert_index += 1;
            } else {
                hold = false;
            }
            index += 1;
        } else {
            keys.push(SendKeysAction::character(character));
            index += 1;
        }
    }

    Ok(keys)
}

fn invalid_braces() -> String {
    "\"{\" or \"{}\" is not valid, use \"{{}\" for \"{\", use \"{}}\" for \"}\"".to_string()
}

/// Named keys map to virtual keys; single characters do not.
fn named_key(name: &str) -> Option<u16> {
    if single_character(name).is_some() {
        return None;
    }
    crate::uia::input::virtual_key_for_name(name)
}

/// `CharacterCodes`: the virtual-key a character maps to under a held modifier.
fn character_key_code(character: char) -> Option<u16> {
    if character.is_ascii_alphabetic() {
        return Some(character.to_ascii_uppercase() as u16);
    }
    if character.is_ascii_digit() {
        return Some(character as u16);
    }
    Some(match character {
        ';' => 0xBA,
        '=' => 0xBB,
        ',' => 0xBC,
        '-' => 0xBD,
        '.' => 0xBE,
        '/' => 0xBF,
        '`' => 0xC0,
        '[' => 0xDB,
        '\\' => 0xDC,
        ']' => 0xDD,
        '\'' => 0xDE,
        ' ' => 0x20,
        _ => return None,
    })
}

fn single_character(name: &str) -> Option<char> {
    let mut characters = name.chars();
    match (characters.next(), characters.next()) {
        (Some(character), None) => Some(character),
        _ => None,
    }
}

/// Executes a SendKeys string against the foreground window.
pub fn send_keys(text: &str, interval: f64, wait_time: f64) -> Result<(), String> {
    let actions = parse_send_keys(text)?;
    let delay = std::time::Duration::from_secs_f64(interval.max(0.0));

    for action in actions {
        if let Some(vk) = action.vk {
            if action.down {
                crate::uia::input::key_down_public(vk);
            } else {
                crate::uia::input::key_up_public(vk);
            }
        } else if let Some(character) = action.character {
            type_char(character);
        }
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }
    }

    std::thread::sleep(std::time::Duration::from_secs_f64(wait_time.max(0.0)));
    Ok(())
}

/// Converts a `ctrl+shift+n` style shortcut into a key chord.
pub fn parse_shortcut(shortcut: &str) -> KeyChord {
    let mut modifiers = Vec::new();
    let mut key = 0u16;
    let mut unicode = None;

    for part in shortcut.split('+') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some(modifier) = crate::uia::input::modifiers_for(part) {
            modifiers.push(modifier);
            continue;
        }
        let mut characters = part.chars();
        if let (Some(character), None) = (characters.next(), characters.next()) {
            if character.is_ascii_alphabetic() {
                key = character.to_ascii_uppercase() as u16;
            } else if character.is_ascii_digit() {
                key = character as u16;
            } else {
                unicode = Some(character);
            }
            continue;
        }
        if let Some(vk) = crate::uia::input::virtual_key_for_name(part) {
            key = vk;
        }
    }

    KeyChord { modifiers, key, unicode }
}

/// Executes a shortcut string like `ctrl+c`.
pub fn send_shortcut(shortcut: &str) {
    send_chord(&parse_shortcut(shortcut));
}

/// Literal text typing without SendKeys brace interpretation.
pub fn type_literal(text: &str, interval: f64) {
    for character in text.chars() {
        type_char(character);
        std::thread::sleep(std::time::Duration::from_secs_f64(interval.max(0.0)));
    }
}

/// Converts physical pixels to logical pixels for a given DPI scale.
pub fn physical_to_logical(value: i32, scale: f64) -> i32 {
    if scale <= 0.0 {
        return value;
    }
    (value as f64 / scale).round() as i32
}

/// Presses and releases a named key once (used by `Desktop::type`).
pub fn tap_named_key(name: &str, interval: f64) -> bool {
    let Some(vk) = crate::uia::input::virtual_key_for_name(name) else {
        return false;
    };
    tap_key(vk);
    std::thread::sleep(std::time::Duration::from_secs_f64(interval.max(0.0)));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summarize(actions: &[SendKeysAction]) -> Vec<String> {
        actions
            .iter()
            .map(|action| {
                if let Some(vk) = action.vk {
                    format!("{}:{vk}", if action.down { "down" } else { "up" })
                } else {
                    format!("char:{}", action.character.unwrap_or('?'))
                }
            })
            .collect()
    }

    #[test]
    fn ctrl_a_sequence_matches_python() {
        // Captured from the Python `SendKeys` with a stubbed keybd_event.
        let actions = parse_send_keys("{Ctrl}a{Back}{Enter}").expect("parse");
        assert_eq!(
            summarize(&actions),
            vec![
                "down:17",
                "down:65",
                "up:65",
                "up:17",
                "down:8",
                "up:8",
                "down:13",
                "up:13",
            ]
        );
    }

    #[test]
    fn repetition_matches_python() {
        let actions = parse_send_keys("{a 3}").expect("parse");
        assert_eq!(
            summarize(&actions),
            vec!["char:a", "char:a", "char:a"]
        );
    }

    #[test]
    fn escaped_braces_match_python() {
        let actions = parse_send_keys("{{}x{}}").expect("parse");
        assert_eq!(summarize(&actions), vec!["char:{", "char:x", "char:}"]);
    }

    #[test]
    fn held_shift_extends_over_a_group() {
        let actions = parse_send_keys("{Shift}(ab)").expect("parse");
        assert_eq!(
            summarize(&actions),
            vec![
                "down:16",
                "down:65",
                "down:66",
                "up:66",
                "up:65",
                "up:16",
            ]
        );
    }

    #[test]
    fn plain_text_types_characters() {
        let actions = parse_send_keys("hello").expect("parse");
        assert_eq!(
            summarize(&actions),
            vec!["char:h", "char:e", "char:l", "char:l", "char:o"]
        );
    }

    #[test]
    fn named_key_taps() {
        let actions = parse_send_keys("{Delete}").expect("parse");
        assert_eq!(summarize(&actions), vec!["down:46", "up:46"]);
    }

    #[test]
    fn unbalanced_braces_are_an_error() {
        assert!(parse_send_keys("{Ctrl").is_err());
    }

    #[test]
    fn escaping_matches_python() {
        // Verified against `escape_text_for_sendkeys` in desktop/utils.py.
        assert_eq!(
            escape_text_for_sendkeys("a{b}\nc\td\r{"),
            "a{{}b{}}{Enter}c{Tab}d{{}"
        );
    }
}
