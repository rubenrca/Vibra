//! Keyboard editing for the Notes, Automations, and commit message fields. Like the review
//! comment drafts, text is edited at its end; there is no caret to move.

use gpui::Modifiers;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextKeyOutcome {
    Edited,
    Submit,
    Cancel,
    NextField,
    PreviousField,
    /// Not a text key: let app shortcuts (⌘W, ⌘1…) through.
    Unhandled,
}

pub(crate) fn apply_text_key(
    buffer: &mut String,
    key: &str,
    key_char: Option<&str>,
    modifiers: &Modifiers,
    multiline: bool,
    paste: impl FnOnce() -> Option<String>,
) -> TextKeyOutcome {
    match key {
        "escape" | "esc" => TextKeyOutcome::Cancel,
        "tab" if !multiline && modifiers.shift => TextKeyOutcome::PreviousField,
        "tab" if !multiline => TextKeyOutcome::NextField,
        "enter" | "return" if multiline && !modifiers.platform => {
            buffer.push('\n');
            TextKeyOutcome::Edited
        }
        "enter" | "return" => TextKeyOutcome::Submit,
        "backspace" => {
            if modifiers.platform {
                // Delete back to the start of the current (or last non-empty) line.
                let start = buffer
                    .trim_end_matches('\n')
                    .rfind('\n')
                    .map_or(0, |index| index + 1);
                buffer.truncate(start);
            } else if modifiers.alt {
                let trimmed = buffer.trim_end().len();
                let start = buffer[..trimmed]
                    .rfind(char::is_whitespace)
                    .map_or(0, |index| index + 1);
                buffer.truncate(start);
            } else {
                buffer.pop();
            }
            TextKeyOutcome::Edited
        }
        "v" if modifiers.platform && !modifiers.shift => {
            if let Some(text) = paste() {
                let text = text.replace("\r\n", "\n");
                if multiline {
                    buffer.push_str(&text);
                } else {
                    buffer.push_str(text.lines().collect::<Vec<_>>().join(" ").trim_end());
                }
            }
            TextKeyOutcome::Edited
        }
        _ if modifiers.platform || modifiers.control => TextKeyOutcome::Unhandled,
        _ => match key_char {
            Some(text) if !text.is_empty() => {
                buffer.push_str(text);
                TextKeyOutcome::Edited
            }
            _ => TextKeyOutcome::Unhandled,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(
        buffer: &mut String,
        key: &str,
        text: Option<&str>,
        modifiers: Modifiers,
    ) -> TextKeyOutcome {
        apply_text_key(buffer, key, text, &modifiers, true, || {
            Some("a\r\nb".into())
        })
    }

    #[test]
    fn multiline_fields_type_newlines_and_submit_with_command_enter() {
        let mut buffer = String::new();
        assert_eq!(
            key(&mut buffer, "h", Some("h"), Modifiers::none()),
            TextKeyOutcome::Edited
        );
        assert_eq!(
            key(&mut buffer, "enter", None, Modifiers::none()),
            TextKeyOutcome::Edited
        );
        assert_eq!(
            key(&mut buffer, "v", None, Modifiers::command()),
            TextKeyOutcome::Edited
        );
        assert_eq!(buffer, "h\na\nb");
        assert_eq!(
            key(&mut buffer, "enter", None, Modifiers::command()),
            TextKeyOutcome::Submit
        );
        assert_eq!(
            key(&mut buffer, "w", Some("w"), Modifiers::command()),
            TextKeyOutcome::Unhandled
        );
        assert_eq!(
            key(&mut buffer, "escape", None, Modifiers::none()),
            TextKeyOutcome::Cancel
        );
    }

    #[test]
    fn deletions_remove_a_character_word_or_line() {
        let mut buffer = "uno\ndos tres".to_owned();
        key(&mut buffer, "backspace", None, Modifiers::none());
        assert_eq!(buffer, "uno\ndos tre");
        key(&mut buffer, "backspace", None, Modifiers::alt());
        assert_eq!(buffer, "uno\ndos ");
        key(&mut buffer, "backspace", None, Modifiers::command());
        assert_eq!(buffer, "uno\n");
        key(&mut buffer, "backspace", None, Modifiers::command());
        assert_eq!(buffer, "");
    }

    #[test]
    fn single_line_fields_move_focus_and_flatten_pastes() {
        let mut buffer = String::new();
        let modifiers = Modifiers::command();
        assert_eq!(
            apply_text_key(&mut buffer, "v", None, &modifiers, false, || Some(
                "a\nb\n".into()
            )),
            TextKeyOutcome::Edited
        );
        assert_eq!(buffer, "a b");
        assert_eq!(
            apply_text_key(&mut buffer, "tab", None, &Modifiers::none(), false, || None),
            TextKeyOutcome::NextField
        );
        assert_eq!(
            apply_text_key(&mut buffer, "tab", None, &Modifiers::shift(), false, || {
                None
            }),
            TextKeyOutcome::PreviousField
        );
        assert_eq!(
            apply_text_key(
                &mut buffer,
                "enter",
                None,
                &Modifiers::none(),
                false,
                || None
            ),
            TextKeyOutcome::Submit
        );
    }
}
