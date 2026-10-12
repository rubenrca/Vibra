//! Timeline items for the virtualized chat list, and the bookkeeping that keeps
//! the GPUI list in step with `ChatTimeline::items`.

use gpui::{AnyElement, Context, Rgba, SharedString, div, prelude::*, px, relative, svg};

use super::AgentChatView;
use crate::domain::agent_chat::{
    ChatCommand, ChatEvent, ChatItem, ChatTimeline, NoticeLevel, PermissionItem,
    PermissionOptionKind, PermissionResolution, PlanEntry, PlanEntryStatus, ToolContent,
};
use crate::ui::markdown::markdown;
use crate::ui::theme::{colors, surface_tint};

/// Indices of existing items that `event` may change when applied.
///
/// Must run before `ChatTimeline::apply`. Items the event appends are not
/// listed; the caller learns about them from the item count.
pub(super) fn dirty_indices(timeline: &ChatTimeline, event: &ChatEvent) -> Vec<usize> {
    let items = &timeline.items;
    match event {
        ChatEvent::UserMessageChunk { .. }
        | ChatEvent::AgentMessageChunk { .. }
        | ChatEvent::ThoughtChunk { .. } => items.len().checked_sub(1).into_iter().collect(),
        ChatEvent::ToolCall(call) => position_of_tool(items, &call.id).into_iter().collect(),
        ChatEvent::ToolCallUpdate(update) => {
            position_of_tool(items, &update.id).into_iter().collect()
        }
        ChatEvent::Plan(_) => items
            .iter()
            .rposition(|item| matches!(item, ChatItem::Plan(_)))
            .into_iter()
            .collect(),
        ChatEvent::TerminalOutput { terminal_id, .. } => items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                matches!(item, ChatItem::ToolCall(call) if call.content.iter().any(|content| {
                    matches!(content, ToolContent::Terminal { terminal_id: id } if id == terminal_id)
                }))
            })
            .map(|(index, _)| index)
            .collect(),
        // Ending a turn cancels the open permission requests.
        ChatEvent::TurnFinished { .. } | ChatEvent::Exited { .. } => items
            .iter()
            .enumerate()
            .filter(|(_, item)| matches!(item, ChatItem::Permission(_)))
            .map(|(index, _)| index)
            .collect(),
        ChatEvent::Initialized { .. }
        | ChatEvent::SessionStarted { .. }
        | ChatEvent::PermissionRequested(_)
        | ChatEvent::Error { .. } => Vec::new(),
    }
}

fn position_of_tool(items: &[ChatItem], id: &str) -> Option<usize> {
    items
        .iter()
        .position(|item| matches!(item, ChatItem::ToolCall(call) if call.id == id))
}

impl AgentChatView {
    /// Brings the GPUI list in step with the timeline after a change. Splicing
    /// only the changed rows keeps a scrolled-up reader in place; the list stays
    /// pinned to the newest item while the reader is at the bottom.
    pub(super) fn sync_list(&mut self, before: usize, dirty: &[usize]) {
        let after = self.timeline.items.len();
        if self.list_state.item_count() != before || after < before {
            self.list_state.reset(after);
            return;
        }
        let mut dirty: Vec<usize> = dirty
            .iter()
            .copied()
            .filter(|index| *index < before)
            .collect();
        dirty.sort_unstable();
        dirty.dedup();
        for index in dirty {
            self.list_state.splice(index..index + 1, 1);
        }
        if after > before {
            self.list_state.splice(before..before, after - before);
        }
    }

    /// Expands or collapses a reasoning or tool card and re-measures its row.
    pub(super) fn toggle_expanded(&mut self, key: String, index: usize, cx: &mut Context<Self>) {
        if !self.expanded.remove(&key) {
            self.expanded.insert(key);
        }
        let count = self.timeline.items.len();
        self.sync_list(count, &[index]);
        cx.notify();
    }

    /// Answers a permission request from the card and tells the agent.
    pub(super) fn answer_permission(
        &mut self,
        request_id: u64,
        option_id: String,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        let resolution = PermissionResolution::Selected(option_id);
        if !self
            .timeline
            .resolve_permission(request_id, resolution.clone())
        {
            return;
        }
        self.send(
            ChatCommand::ResolvePermission {
                request_id,
                resolution,
            },
            cx,
        );
        let count = self.timeline.items.len();
        self.sync_list(count, &[index]);
        cx.notify();
    }

    /// One list row. The row wrapper owns the spacing between items.
    pub(super) fn render_item(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(item) = self.timeline.items.get(index) else {
            return div().into_any_element();
        };
        let content = match item {
            ChatItem::UserMessage { text } => user_message(text),
            ChatItem::AssistantMessage { text } => div()
                .w_full()
                .child(markdown(&format!("chat-assistant-{index}"), text))
                .into_any_element(),
            ChatItem::Reasoning { text } => self.reasoning(index, text, cx),
            ChatItem::ToolCall(call) => self.tool_card(index, call, cx),
            ChatItem::Plan(entries) => plan(entries),
            ChatItem::Permission(permission) => self.permission_card(index, permission, cx),
            ChatItem::Notice { level, text } => notice(*level, text),
        };
        div().w_full().pb_3().child(content).into_any_element()
    }

