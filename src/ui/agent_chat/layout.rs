//! Chrome around the timeline: the list, the empty and closed states, and the dock that holds the composer. The title lives in the
//! pane's tab, so the view has no header of its own.

pub(super) mod working;

use std::path::Path;

use gpui::{
    AnyElement, Context, FontWeight, MouseButton, Render, Window, div, list, prelude::*, px, svg,
};

use super::AgentChatView;
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
                    ),
            )
            .into_any_element()
    }

    /// Empty state before the first message: a centered column with the
    /// heading, and the composer right under it.
    fn start_body(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let starting = matches!(
            self.timeline.state,
            None | Some(ChatConnectionState::Starting)
        );
        let project = project_name(
            self.file_search
                .as_ref()
                .map(|search| search.root.as_path()),
        );
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
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .w_full()
                            .min_w(px(0.0))
                            .px_3()
                            .truncate()
                            .text_size(px(20.0))
                            .font_weight(FontWeight::MEDIUM)
                            .child(start_heading(project)),
                    )
                    .child(self.composer(window, cx))
                    .when(starting, |column| {
                        column.child(
                            div()
                                .px_3()
                                .text_size(px(12.0))
                                .text_color(colors().muted)
                                .child(format!("Starting {}…", self.agent.label())),
                        )
                    }),
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
            .when(
                self.session_id().is_some() && !self.has_conversation(),
                |column| {
                    column.child(
                        div()
                            .text_center()
                            .text_size(px(12.0))
                            .text_color(colors().subtle)
                            .child("Use Continue in Terminal to keep working in the terminal."),
                    )
                },
            )
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

/// Last path component of the chat's folder, which names the project in the heading.
fn project_name(folder: Option<&Path>) -> Option<&str> {
    folder?.file_name()?.to_str()
}

/// Heading of the empty state, with the project when there is one.
fn start_heading(project: Option<&str>) -> String {
    match project {
        Some(project) => format!("What should we work on in {project}?"),
        None => "What should we work on?".to_owned(),
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
    fn the_heading_names_the_project_folder() {
        assert_eq!(
            start_heading(project_name(Some(Path::new("/Users/me/Dev/Vibra")))),
            "What should we work on in Vibra?"
        );
        assert_eq!(
            start_heading(project_name(Some(Path::new("/Users/me/Dev/Vibra/")))),
            "What should we work on in Vibra?"
        );
    }

    #[test]
    fn the_heading_falls_back_when_there_is_no_folder_name() {
        assert_eq!(project_name(None), None);
        assert_eq!(project_name(Some(Path::new("/"))), None);
        assert_eq!(start_heading(None), "What should we work on?");
    }

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
