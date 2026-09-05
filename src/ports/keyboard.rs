//! Shared mode-aware keyboard encoder for local and remote input.
use super::terminal::TerminalInputMode;
#[derive(Clone, Copy, Default)]
pub struct TerminalModifiers {
    pub shift: bool,
    pub alt: bool,
    pub control: bool,
    pub platform: bool,
}
impl TerminalModifiers {
    fn modified(self) -> bool {
        self.shift || self.alt || self.control || self.platform
    }
}
pub struct TerminalKeystroke {
    pub key: String,
    pub key_char: Option<String>,
    pub modifiers: TerminalModifiers,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalKeyEventType {
    Press,
    Repeat,
    Release,
}

pub fn key_event_bytes(
    keystroke: &TerminalKeystroke,
    mode: TerminalInputMode,
    event_type: TerminalKeyEventType,
) -> Option<Vec<u8>> {
    if event_type == TerminalKeyEventType::Release && !mode.report_event_types {
        return None;
    }

    let key = keystroke.key.to_ascii_lowercase();
    let kitty_control_code = match key.as_str() {
        "tab" => Some(9),
        "enter" | "return" => Some(13),
        "escape" | "esc" => Some(27),
        "space" => Some(32),
        "backspace" => Some(127),
        _ => None,
    };
    let modifiers = keystroke.modifiers;
    if let Some(codepoint) = kitty_control_code
        && (mode.report_all_keys_as_escape_codes
            || (mode.disambiguate_escape_codes
                && (modifiers.modified()
                    || matches!(
                        key.as_str(),
                        "tab" | "enter" | "return" | "escape" | "esc" | "backspace"
                    ))))
    {
        return Some(kitty_unicode_sequence(
            codepoint, None, modifiers, event_type, mode, None,
        ));
    }

    if let Some((base, terminator, application_sequence)) = named_key_sequence(&key) {
        let has_modifiers = modifiers.shift || modifiers.alt || modifiers.control;
        let kitty_event = mode.report_event_types && event_type != TerminalKeyEventType::Press;
        if has_modifiers || kitty_event {
            let base = if base.is_empty() { "1" } else { base };
            let mut sequence = format!("\x1b[{base};{}", modifier_parameter(modifiers));
            if kitty_event {
                sequence.push(':');
                sequence.push(key_event_code(event_type));
            }
            sequence.push(terminator);
            return Some(sequence.into_bytes());
        }

        if event_type == TerminalKeyEventType::Release {
            return None;
        }
        if application_sequence && mode.application_cursor {
            return Some(format!("\x1bO{terminator}").into_bytes());
        }
        if matches!(key.as_str(), "f1" | "f2" | "f3" | "f4") {
            return Some(format!("\x1bO{terminator}").into_bytes());
        }
        return Some(format!("\x1b[{base}{terminator}").into_bytes());
    }

    if event_type == TerminalKeyEventType::Release {
        return kitty_text_sequence(keystroke, mode, event_type);
    }

    match key.as_str() {
        "enter" | "return" => {
            return Some(prefixed_control_byte(b'\r', modifiers.alt));
        }
        "tab" if modifiers.shift => return Some(b"\x1b[Z".to_vec()),
        "tab" => return Some(prefixed_control_byte(b'\t', modifiers.alt)),
        "backspace" => return Some(prefixed_control_byte(0x7f, modifiers.alt)),
        "escape" | "esc" => return Some(prefixed_control_byte(0x1b, modifiers.alt)),
        _ => {}
    }

    if mode.kitty_keyboard()
        && (mode.report_all_keys_as_escape_codes
            || (mode.disambiguate_escape_codes && (modifiers.control || modifiers.alt)))
    {
        return kitty_text_sequence(keystroke, mode, event_type);
    }

    if modifiers.control {
        let character = key.chars().next()?;
        let control = match character {
            'a'..='z' => character as u8 - b'a' + 1,
            '@' | ' ' | '2' => 0,
            '[' | '3' => 27,
            '\\' | '4' => 28,
            ']' | '5' => 29,
            '^' | '6' => 30,
            '_' | '7' | '/' => 31,
            '8' | '?' => 127,
            _ => return None,
        };
        let mut bytes = Vec::with_capacity(2);
        if modifiers.alt {
            bytes.push(0x1b);
        }
        bytes.push(control);
        return Some(bytes);
    }

    if modifiers.alt {
        let text = keystroke.key_char.as_deref().unwrap_or(&keystroke.key);
        let mut bytes = Vec::with_capacity(text.len() + 1);
        bytes.push(0x1b);
        bytes.extend_from_slice(text.as_bytes());
        return Some(bytes);
    }

    None
}

fn named_key_sequence(key: &str) -> Option<(&'static str, char, bool)> {
    let sequence = match key {
        "up" => ("", 'A', true),
        "down" => ("", 'B', true),
        "right" => ("", 'C', true),
        "left" => ("", 'D', true),
        "home" => ("", 'H', false),
        "end" => ("", 'F', false),
        "insert" => ("2", '~', false),
        "delete" => ("3", '~', false),
        "pageup" | "page-up" => ("5", '~', false),
        "pagedown" | "page-down" => ("6", '~', false),
        "f1" => ("", 'P', false),
        "f2" => ("", 'Q', false),
        "f3" => ("", 'R', false),
        "f4" => ("", 'S', false),
        "f5" => ("15", '~', false),
        "f6" => ("17", '~', false),
        "f7" => ("18", '~', false),
        "f8" => ("19", '~', false),
        "f9" => ("20", '~', false),
        "f10" => ("21", '~', false),
        "f11" => ("23", '~', false),
        "f12" => ("24", '~', false),
        "f13" => ("25", '~', false),
        "f14" => ("26", '~', false),
        "f15" => ("28", '~', false),
        "f16" => ("29", '~', false),
        "f17" => ("31", '~', false),
        "f18" => ("32", '~', false),
        "f19" => ("33", '~', false),
        "f20" => ("34", '~', false),
        _ => return None,
    };
    Some(sequence)
}

fn kitty_text_sequence(
    keystroke: &TerminalKeystroke,
    mode: TerminalInputMode,
    event_type: TerminalKeyEventType,
) -> Option<Vec<u8>> {
    let base_character = keystroke.key.chars().next()?;
    let alternate_character = keystroke.key_char.as_deref().and_then(|text| {
        (text.chars().count() == 1)
            .then(|| text.chars().next())
            .flatten()
    });
    let alternate = mode
        .report_alternate_keys
        .then_some(alternate_character)
        .flatten()
        .filter(|alternate| *alternate != base_character)
        .map(u32::from);
    let associated_text = mode
        .report_associated_text
        .then_some(keystroke.key_char.as_deref())
        .flatten()
        .filter(|text| !text.is_empty());
    Some(kitty_unicode_sequence(
        u32::from(base_character),
        alternate,
        keystroke.modifiers,
        event_type,
        mode,
        associated_text,
    ))
}

fn kitty_unicode_sequence(
    codepoint: u32,
    alternate: Option<u32>,
    modifiers: TerminalModifiers,
    event_type: TerminalKeyEventType,
    mode: TerminalInputMode,
    associated_text: Option<&str>,
) -> Vec<u8> {
    let mut sequence = format!("\x1b[{codepoint}");
    if let Some(alternate) = alternate {
        sequence.push(':');
        sequence.push_str(&alternate.to_string());
    }
    let include_event = mode.report_event_types && event_type != TerminalKeyEventType::Press;
    if modifiers.modified() || include_event || associated_text.is_some() {
        sequence.push(';');
        sequence.push_str(&modifier_parameter(modifiers).to_string());
    }
    if include_event {
        sequence.push(':');
        sequence.push(key_event_code(event_type));
    }
    if let Some(text) = associated_text {
        sequence.push(';');
        let mut codepoints = text.chars().map(u32::from);
        if let Some(codepoint) = codepoints.next() {
            sequence.push_str(&codepoint.to_string());
            for codepoint in codepoints {
                sequence.push(':');
                sequence.push_str(&codepoint.to_string());
            }
        }
    }
    sequence.push('u');
    sequence.into_bytes()
}

fn modifier_parameter(modifiers: TerminalModifiers) -> u8 {
    1 + modifiers.shift as u8
        + (modifiers.alt as u8 * 2)
        + (modifiers.control as u8 * 4)
        + (modifiers.platform as u8 * 8)
}

fn key_event_code(event_type: TerminalKeyEventType) -> char {
    match event_type {
        TerminalKeyEventType::Press => '1',
        TerminalKeyEventType::Repeat => '2',
        TerminalKeyEventType::Release => '3',
    }
}

fn prefixed_control_byte(byte: u8, alt: bool) -> Vec<u8> {
    if alt { vec![0x1b, byte] } else { vec![byte] }
}
