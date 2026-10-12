//! Prompts written while a turn runs. They wait in a strip attached above the
//! composer card and go out one at a time as turns end.

use gpui::{
    AnyElement, App, ClickEvent, Context, MouseButton, SharedString, Window, div, prelude::*, px,
    svg,
};

use super::super::AgentChatView;
use super::super::tooltip::tooltip;
use super::popups::add_attachment;
use crate::domain::agent_chat::PromptFile;
use crate::ui::theme::{colors, surface_tint};

/// A prompt waiting for the running turn to end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::ui::agent_chat) struct QueuedPrompt {
    pub text: String,
    pub files: Vec<PromptFile>,
}

impl AgentChatView {
    /// The queued prompts, attached above the composer card; `None` when the queue is empty.
    pub(super) fn queue_strip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.queued.is_empty() {
            return None;
        }
        let editable = self.accepts_input();
        Some(
            div()
                .flex()
                .flex_col()
                .pb(px(4.0))
                .rounded_t(px(14.0))
                .border_1()
                .border_b_0()
                .border_color(colors().border_subtle)
                .bg(surface_tint(colors().elevated, colors().terminal))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .px(px(14.0))
                        .pt(px(8.0))
                        .pb(px(4.0))
                        .text_size(px(11.0))
                        .text_color(colors().muted)
                        .child("Queued")
                        .child(
                            div()
                                .text_color(colors().subtle)
                                .child(self.queued.len().to_string()),
                        ),
                )
                .children(
                    self.queued
                        .iter()
                        .enumerate()
                        .map(|(index, prompt)| self.queued_row(index, prompt, editable, cx)),
                )
                .into_any_element(),
        )
    }

    /// One queued prompt: its text on one line, then the edit and remove buttons.
    fn queued_row(
        &self,
        index: usize,
        prompt: &QueuedPrompt,
        editable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let text = prompt.text.split_whitespace().collect::<Vec<_>>().join(" ");
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .h(px(28.0))
            .pl(px(14.0))
            .pr(px(8.0))
            .text_size(px(13.0))
            .child(
                svg()
                    .path("chrome-icons/list-plus.svg")
                    .size(px(13.0))
                    .flex_none()
                    .text_color(colors().muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_color(colors().foreground)
                    .child(text),
            )
            .child(queued_button(
                SharedString::from(format!("agent-chat-queued-edit-{index}")),
                "chrome-icons/pencil.svg",
                "Edit queued message",
                editable,
                cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.focus_handle.focus(window);
                    this.edit_queued(index, cx);
                }),
            ))
            .child(queued_button(
                SharedString::from(format!("agent-chat-queued-remove-{index}")),
                "chrome-icons/close.svg",
                "Remove from queue",
                true,
                cx.listener(move |this, _: &ClickEvent, _, cx| this.remove_queued(index, cx)),
            ))
            .into_any_element()
    }

    /// Drops a queued prompt without sending it.
    fn remove_queued(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.queued.len() {
            self.queued.remove(index);
            cx.notify();
        }
    }

    /// Takes a queued prompt back into the draft, on a new line after any text
    /// already there. Its files return as attachments.
    fn edit_queued(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.queued.len() || !self.accepts_input() {
            return;
        }
        let prompt = self.queued.remove(index);
        if self.draft.trim().is_empty() {
            self.draft = prompt.text;
        } else {
            if !self.draft.ends_with('\n') {
                self.draft.push('\n');
            }
            self.draft.push_str(&prompt.text);
        }
        for file in prompt.files {
            add_attachment(&mut self.attachments, file);
        }
        self.after_edit(cx);
        cx.notify();
    }
}

/// A small icon button at the end of a queued row. A press does not reach the composer.
fn queued_button(
    id: SharedString,
    icon: &'static str,
    label: &'static str,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .flex_none()
        .size(px(22.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.0))
        .tooltip(move |_, cx| tooltip(label, cx))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .when(enabled, |button| {
            button
                .cursor_pointer()
                .hover(|button| button.bg(colors().hover))
                .on_click(on_click)
        })
        .when(!enabled, |button| button.opacity(0.4))
        .child(
            svg()
                .path(icon)
                .size(px(11.0))
                .flex_none()
                .text_color(colors().muted),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::agent_chat::{ChatAgent, ChatConnectionState};
    use gpui::TestAppContext;

    fn queued(text: &str) -> QueuedPrompt {
        QueuedPrompt {
            text: text.into(),
            files: Vec::new(),
        }
    }

    fn texts(queued: &[QueuedPrompt]) -> Vec<&str> {
        queued.iter().map(|prompt| prompt.text.as_str()).collect()
    }

    #[gpui::test]
    fn queued_prompts_can_be_removed_and_edited_back_into_the_draft(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut view = AgentChatView::empty(ChatAgent::Claude, cx);
            view.timeline.state = Some(ChatConnectionState::Working);
            view.queued = vec![
                queued("one"),
                QueuedPrompt {
                    text: "two".into(),
                    files: vec![PromptFile {
                        path: "/repo/src/lib.rs".into(),
                        label: "src/lib.rs".into(),
                    }],
                },
                queued("three"),
            ];
            view
        });
        view.update_in(cx, |view, _, cx| view.remove_queued(0, cx));
        view.read_with(cx, |view, _| {
            assert_eq!(texts(&view.queued), ["two", "three"])
        });

        view.update_in(cx, |view, _, cx| {
            view.draft = "draft".into();
            view.edit_queued(0, cx);
        });
        view.read_with(cx, |view, _| {
            // The edited prompt joins the draft on a new line and brings its file back.
            assert_eq!(view.draft, "draft\ntwo");
            assert_eq!(view.attachments.len(), 1);
            assert_eq!(texts(&view.queued), ["three"]);
        });

        view.update_in(cx, |view, _, cx| {
            view.draft.clear();
            view.edit_queued(0, cx);
            view.remove_queued(7, cx);
        });
        view.read_with(cx, |view, _| {
            assert_eq!(view.draft, "three");
            assert!(view.queued.is_empty());
        });
    }
}
