//! The message box at the bottom of the chat: a multiline field where Enter
//! sends, Shift+Enter adds a line, and Escape stops a running turn.

use gpui::{
    AnyElement, Context, KeyDownEvent, Modifiers, MouseButton, SharedString, Window, div,
    prelude::*, px, svg,
};

use super::AgentChatView;
use super::tooltip::tooltip;
use crate::domain::agent_chat::ChatConnectionState;
use crate::ui::text_edit::{TextKeyOutcome, apply_text_key};
use crate::ui::theme::{MONO_FONT, colors, surface_tint};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ComposerKey {
    /// The draft changed.
    Edited,
    /// Enter: send the draft.
    Send,
    /// Escape: stop the running turn, if any.
    Cancel,
    /// Not a composer key; let it propagate.
    Ignored,
}

/// Maps a key press onto the draft. Plain Enter sends; every other editing key
/// goes through the shared text editing rules (multiline, so Shift+Enter adds
/// a newline and paste keeps line breaks).
pub(super) fn handle_key(
    draft: &mut String,
    key: &str,
    key_char: Option<&str>,
    modifiers: &Modifiers,
    paste: impl FnOnce() -> Option<String>,
) -> ComposerKey {
    if key == "escape" || key == "esc" {
        return ComposerKey::Cancel;
    }
    if matches!(key, "enter" | "return") && !modifiers.shift {
        return ComposerKey::Send;
    }
    match apply_text_key(draft, key, key_char, modifiers, true, paste) {
        TextKeyOutcome::Edited => ComposerKey::Edited,
        TextKeyOutcome::Submit => ComposerKey::Send,
        TextKeyOutcome::Cancel => ComposerKey::Cancel,
        TextKeyOutcome::NextField | TextKeyOutcome::PreviousField | TextKeyOutcome::Unhandled => {
            ComposerKey::Ignored
        }
    }
}

impl AgentChatView {
    pub(super) fn on_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        if !self.accepts_input() {
            // Escape still stops a turn while the composer is locked.
            if matches!(key, "escape" | "esc") && self.is_working() {
                self.cancel_turn(cx);
                cx.stop_propagation();
            }
            return;
        }
        let outcome = handle_key(
            &mut self.draft,
            key,
            event.keystroke.key_char.as_deref(),
            &event.keystroke.modifiers,
            || cx.read_from_clipboard().and_then(|item| item.text()),
        );
        match outcome {
            ComposerKey::Edited => {}
            ComposerKey::Send => self.submit_draft(cx),
            ComposerKey::Cancel => {
                if !self.is_working() {
                    return;
                }
                self.cancel_turn(cx);
            }
            ComposerKey::Ignored => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn composer(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let enabled = self.accepts_input();
        let focused = self.focus_handle.is_focused(window);
        let placeholder = format!("Message {}…", self.agent().label());
        let lines: Vec<String> = self.draft.split('\n').map(str::to_owned).collect();
        let line_count = lines.len();
        let text = div()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(34.0))
            .max_h(px(160.0))
            .overflow_hidden()
            .py(px(7.0))
            .text_size(px(13.0))
            .line_height(px(20.0))
            .text_color(if enabled {
                colors().foreground
            } else {
                colors().subtle
            })
            .when(self.draft.is_empty(), |text| {
                text.text_color(colors().subtle).child(placeholder)
            })
            .when(!self.draft.is_empty(), |text| {
                text.children(lines.into_iter().enumerate().map(|(index, line)| {
                    let caret = focused && enabled && index + 1 == line_count;
                    let line = if caret { format!("{line}▏") } else { line };
                    div().min_h(px(20.0)).child(if line.is_empty() {
                        " ".to_owned()
                    } else {
                        line
                    })
                }))
            });
        div()
            .flex_none()
            .p_3()
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap_2()
                    .px_3()
                    .py_1()
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(if focused && enabled {
                        colors().accent
                    } else {
                        colors().border_subtle
                    })
                    .bg(surface_tint(colors().elevated, colors().terminal))
                    .font_family(MONO_FONT)
                    .child(text)
                    .child(self.composer_action(enabled, cx)),
            )
            .into_any_element()
    }

    /// Stop while a turn runs; otherwise the send button.
    fn composer_action(&self, enabled: bool, cx: &mut Context<Self>) -> AnyElement {
        if self.is_working() {
            return div()
                .id("agent-chat-stop")
                .flex_none()
                .h(px(26.0))
                .mb(px(4.0))
                .px_3()
                .flex()
                .items_center()
                .rounded(px(6.0))
                .border_1()
                .border_color(colors().border_subtle)
                .cursor_pointer()
                .text_size(px(12.0))
                .text_color(colors().foreground)
                .hover(|button| button.bg(colors().hover))
                .tooltip(|_, cx| tooltip("Stop · esc", cx))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, _, cx| this.cancel_turn(cx)))
                .child("Stop")
                .into_any_element();
        }
        let can_send = enabled && !self.draft.trim().is_empty();
        div()
            .id("agent-chat-send")
            .flex_none()
            .size(px(28.0))
            .mb(px(2.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.0))
            .when(can_send, |button| {
                button
                    .cursor_pointer()
                    .hover(|button| button.bg(colors().hover))
                    .on_click(cx.listener(|this, _, _, cx| this.submit_draft(cx)))
            })
            .when(!can_send, |button| button.opacity(0.4))
            .tooltip(|_, cx| tooltip("Send · ↵", cx))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                svg()
                    .path("chrome-icons/send.svg")
                    .size(px(14.0))
                    .flex_none()
                    .text_color(if can_send {
                        colors().foreground
                    } else {
                        colors().muted
                    }),
            )
            .into_any_element()
    }

    /// Whether the composer accepts input: the session is open and idle.
    pub(super) fn accepts_input(&self) -> bool {
        matches!(self.timeline.state, Some(ChatConnectionState::Ready))
    }
}

