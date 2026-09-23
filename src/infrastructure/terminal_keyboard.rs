//! Kitty/xterm keyboard encoding for terminal input.

use crate::ports::terminal::TerminalInputMode;
use crate::ports::terminal_keyboard::{
    TerminalKeyEventType, TerminalKeyInput, TerminalKeystroke, TerminalModifiers,
};

impl TerminalModifiers {
    fn modified(self) -> bool {
        self.shift || self.alt || self.control || self.platform
    }
}

impl TerminalKeyInput {
    pub fn bytes(&self, mode: TerminalInputMode) -> Vec<u8> {
        key_event_bytes(&self.keystroke, mode, self.event_type)
            .or_else(|| {
                // The UI may have consumed printable text in report-all mode
                // before the application disabled that mode. Preserve the text.
                (self.event_type != TerminalKeyEventType::Release
                    && !self.keystroke.modifiers.control
                    && !self.keystroke.modifiers.alt
                    && !self.keystroke.modifiers.platform)
                    .then(|| {
                        self.keystroke
                            .key_char
                            .as_ref()
                            .map(|s| s.as_bytes().to_vec())
                    })
                    .flatten()
            })
            .unwrap_or_default()
    }
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
    // These keys remain usable by a shell after an application exits without
    // resetting Kitty mode. Only report-all mode encodes their release events.
    let legacy_control_key = matches!(key.as_str(), "tab" | "enter" | "return" | "backspace");
    if legacy_control_key
        && !mode.report_all_keys_as_escape_codes
        && event_type == TerminalKeyEventType::Release
    {
        return None;
    }
    if let Some(codepoint) = kitty_control_code
        && (mode.report_all_keys_as_escape_codes
            || (!legacy_control_key
                && mode.disambiguate_escape_codes
                && (modifiers.control
                    || modifiers.alt
                    || matches!(key.as_str(), "escape" | "esc")))
            || (matches!(key.as_str(), "escape" | "esc")
                && mode.report_event_types
                && event_type != TerminalKeyEventType::Press))
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
        return (mode.report_all_keys_as_escape_codes
            || (mode.disambiguate_escape_codes && (modifiers.control || modifiers.alt)))
            .then(|| kitty_text_sequence(keystroke, mode, event_type))
            .flatten();
    }

    match key.as_str() {
        "enter" | "return" => {
            return Some(prefixed_control_byte(b'\r', modifiers.alt));
        }
        "tab" if modifiers.shift => {
            return Some(if modifiers.alt {
                b"\x1b\x1b[Z".to_vec()
            } else {
                b"\x1b[Z".to_vec()
            });
        }
        "tab" => return Some(prefixed_control_byte(b'\t', modifiers.alt)),
        "backspace" => {
            return Some(prefixed_control_byte(
                if modifiers.control { 0x08 } else { 0x7f },
                modifiers.alt,
            ));
        }
        "escape" | "esc" => return Some(prefixed_control_byte(0x1b, modifiers.alt)),
        "space" if modifiers.control => return Some(prefixed_control_byte(0, modifiers.alt)),
        "space" if modifiers.alt => return Some(b"\x1b ".to_vec()),
        _ => {}
    }

    if mode.kitty_keyboard()
        && (mode.report_all_keys_as_escape_codes
            || (mode.disambiguate_escape_codes && (modifiers.control || modifiers.alt)))
    {
        return kitty_text_sequence(keystroke, mode, event_type);
    }

    // Legacy Ctrl+Shift text keys have no unambiguous C0 encoding. Kitty's
    // baseline protocol uses CSI u for these even before enhancement flags.
    if modifiers.control && modifiers.shift {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_printable_text_survives_report_all_reset() {
        let mut input = TerminalKeyInput {
            keystroke: TerminalKeystroke {
                key: "ñ".into(),
                key_char: Some("Ñ".into()),
                modifiers: TerminalModifiers {
                    shift: true,
                    ..TerminalModifiers::default()
                },
            },
            event_type: TerminalKeyEventType::Press,
        };
        let active = TerminalInputMode {
            report_all_keys_as_escape_codes: true,
            report_event_types: true,
            ..TerminalInputMode::default()
        };
        assert!(input.bytes(active).starts_with(b"\x1b["));
        for event_type in [TerminalKeyEventType::Press, TerminalKeyEventType::Repeat] {
            input.event_type = event_type;
            assert_eq!(input.bytes(TerminalInputMode::default()), "Ñ".as_bytes());
        }
        input.event_type = TerminalKeyEventType::Release;
        assert!(input.bytes(TerminalInputMode::default()).is_empty());
    }

    fn stroke(key: &str, text: Option<&str>) -> TerminalKeystroke {
        TerminalKeystroke {
            key: key.into(),
            key_char: text.map(str::to_owned),
            modifiers: TerminalModifiers::default(),
        }
    }

    #[test]
    fn kitty_disambiguation_keeps_shell_control_keys_in_legacy_form() {
        let mode = TerminalInputMode {
            disambiguate_escape_codes: true,
            report_event_types: true,
            ..TerminalInputMode::default()
        };
        for (key, bytes) in [
            ("enter", b"\r".as_slice()),
            ("tab", b"\t".as_slice()),
            ("backspace", b"\x7f".as_slice()),
        ] {
            let key = stroke(key, None);
            assert_eq!(
                key_event_bytes(&key, mode, TerminalKeyEventType::Press).as_deref(),
                Some(bytes)
            );
            assert_eq!(
                key_event_bytes(&key, mode, TerminalKeyEventType::Repeat).as_deref(),
                Some(bytes)
            );
            assert_eq!(
                key_event_bytes(&key, mode, TerminalKeyEventType::Release),
                None
            );
        }
    }

    #[test]
    fn kitty_event_types_do_not_emit_text_release_events_without_report_all() {
        let mode = TerminalInputMode {
            report_event_types: true,
            ..TerminalInputMode::default()
        };
        assert_eq!(
            key_event_bytes(
                &stroke("space", Some(" ")),
                mode,
                TerminalKeyEventType::Release
            ),
            None
        );
        assert_eq!(
            key_event_bytes(&stroke("a", Some("a")), mode, TerminalKeyEventType::Release),
            None
        );
        assert_eq!(
            key_event_bytes(&stroke("escape", None), mode, TerminalKeyEventType::Release)
                .as_deref(),
            Some(b"\x1b[27;1:3u".as_slice())
        );

        let mut shifted_space = stroke("space", Some(" "));
        shifted_space.modifiers.shift = true;
        assert_eq!(
            key_event_bytes(
                &shifted_space,
                TerminalInputMode {
                    disambiguate_escape_codes: true,
                    ..mode
                },
                TerminalKeyEventType::Press
            ),
            None
        );
    }

    #[test]
    fn kitty_report_all_includes_control_key_release_events() {
        let mode = TerminalInputMode {
            report_all_keys_as_escape_codes: true,
            report_event_types: true,
            ..TerminalInputMode::default()
        };
        assert_eq!(
            key_event_bytes(&stroke("tab", None), mode, TerminalKeyEventType::Release).as_deref(),
            Some(b"\x1b[9;1:3u".as_slice())
        );
        assert_eq!(
            key_event_bytes(
                &stroke("space", Some(" ")),
                mode,
                TerminalKeyEventType::Release
            )
            .as_deref(),
            Some(b"\x1b[32;1:3u".as_slice())
        );
    }

    #[test]
    fn legacy_control_combinations_match_kitty_c0_table() {
        let mut backspace = stroke("backspace", None);
        backspace.modifiers.control = true;
        assert_eq!(
            key_event_bytes(
                &backspace,
                TerminalInputMode::default(),
                TerminalKeyEventType::Press
            ),
            Some(vec![0x08])
        );
        backspace.modifiers.alt = true;
        assert_eq!(
            key_event_bytes(
                &backspace,
                TerminalInputMode {
                    disambiguate_escape_codes: true,
                    ..TerminalInputMode::default()
                },
                TerminalKeyEventType::Press
            ),
            Some(vec![0x1b, 0x08])
        );

        let mut space = stroke("space", Some(" "));
        space.modifiers.control = true;
        assert_eq!(
            key_event_bytes(
                &space,
                TerminalInputMode::default(),
                TerminalKeyEventType::Press
            ),
            Some(vec![0])
        );

        let mut tab = stroke("tab", None);
        tab.modifiers.shift = true;
        tab.modifiers.alt = true;
        assert_eq!(
            key_event_bytes(
                &tab,
                TerminalInputMode::default(),
                TerminalKeyEventType::Press
            ),
            Some(b"\x1b\x1b[Z".to_vec())
        );

        let mut letter = stroke("c", Some("C"));
        letter.modifiers.control = true;
        letter.modifiers.shift = true;
        assert_eq!(
            key_event_bytes(
                &letter,
                TerminalInputMode::default(),
                TerminalKeyEventType::Press
            ),
            Some(b"\x1b[99;6u".to_vec())
        );
    }
}
