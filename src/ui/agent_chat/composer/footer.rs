//! The quiet line under the composer card: the agent, its quota windows, and,
//! before the first message, a small icon button that switches to the terminal.

use gpui::{AnyElement, Context, div, prelude::*, px, svg};

use super::super::tooltip::tooltip;
use super::super::{AgentChatView, AgentChatViewEvent};
use super::meter::{quota_percent, quota_text};
use crate::ui::agent_marks::agent_compact_badge;
use crate::ui::theme::colors;

/// Quota at or above this share is drawn in the warning color.
const QUOTA_WARNING_PERCENT: u32 = 90;

impl AgentChatView {
    /// "[mark] Claude · 19% 3h 52m · 35% 1d 2h" on the left, and the Continue in
    /// Terminal button on the right. Muted text, no border.
    pub(super) fn composer_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut line = div()
            .flex()
            .items_center()
            .gap(px(5.0))
            .h(px(22.0))
            .px(px(6.0))
            .min_w(px(0.0))
            .text_size(px(11.0))
            .text_color(colors().muted)
            .child(agent_compact_badge(
                Some(self.agent.label()),
                None,
                None,
                false,
            ))
            .child(div().flex_none().child(self.agent.label()));
        for meter in &self.context.quota {
            let warning = quota_percent(meter.used_percent) >= QUOTA_WARNING_PERCENT;
            line = line.child(div().flex_none().child("·")).child(
                div()
                    .flex_none()
                    .when(warning, |text| text.text_color(colors().warning))
                    .child(quota_text(meter)),
            );
        }
        // A started chat stays a chat, so only an empty one offers the terminal.
        line.child(div().flex_1())
            .when(!self.has_conversation(), |line| {
                line.child(self.continue_in_terminal_button(cx))
            })
            .into_any_element()
    }

    /// Icon-only; dimmed until the session has an id to resume.
    fn continue_in_terminal_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let enabled = self.session_id().is_some();
        div()
            .id("agent-chat-continue-terminal")
            .size(px(22.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.0))
            .when(enabled, |button| {
                button
                    .cursor_pointer()
                    .hover(|button| button.bg(colors().hover))
                    .on_click(cx.listener(|_, _, _, cx| {
                        cx.emit(AgentChatViewEvent::SwitchToTerminal);
                    }))
            })
            .when(!enabled, |button| button.opacity(0.4))
            .tooltip(|_, cx| tooltip("Continue in Terminal", cx))
            .child(
                svg()
                    .path("chrome-icons/terminal.svg")
                    .size(px(13.0))
                    .flex_none()
                    .text_color(colors().muted),
            )
            .into_any_element()
    }
}
