//! Chrome around the timeline: the header strip, the list or its empty and
//! closed states, and the footer (composer or closed notice).

use gpui::{
    AnyElement, Context, FontWeight, MouseButton, Render, Rgba, Window, div, list, prelude::*, px,
    svg,
};

use super::composer::closed_notice;
use super::tooltip::tooltip;
use super::{AgentChatView, AgentChatViewEvent};
use crate::domain::agent_chat::ChatConnectionState;
use crate::ui::agent_marks::agent_compact_badge;
use crate::ui::theme::{MONO_FONT, colors, surface, surface_tint};

impl Render for AgentChatView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("agent-chat")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, _| this.focus_handle.focus(window)),
            )
            .size_full()
            .flex()
            .flex_col()
            .bg(surface(colors().terminal))
            .text_color(colors().foreground)
            .child(self.header(cx))
            .child(self.body(cx))
            .child(self.footer(window, cx))
    }
}

impl AgentChatView {
    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let (label, color) = self.state_label();
        div()
            .h(px(30.0))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .border_b_1()
            .border_color(colors().border_subtle)
            .child(agent_compact_badge(
                Some(self.agent.label()),
                None,
                None,
                true,
            ))
            .child(
                div()
                    .text_size(px(12.5))
                    .font_weight(FontWeight::MEDIUM)
                    .child(self.agent.label()),
            )
            .child(div().text_size(px(11.5)).text_color(color).child(label))
            .child(div().flex_1())
            .child(self.continue_button(cx))
            .into_any_element()
    }

    fn state_label(&self) -> (&'static str, Rgba) {
        match &self.timeline.state {
            None | Some(ChatConnectionState::Starting) => ("Starting…", colors().subtle),
            Some(ChatConnectionState::Ready) => ("Ready", colors().subtle),
            Some(ChatConnectionState::Working) => ("Working…", colors().accent),
            Some(ChatConnectionState::Closed { .. }) => ("Closed", colors().subtle),
        }
    }

    fn continue_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let enabled = self.session_id().is_some();
        div()
            .id("agent-chat-continue-terminal")
            .h(px(22.0))
            .px_2()
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .rounded(px(5.0))
            .border_1()
            .border_color(colors().border_subtle)
            .text_size(px(12.0))
            .text_color(if enabled {
                colors().foreground
            } else {
                colors().subtle
            })
            .when(enabled, |button| {
                button
                    .cursor_pointer()
                    .hover(|button| button.bg(colors().hover))
                    .on_click(cx.listener(|_, _, _, cx| {
                        cx.emit(AgentChatViewEvent::SwitchToTerminal);
                    }))
            })
            .when(!enabled, |button| button.opacity(0.5))
            .tooltip(|_, cx| tooltip("Resume this session in the terminal", cx))
            .child(
                svg()
                    .path("chrome-icons/terminal.svg")
                    .size(px(12.0))
                    .flex_none()
                    .text_color(colors().muted),
            )
            .child("Continue in Terminal")
            .into_any_element()
    }

    fn body(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.timeline.items.is_empty() {
            return self.empty_body();
        }
        div()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .overflow_hidden()
            .px_4()
            .pt_3()
            .child(
                list(
                    self.list_state.clone(),
                    cx.processor(|this, index: usize, _, cx| this.render_item(index, cx)),
                )
                .size_full(),
            )
            .into_any_element()
    }

    /// Shown before the first item: connecting, ready for a prompt, or closed.
    fn empty_body(&self) -> AnyElement {
        let card = match &self.timeline.state {
            Some(ChatConnectionState::Closed { message }) => self.closed_card(message.as_deref()),
            _ => {
                let text = match self.timeline.state {
                    Some(ChatConnectionState::Ready) => {
                        format!(
                            "Send a message to start working with {}.",
                            self.agent.label()
                        )
                    }
                    _ => format!("Connecting to {}…", self.agent.label()),
                };
                div()
                    .text_size(px(12.5))
                    .text_color(colors().subtle)
                    .child(text)
                    .into_any_element()
            }
        };
        div()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .flex()
            .items_center()
            .justify_center()
            .px_4()
            .child(card)
            .into_any_element()
    }

    /// Readable block for a session that never started or has ended.
    fn closed_card(&self, message: Option<&str>) -> AnyElement {
        let message = message.unwrap_or("The agent session ended.");
        let multiline = message.contains('\n');
        div()
            .w_full()
            .max_w(px(520.0))
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .rounded(px(8.0))
            .border_1()
            .border_color(colors().border_subtle)
            .bg(surface_tint(colors().elevated, colors().terminal))
            .child(
                div()
                    .text_size(px(13.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Chat is not available"),
            )
            .child(if multiline {
                div()
                    .id("agent-chat-closed-message")
                    .max_h(px(220.0))
                    .overflow_y_scroll()
                    .font_family(MONO_FONT)
                    .text_size(px(11.5))
                    .line_height(px(16.0))
                    .text_color(colors().muted)
                    .child(message.to_owned())
                    .into_any_element()
            } else {
                div()
                    .text_size(px(12.5))
                    .line_height(px(18.0))
                    .text_color(colors().muted)
                    .child(message.to_owned())
                    .into_any_element()
            })
            .when(self.session_id().is_some(), |card| {
                card.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(colors().subtle)
                        .child("Use Continue in Terminal to keep working in the terminal."),
                )
            })
            .into_any_element()
    }

    fn footer(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        match &self.timeline.state {
            // With no items the closed card already shows the message.
            Some(ChatConnectionState::Closed { message }) if !self.timeline.items.is_empty() => {
                div()
                    .flex_none()
                    .child(closed_notice(message.as_deref()))
                    .into_any_element()
            }
            Some(ChatConnectionState::Closed { .. }) => div().into_any_element(),
            _ => self.composer(window, cx),
        }
    }
}
