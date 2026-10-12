//! Chrome around the timeline: the list, the empty and closed states, and the dock that holds the composer. The title lives in the
//! pane's tab, so the view has no header of its own.

pub(super) mod working;

use gpui::{AnyElement, Context, MouseButton, Render, Window, div, list, prelude::*, px, svg};

use super::AgentChatView;
use super::tooltip::tooltip;
use crate::domain::agent_chat::{ChatAgent, ChatConnectionState};
use crate::ui::theme::{MONO_FONT, colors, surface};

/// Width of the transcript, the empty-state column and the composer dock. The
/// composer keeps this width when the first message moves it to the dock.
pub(super) const COLUMN_WIDTH: f32 = 736.0;

/// Shown for a closed session that has no reason to give.
const CLOSED_MESSAGE: &str = "The agent session ended.";

impl Render for AgentChatView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let typing = self.focus_handle.is_focused(window) && self.accepts_input();
        self.sync_editor(typing, cx);
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
            .child(self.body(window, cx))
            .child(self.footer(window, cx))
    }
}

impl AgentChatView {
    fn body(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        if self.timeline.items.is_empty() {
            return if self.is_closed() {
                self.closed_body()
            } else {
                self.start_body(window, cx)
            };
        }
        div()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .overflow_hidden()
                    .px_4()
                    .pt_4()
                    .child(
                        list(
                            self.list_state.clone(),
                            cx.processor(|this, index: usize, _, cx| this.render_item(index, cx)),
                        )
                        .size_full(),
                    )
                    .when(
                        self.scrolled_away
                            && self.list_state.logical_scroll_top().item_ix
                                < self.list_state.item_count(),
                        |body| body.child(self.jump_to_latest_button(cx)),
                    ),
            )
            .into_any_element()
    }

    /// A quiet floating control, shown only while the reader is above the tail.
    fn jump_to_latest_button(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .absolute()
            .bottom(px(12.0))
            .left_0()
            .w_full()
            .flex()
            .justify_center()
            .child(
                div()
                    .id("agent-chat-jump-latest")
                    .size(px(30.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .border_1()
                    .border_color(colors().border_subtle)
                    .bg(colors().elevated)
                    .cursor_pointer()
                    .hover(|button| button.bg(colors().hover))
                    .tooltip(|_, cx| tooltip("Jump to latest", cx))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, _, cx| this.jump_to_latest(cx)))
                    .child(
                        svg()
                            .path("chrome-icons/chevron-down.svg")
                            .size(px(16.0))
                            .text_color(colors().muted),
                    ),
            )
            .into_any_element()
    }

    /// An empty chat shows only the composer, including while connecting.
    fn start_body(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .px_4()
            .child(
                div()
                    .w_full()
                    .max_w(px(COLUMN_WIDTH))
                    .child(self.composer(window, cx)),
            )
            .into_any_element()
    }

    /// Closed before any message: the reason, centered. The composer docks below.
    fn closed_body(&self) -> AnyElement {
        div()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .px_4()
            .child(self.closed_notice(self.closed_message().unwrap_or(CLOSED_MESSAGE)))
            .into_any_element()
    }

    /// The agent mark, the reason the chat is closed, and where to go next.
    /// Muted text and no card, so it reads as a status rather than an error box.
    fn closed_notice(&self, message: &str) -> AnyElement {
        let multiline = message.contains('\n');
        div()
            .w_full()
            .max_w(px(560.0))
            .flex()
            .flex_col()
            .items_center()
            .gap_3()
            .child(
                svg()
                    .path(agent_mark_path(self.agent))
                    .size(px(24.0))
                    .flex_none()
                    .text_color(colors().subtle),
            )
            .child(if multiline {
                // Logs keep their layout, so they stay left-aligned and scroll.
                div()
                    .id("agent-chat-closed-message")
                    .w_full()
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
                    .text_center()
                    .text_size(px(13.0))
                    .line_height(px(19.0))
                    .text_color(colors().muted)
                    .child(message.to_owned())
                    .into_any_element()
            })
            .into_any_element()
    }

    /// The reason the chat closed, or `None` while it is open.
    fn closed_message(&self) -> Option<&str> {
        match &self.timeline.state {
            Some(ChatConnectionState::Closed { message }) => {
                Some(message.as_deref().unwrap_or(CLOSED_MESSAGE))
            }
            _ => None,
        }
    }

    /// The dock under the list: the composer, centered at the transcript width.
    /// Empty and open chats skip it, because their composer is in the empty state.
    fn footer(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let closed = self.is_closed();
        if self.timeline.items.is_empty() && !closed {
            return div().into_any_element();
        }
        // A closed chat with messages says why above the composer, so the
        // transcript stays readable.
        let notice = match self.closed_message() {
            Some(message) if !self.timeline.items.is_empty() => Some(
                div()
                    .w_full()
                    .px_3()
                    .pt_3()
                    .flex()
                    .justify_center()
                    .child(self.closed_notice(message)),
            ),
            _ => None,
        };
        div()
            .w_full()
            .flex_none()
            .flex()
            .justify_center()
            .px_4()
            .child(
                div()
                    .w_full()
                    .max_w(px(COLUMN_WIDTH))
                    .flex()
                    .flex_col()
                    .children(notice)
                    .child(self.composer(window, cx)),
            )
            .into_any_element()
    }
}

/// Bundled mark of `agent`, for example "agent-marks/claude.svg".
fn agent_mark_path(agent: ChatAgent) -> String {
    format!("agent-marks/{}.svg", agent.label().to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::agent_marks::VibraAssets;
    use gpui::AssetSource;

    #[test]
    fn every_agent_has_a_bundled_mark_at_its_path() {
        for agent in ChatAgent::ALL {
            let path = agent_mark_path(agent);
            let asset = VibraAssets.load(&path).expect("asset source");
            assert!(asset.is_some(), "missing bundled mark {path}");
        }
        assert_eq!(agent_mark_path(ChatAgent::Codex), "agent-marks/codex.svg");
    }
}
