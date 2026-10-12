//! An agent message: Markdown prose without a card. A final answer also carries
//! its turn's changed files (see `changes`), and every message has a meta row
//! with a copy button and the local time it arrived, revealed on hover.

mod changes;

use gpui::{AnyElement, ClipboardItem, Context, SharedString, div, prelude::*, px, svg};

use super::super::AgentChatView;
use super::super::layout::working::clock_time;
use super::super::tooltip::tooltip;
use crate::domain::agent_chat::ChatItem;
use crate::ui::markdown::markdown_prose;
use crate::ui::theme::colors;

impl AgentChatView {
    /// The message at `index`. `final_of` is the turn it answers once that turn
    /// has ended (indexes `ChatTimeline::turns`); `None` for commentary or a
    /// turn still running.
    pub(in super::super) fn assistant_row(
        &self,
        index: usize,
        text: &str,
        final_of: Option<usize>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let group = format!("chat-assistant-{index}");
        let card = final_of.and_then(|turn| self.changed_files_card(index, turn, cx));
        div()
            .group(group.clone())
            .w_full()
            .flex()
            .flex_col()
            .child(markdown_prose(&group, text))
            .when_some(card, |row, card| row.child(div().mt(px(12.0)).child(card)))
            .child(self.message_meta(index, &group, cx))
            .into_any_element()
    }

    /// The copy button and, when the arrival time is known, the local time. Both
    /// fade in on hover. The row always takes its height, so hovering moves nothing.
    fn message_meta(&self, index: usize, group: &str, cx: &mut Context<Self>) -> AnyElement {
        let clock = self.timeline.item_time(index).and_then(clock_time);
        div()
            .mt(px(6.0))
            .h(px(20.0))
            .flex()
            .items_center()
            .gap_2()
            .text_size(px(12.0))
            .text_color(colors().muted)
            .opacity(0.0)
            .group_hover(group.to_owned(), |meta| meta.opacity(1.0))
            .child(copy_button(index, cx))
            .when_some(clock, |meta, clock| meta.child(clock))
            .into_any_element()
    }
}

/// Copies the message's raw Markdown. The text is read on click, so the copy
/// always matches the message as it stands.
fn copy_button(index: usize, cx: &mut Context<AgentChatView>) -> AnyElement {
    div()
        .id(SharedString::from(format!("chat-assistant-copy-{index}")))
        .flex_none()
        .size(px(20.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.0))
        .cursor_pointer()
        .hover(|button| button.bg(colors().hover))
        .tooltip(|_, cx| tooltip("Copy", cx))
        .on_click(cx.listener(move |this, _, _, cx| {
            if let Some(ChatItem::AssistantMessage { text }) = this.timeline.items.get(index) {
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
            }
        }))
        .child(
            svg()
                .path("chrome-icons/copy.svg")
                .size(px(13.0))
                .flex_none()
                .text_color(colors().muted),
        )
        .into_any_element()
}
