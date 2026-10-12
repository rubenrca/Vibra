//! Chrome around the timeline: the header strip, the list or its empty and
//! closed states, the working line and the footer (composer or closed card).

mod header;
mod working;

use gpui::{
    AnyElement, Context, FontWeight, MouseButton, Render, Window, div, list, prelude::*, px,
};

use super::AgentChatView;
use crate::domain::agent_chat::ChatConnectionState;
use crate::ui::agent_marks::agent_compact_badge;
use crate::ui::theme::{MONO_FONT, colors, surface, surface_tint};
use working::model_name;

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
    fn body(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.timeline.items.is_empty() {
            return self.empty_body();
        }
        let status = self.working_line();
        div()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .flex()
            .flex_col()
            .child(
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
                    ),
            )
            .children(status)
            .into_any_element()
    }

    /// Shown before the first item: the agent's identity while it starts or
    /// waits for a prompt, or the closed card.
    fn empty_body(&self) -> AnyElement {
        let content = match &self.timeline.state {
            Some(ChatConnectionState::Closed { message }) => self.closed_card(message.as_deref()),
            _ => self.identity(),
        };
        div()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .flex()
            .items_center()
            .justify_center()
            .px_4()
            .child(content)
            .into_any_element()
    }

    /// Agent name, with its model when known, or "Starting…" while it connects.
    fn identity(&self) -> AnyElement {
        let label = self.agent.label();
        let starting = matches!(
            self.timeline.state,
            None | Some(ChatConnectionState::Starting)
        );
        let detail = if starting {
            Some("Starting…".to_owned())
        } else {
            model_name(&self.timeline.config_options).map(str::to_owned)
        };
        let ready = matches!(self.timeline.state, Some(ChatConnectionState::Ready));
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .child(agent_compact_badge(Some(label), None, None, true))
            .child(
                div()
                    .text_size(px(17.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(label),
            )
            .when_some(detail, |column, detail| {
                column.child(
                    div()
                        .text_size(px(12.5))
                        .text_color(colors().muted)
                        .child(detail),
                )
            })
            .when(ready, |column| {
                column.child(
                    div()
                        .mt_2()
                        .text_size(px(12.0))
                        .text_color(colors().subtle)
                        .child(format!("Send a message to start working with {label}.")),
                )
            })
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
            // With no items the centered closed card in the body already shows the message.
            Some(ChatConnectionState::Closed { message }) => {
                if self.timeline.items.is_empty() {
                    return div().into_any_element();
                }
                div()
                    .w_full()
                    .flex_none()
                    .flex()
                    .justify_center()
                    .px_4()
                    .pb_3()
                    .child(self.closed_card(message.as_deref()))
                    .into_any_element()
            }
            _ => div()
                .w_full()
                .flex_none()
                .child(self.composer(window, cx))
                .into_any_element(),
        }
    }
}
