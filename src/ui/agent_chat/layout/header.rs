//! Thin header above the timeline: the session title, the connection state,
//! and the Continue in Terminal action. The pane header owns closing.

use gpui::{AnyElement, Context, FontWeight, Rgba, div, prelude::*, px, svg};

use super::super::tooltip::tooltip;
use super::super::{AgentChatView, AgentChatViewEvent};
use crate::domain::agent_chat::ChatConnectionState;
use crate::ui::theme::colors;

impl AgentChatView {
    pub(super) fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let (label, dot) = self.state_label();
        div()
            .h(px(32.0))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .border_b_1()
            .border_color(colors().border_subtle)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_size(px(12.5))
                    .font_weight(FontWeight::MEDIUM)
                    .child(self.title()),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(5.0))
                    .text_size(px(11.5))
                    .text_color(colors().muted)
                    .child(div().size(px(6.0)).flex_none().rounded(px(3.0)).bg(dot))
                    .child(label),
            )
            .child(self.continue_button(cx))
            .into_any_element()
    }

    /// The session title, or "<agent> · New chat" until the agent names it.
    fn title(&self) -> String {
        match self
            .timeline
            .title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
        {
            Some(title) => title.to_owned(),
            None => format!("{} · New chat", self.agent.label()),
        }
    }

    fn state_label(&self) -> (&'static str, Rgba) {
        match &self.timeline.state {
            None | Some(ChatConnectionState::Starting) => ("Starting…", colors().subtle),
            Some(ChatConnectionState::Ready) => ("Ready", colors().subtle),
            Some(ChatConnectionState::Working) => ("Working…", colors().accent),
            Some(ChatConnectionState::Closed { .. }) => ("Closed", colors().danger),
        }
    }

    /// Icon-only action; it is dimmed until the agent has a session to resume.
    fn continue_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let enabled = self.session_id().is_some();
        div()
            .id("agent-chat-continue-terminal")
            .size(px(22.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.0))
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
                    .size(px(14.0))
                    .flex_none()
                    .text_color(colors().muted),
            )
            .into_any_element()
    }
}