    fn reasoning(&self, index: usize, text: &str, cx: &mut Context<Self>) -> AnyElement {
        let key = format!("thought-{index}");
        let expanded = self.expanded.contains(&key);
        let header = div()
            .id(SharedString::from(format!("chat-thought-{index}")))
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .text_size(px(12.5))
            .text_color(colors().muted)
            .hover(|row| row.text_color(colors().foreground))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_expanded(key.clone(), index, cx);
            }))
            .child(
                svg()
                    .path(if expanded {
                        "chrome-icons/chevron-down.svg"
                    } else {
                        "chrome-icons/chevron-right.svg"
                    })
                    .size(px(12.0))
                    .flex_none()
                    .text_color(colors().subtle),
            )
            .child(
                svg()
                    .path("chrome-icons/sparkles.svg")
                    .size(px(13.0))
                    .flex_none()
                    .text_color(colors().subtle),
            )
            .child("Thinking");
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_2()
            .child(header)
            .when(expanded, |column| {
                column.child(
                    div()
                        .pl_5()
                        .border_l_1()
                        .border_color(colors().border_subtle)
                        .text_size(px(12.5))
                        .line_height(px(18.0))
                        .text_color(colors().muted)
                        .children(
                            text.lines()
                                .map(|line| div().min_h(px(18.0)).child(line.to_owned())),
                        ),
                )
            })
            .into_any_element()
    }

    fn permission_card(
        &self,
        index: usize,
        permission: &PermissionItem,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let resolved = permission.resolution.clone();
        let status = match &resolved {
            None => None,
            Some(PermissionResolution::Cancelled) => Some("Cancelled".to_owned()),
            Some(PermissionResolution::Selected(option_id)) => Some(
                permission
                    .options
                    .iter()
                    .find(|option| option.id == *option_id)
                    .map_or_else(|| option_id.clone(), |option| option.name.clone()),
            ),
        };
        let request_id = permission.request_id;
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded(px(8.0))
            .border_1()
            .border_color(colors().border_subtle)
            .bg(surface_tint(colors().elevated, colors().terminal))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(11.5))
                    .text_color(colors().warning)
                    .child(
                        svg()
                            .path("chrome-icons/shield.svg")
                            .size(px(13.0))
                            .flex_none()
                            .text_color(colors().warning),
                    )
                    .child("Permission requested"),
            )
            .child(
                div()
                    .text_size(px(12.5))
                    .line_height(px(18.0))
                    .text_color(colors().foreground)
                    .child(permission.title.clone()),
            )
            .map(|card| match status {
                Some(status) => card.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(colors().muted)
                        .child(format!("Answer: {status}")),
                ),
                None => card.child(
                    div().flex().flex_wrap().gap_2().children(
                        permission
                            .options
                            .iter()
                            .enumerate()
                            .map(|(option_index, option)| {
                                let option_id = option.id.clone();
                                let color = match option.kind {
                                    PermissionOptionKind::RejectOnce
                                    | PermissionOptionKind::RejectAlways => colors().danger,
                                    PermissionOptionKind::AllowOnce
                                    | PermissionOptionKind::AllowAlways => colors().foreground,
                                };
                                div()
                                    .id(SharedString::from(format!(
                                        "chat-permission-{index}-{option_index}"
                                    )))
                                    .px_3()
                                    .py_1()
                                    .rounded(px(5.0))
                                    .border_1()
                                    .border_color(colors().border_subtle)
                                    .bg(surface_tint(colors().selection, colors().elevated))
                                    .cursor_pointer()
                                    .text_size(px(12.0))
                                    .text_color(color)
                                    .hover(|button| button.bg(colors().hover))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.answer_permission(
                                            request_id,
                                            option_id.clone(),
                                            index,
                                            cx,
                                        );
                                    }))
                                    .child(option.name.clone())
                            }),
                    ),
                ),
            })
            .into_any_element()
    }
}

fn user_message(text: &str) -> AnyElement {
    div()
        .w_full()
        .flex()
        .justify_end()
        .child(
            div()
                .max_w(relative(0.85))
                .px_3()
                .py_2()
                .rounded(px(10.0))
                .bg(surface_tint(colors().selection, colors().terminal))
                .text_size(px(13.0))
                .line_height(px(20.0))
                .text_color(colors().foreground)
                .children(
                    text.lines()
                        .map(|line| div().min_h(px(20.0)).child(line.to_owned())),
                ),
        )
        .into_any_element()
}