/// Shown next to the composer while the session is closed.
pub(super) fn closed_notice(message: Option<&str>) -> AnyElement {
    let text = message.unwrap_or("This chat session has ended.");
    div()
        .flex_none()
        .mx_3()
        .mb_3()
        .px_3()
        .py_2()
        .rounded(px(6.0))
        .bg(surface_tint(colors().danger, colors().terminal))
        .text_size(px(12.0))
        .text_color(colors().foreground)
        .child(SharedString::from(text.to_owned()))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(
        draft: &mut String,
        key: &str,
        key_char: Option<&str>,
        modifiers: Modifiers,
    ) -> ComposerKey {
        handle_key(draft, key, key_char, &modifiers, || {
            Some("pegado\r\nlínea".into())
        })
    }

    #[test]
    fn enter_sends_and_shift_enter_adds_a_line() {
        let mut draft = "hola".to_owned();
        assert_eq!(
            press(&mut draft, "enter", None, Modifiers::none()),
            ComposerKey::Send
        );
        assert_eq!(draft, "hola");
        assert_eq!(
            press(&mut draft, "enter", None, Modifiers::shift()),
            ComposerKey::Edited
        );
        assert_eq!(draft, "hola\n");
    }

    #[test]
    fn typing_paste_and_deletion_edit_the_draft() {
        let mut draft = String::new();
        assert_eq!(
            press(&mut draft, "h", Some("h"), Modifiers::none()),
            ComposerKey::Edited
        );
        assert_eq!(
            press(&mut draft, "v", None, Modifiers::command()),
            ComposerKey::Edited
        );
        assert_eq!(draft, "hpegado\nlínea");
        assert_eq!(
            press(&mut draft, "backspace", None, Modifiers::alt()),
            ComposerKey::Edited
        );
        assert_eq!(draft, "hpegado\n");
    }

    #[test]
    fn escape_cancels_and_app_shortcuts_pass_through() {
        let mut draft = "borrador".to_owned();
        assert_eq!(
            press(&mut draft, "escape", None, Modifiers::none()),
            ComposerKey::Cancel
        );
        assert_eq!(
            press(&mut draft, "w", Some("w"), Modifiers::command()),
            ComposerKey::Ignored
        );
        assert_eq!(draft, "borrador");
    }
}