fn plan(entries: &[PlanEntry]) -> AnyElement {
    let done = entries
        .iter()
        .filter(|entry| entry.status == PlanEntryStatus::Completed)
        .count();
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .rounded(px(8.0))
        .border_1()
        .border_color(colors().border_subtle)
        .child(
            div()
                .text_size(px(11.5))
                .text_color(colors().subtle)
                .child(format!("Plan · {done}/{} done", entries.len())),
        )
        .children(entries.iter().map(|entry| {
            let (icon, color) = match entry.status {
                PlanEntryStatus::Completed => (Some("chrome-icons/check.svg"), colors().success),
                PlanEntryStatus::InProgress => (Some("chrome-icons/refresh.svg"), colors().accent),
                PlanEntryStatus::Pending => (None, colors().subtle),
            };
            div()
                .flex()
                .items_start()
                .gap_2()
                .text_size(px(12.5))
                .line_height(px(18.0))
                .text_color(if entry.status == PlanEntryStatus::Completed {
                    colors().muted
                } else {
                    colors().foreground
                })
                .child(plan_marker(icon, color))
                .child(div().flex_1().min_w(px(0.0)).child(entry.content.clone()))
        }))
        .into_any_element()
}

fn plan_marker(icon: Option<&'static str>, color: Rgba) -> AnyElement {
    match icon {
        Some(path) => div()
            .w(px(13.0))
            .h(px(18.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .child(svg().path(path).size(px(12.0)).text_color(color))
            .into_any_element(),
        None => div()
            .w(px(13.0))
            .h(px(18.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .size(px(9.0))
                    .rounded(px(5.0))
                    .border_1()
                    .border_color(color),
            )
            .into_any_element(),
    }
}

fn notice(level: NoticeLevel, text: &str) -> AnyElement {
    match level {
        NoticeLevel::Info => div()
            .w_full()
            .px_1()
            .text_size(px(12.0))
            .line_height(px(18.0))
            .text_color(colors().subtle)
            .child(text.to_owned())
            .into_any_element(),
        NoticeLevel::Error => div()
            .w_full()
            .px_3()
            .py_2()
            .rounded(px(6.0))
            .bg(surface_tint(colors().danger, colors().terminal))
            .text_size(px(12.0))
            .line_height(px(18.0))
            .text_color(colors().foreground)
            .child(text.to_owned())
            .into_any_element(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::agent_chat::{ToolCallItem, ToolKind, ToolStatus};

    fn tool(id: &str, content: Vec<ToolContent>) -> ChatItem {
        ChatItem::ToolCall(ToolCallItem {
            id: id.to_owned(),
            title: id.to_owned(),
            kind: ToolKind::Execute,
            status: ToolStatus::InProgress,
            content,
            locations: Vec::new(),
        })
    }

    fn timeline_with(items: Vec<ChatItem>) -> ChatTimeline {
        let mut timeline = ChatTimeline::default();
        timeline.items = items;
        timeline
    }

    #[test]
    fn chunks_only_touch_the_last_item() {
        let timeline = timeline_with(vec![
            ChatItem::UserMessage { text: "hi".into() },
            ChatItem::AssistantMessage { text: "a".into() },
        ]);
        let chunk = ChatEvent::AgentMessageChunk { text: "b".into() };
        assert_eq!(dirty_indices(&timeline, &chunk), vec![1]);
        assert!(dirty_indices(&ChatTimeline::default(), &chunk).is_empty());
    }

    #[test]
    fn tool_updates_and_terminal_output_find_their_cards() {
        let timeline = timeline_with(vec![
            ChatItem::AssistantMessage { text: "a".into() },
            tool("t1", Vec::new()),
            tool(
                "t2",
                vec![ToolContent::Terminal {
                    terminal_id: "term".into(),
                }],
            ),
        ]);
        let update = ChatEvent::ToolCallUpdate(crate::domain::agent_chat::ToolCallUpdate {
            id: "t1".into(),
            ..Default::default()
        });
        assert_eq!(dirty_indices(&timeline, &update), vec![1]);
        let output = ChatEvent::TerminalOutput {
            terminal_id: "term".into(),
            output: "ok".into(),
            exit_status: None,
        };
        assert_eq!(dirty_indices(&timeline, &output), vec![2]);
    }

    #[test]
    fn ending_a_turn_revisits_open_permissions_only() {
        let permission = ChatItem::Permission(PermissionItem {
            request_id: 1,
            tool_call_id: None,
            title: "Run tests?".into(),
            options: Vec::new(),
            resolution: None,
        });
        let timeline = timeline_with(vec![
            ChatItem::UserMessage { text: "go".into() },
            permission,
        ]);
        let finished = ChatEvent::TurnFinished {
            stop_reason: "end_turn".into(),
        };
        assert_eq!(dirty_indices(&timeline, &finished), vec![1]);
        let started = ChatEvent::SessionStarted {
            session_id: "s".into(),
            loaded: false,
        };
        assert!(dirty_indices(&timeline, &started).is_empty());
    }
}
