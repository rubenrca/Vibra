//! Tool activity. A run of consecutive tool calls and thoughts is one row. Settled,
//! it is one summary line that expands into a line per call and per thought.
//! While the turn runs, it shows the newest thought and the current activity,
//! which pulses. A call line with content expands that content inline.

use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, Context, Rgba, SharedString, div, prelude::*,
    pulsating_between, px, svg,
};

use super::AgentChatView;
use super::diff;
use super::tooltip::tooltip;
use crate::domain::agent_chat::{ChatItem, ToolCallItem, ToolContent, ToolKind, ToolStatus};
use crate::ui::theme::{MONO_FONT, colors, surface_tint};

mod content;
mod labels;

use labels::{Activity, THOUGHT_ICON};

/// File chips shown per call before a "+N" count.
const MAX_CHIPS: usize = 3;

impl AgentChatView {
    /// One row for `items[start..end]`, a work run of tool calls and thoughts.
    /// Permission items are skipped, since the composer shows them. `live` while
    /// the run is the running turn's trailing activity.
    pub(super) fn work_row(
        &self,
        start: usize,
        end: usize,
        live: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let items = &self.timeline.items[start..end];
        let has_work = items
            .iter()
            .any(|item| matches!(item, ChatItem::ToolCall(_) | ChatItem::Reasoning { .. }));
        if !has_work {
            return div().into_any_element();
        }
        // The key comes from the first call, so it stays the same as the run grows.
        let key = match &self.timeline.items[start] {
            ChatItem::ToolCall(call) => group_key(&call.id),
            _ => format!("run-{start}"),
        };
        let expanded = self.expanded.contains(&key);
        let header = if live {
            self.live_header(start, items, key, expanded, cx)
        } else {
            self.settled_header(start, items, key, expanded, cx)
        };
        // Built only when expanded: a collapsed run should not lay out every entry.
        let body = expanded.then(|| self.run_entries(start, items, cx));
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            .child(header)
            .when_some(body, |column, body| column.child(body))
            .into_any_element()
    }

    /// A settled run: one 24px line with its icon, summary and chevron. A click
    /// expands or collapses it.
    fn settled_header(
        &self,
        start: usize,
        items: &[ChatItem],
        key: String,
        expanded: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let summary = labels::summarize(items);
        let icon_color = if summary.failed {
            colors().danger
        } else {
            colors().muted
        };
        div()
            .id(SharedString::from(format!("chat-group-{start}")))
            .w_full()
            .h(px(24.0))
            .flex()
            .items_center()
            .gap_2()
            .min_w(px(0.0))
            .cursor_pointer()
            .text_size(px(13.0))
            .text_color(colors().muted)
            .hover(|row| row.text_color(colors().foreground))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_expanded(key.clone(), start, cx);
            }))
            .child(icon(labels::summary_icon(items), icon_color))
            .child(div().flex_1().min_w(px(0.0)).truncate().child(summary.text))
            .child(icon(chevron(expanded), colors().subtle))
            .into_any_element()
    }

    /// A live run: the newest thought (while collapsed), then the current
    /// activity. The activity pulses while the turn runs and stays still when
    /// its call failed. A click expands or collapses the run.
    fn live_header(
        &self,
        start: usize,
        items: &[ChatItem],
        key: String,
        expanded: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let preview = (!expanded)
            .then(|| labels::latest_thought(items).map(labels::thought_preview))
            .flatten()
            .filter(|text| !text.is_empty());
        let (path, label, failed) = match labels::activity(items) {
            Activity::Thinking => (THOUGHT_ICON, "Thinking".to_owned(), false),
            Activity::Call(call) => (
                kind_icon(call.kind),
                labels::call_label(call, true),
                call.status == ToolStatus::Failed,
            ),
        };
        let line = div()
            .flex()
            .items_center()
            .gap_2()
            .min_w(px(0.0))
            .min_h(px(24.0))
            .text_size(px(13.0))
            .text_color(if failed {
                colors().danger
            } else {
                colors().foreground
            })
            .child(icon(
                path,
                if failed {
                    colors().danger
                } else {
                    colors().accent
                },
            ))
            .child(div().flex_1().min_w(px(0.0)).truncate().child(label));
        let activity = if failed {
            line.into_any_element()
        } else {
            line.with_animation(
                ("chat-live-activity", start),
                Animation::new(Duration::from_millis(1600))
                    .repeat()
                    .with_easing(pulsating_between(0.45, 1.0)),
                |row, delta| row.opacity(delta),
            )
            .into_any_element()
        };
        div()
            .id(SharedString::from(format!("chat-group-{start}")))
            .w_full()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_expanded(key.clone(), start, cx);
            }))
            .when_some(preview, |column, text| {
                column.child(thought_preview_row(text))
            })
            .child(activity)
            .into_any_element()
    }

    /// The expanded run: one line per call and per thought, indented so the
    /// icons sit under the summary's label.
    fn run_entries(&self, start: usize, items: &[ChatItem], cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .pl(px(13.0))
            .children(items.iter().enumerate().filter_map(|(offset, item)| {
                let index = start + offset;
                match item {
                    ChatItem::ToolCall(call) => Some(self.tool_line(index, start, call, cx)),
                    ChatItem::Reasoning { text } => Some(self.thought_line(index, start, text, cx)),
                    _ => None,
                }
            }))
            .into_any_element()
    }

    /// One call inside an expanded run: its kind, what it did, and its status.
    /// A line with content toggles that content inline; `group` is the run's
    /// leader row, which draws the line.
    fn tool_line(
        &self,
        index: usize,
        group: usize,
        call: &ToolCallItem,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let has_content = !call.content.is_empty();
        let expanded = has_content && self.expanded.contains(&call.id);
        let key = call.id.clone();
        let (added, removed) = diff_totals(&call.content);
        let failed = call.status == ToolStatus::Failed;
        let detail = labels::command_detail(call);
        let has_detail = detail.is_some();
        let line = div()
            .id(SharedString::from(format!("chat-tool-{index}")))
            .flex()
            .items_center()
            .gap_2()
            .min_w(px(0.0))
            .px_2()
            .py(px(3.0))
            .rounded(px(6.0))
            .text_size(px(12.0))
            .when(has_content, |row| {
                row.cursor_pointer()
                    .hover(|row| row.bg(colors().hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_expanded(key.clone(), group, cx);
                    }))
            })
            .child(icon(kind_icon(call.kind), colors().muted))
            .child(
                div()
                    .flex_shrink()
                    .min_w(px(0.0))
                    .truncate()
                    .text_color(if failed {
                        colors().danger
                    } else {
                        colors().foreground
                    })
                    .child(labels::call_label(call, false)),
            )
            .when_some(detail, |row, detail| {
                row.child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate()
                        .font_family(MONO_FONT)
                        .text_size(px(11.5))
                        .text_color(colors().subtle)
                        .child(detail),
                )
            })
            .children(file_chips(index, labels::extra_locations(call)))
            .when(!has_detail, |row| row.child(div().flex_1()))
            .when(added + removed > 0, |row| {
                row.child(diff_counts(added, removed))
            })
            .children(status_icon(call.status))
            .when(has_content, |row| {
                row.child(icon(chevron(expanded), colors().subtle))
            });
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            .child(line)
            .when(expanded, |column| column.child(self.tool_body(index, call)))
            .into_any_element()
    }

    /// One thought inside an expanded run: its first line, and the full text
    /// once clicked.
    fn thought_line(
        &self,
        index: usize,
        group: usize,
        text: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = format!("thought-{index}");
        let expanded = self.expanded.contains(&key);
        let line = div()
            .id(SharedString::from(format!("chat-thought-{index}")))
            .flex()
            .items_center()
            .gap_2()
            .min_w(px(0.0))
            .px_2()
            .py(px(3.0))
            .rounded(px(6.0))
            .cursor_pointer()
            .text_size(px(12.5))
            .text_color(colors().muted)
            .hover(|row| row.bg(colors().hover))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_expanded(key.clone(), group, cx);
            }))
            .child(icon(THOUGHT_ICON, colors().muted))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .child(labels::first_line(text).to_owned()),
            )
            .child(icon(chevron(expanded), colors().subtle));
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            .child(line)
            .when(expanded, |column| column.child(thought_text(text)))
            .into_any_element()
    }

    fn tool_body(&self, index: usize, call: &ToolCallItem) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .ml(px(12.0))
            .text_size(px(12.0))
            .text_color(colors().muted)
            .children(
                call.content
                    .iter()
                    .enumerate()
                    .map(|(content_index, content)| {
                        self.tool_content(index, content_index, content)
                    }),
            )
            .into_any_element()
    }
}

/// Whether a call is still going, so its run shows as active.
pub(super) fn is_running(status: ToolStatus) -> bool {
    matches!(status, ToolStatus::Pending | ToolStatus::InProgress)
}

/// Stable key for a run's expansion. The prefix keeps it apart from call ids.
fn group_key(first_call_id: &str) -> String {
    format!("group:{first_call_id}")
}

/// Name for a file chip: the last path component.
pub(super) fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(path)
}

/// Added and removed line counts over a call's diffs.
fn diff_totals(content: &[ToolContent]) -> (usize, usize) {
    content
        .iter()
        .fold((0, 0), |(added, removed), content| match content {
            ToolContent::Diff {
                old_text, new_text, ..
            } => {
                let (more_added, more_removed) =
                    diff::line_counts(&diff::changed_lines(old_text.as_deref(), new_text));
                (added + more_added, removed + more_removed)
            }
            ToolContent::Text(_) | ToolContent::Terminal { .. } => (added, removed),
        })
}

pub(super) fn icon(path: &'static str, color: Rgba) -> AnyElement {
    svg()
        .path(path)
        .size(px(13.0))
        .flex_none()
        .text_color(color)
        .into_any_element()
}

fn chevron(expanded: bool) -> &'static str {
    if expanded {
        "chrome-icons/chevron-down.svg"
    } else {
        "chrome-icons/chevron-right.svg"
    }
}

fn kind_icon(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => "chrome-icons/eye.svg",
        ToolKind::Edit => "chrome-icons/square-pen.svg",
        ToolKind::Delete => "chrome-icons/trash.svg",
        ToolKind::Move => "chrome-icons/folder-open.svg",
        ToolKind::Search => "chrome-icons/search.svg",
        ToolKind::Execute => "chrome-icons/terminal.svg",
        ToolKind::Think => THOUGHT_ICON,
        ToolKind::Fetch => "chrome-icons/globe.svg",
        ToolKind::SwitchMode => "chrome-icons/settings.svg",
        ToolKind::Other => "chrome-icons/wrench.svg",
    }
}

/// Running calls spin, failed calls are marked, and finished calls stay quiet.
fn status_icon(status: ToolStatus) -> Option<AnyElement> {
    match status {
        ToolStatus::Pending | ToolStatus::InProgress => {
            Some(icon("chrome-icons/refresh.svg", colors().accent))
        }
        ToolStatus::Failed => Some(icon("chrome-icons/close.svg", colors().danger)),
        ToolStatus::Completed => None,
    }
}

/// "+N −M" for the edits in a call.
fn diff_counts(added: usize, removed: usize) -> AnyElement {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap_1()
        .font_family(MONO_FONT)
        .text_size(px(11.5))
        .child(
            div()
                .text_color(colors().diff_added)
                .child(format!("+{added}")),
        )
        .child(
            div()
                .text_color(colors().diff_deleted)
                .child(format!("\u{2212}{removed}")),
        )
        .into_any_element()
}

/// The newest thought above the live activity, wrapped to four lines.
fn thought_preview_row(text: String) -> AnyElement {
    div()
        .w_full()
        .flex()
        .items_start()
        .gap_2()
        .min_w(px(0.0))
        .text_size(px(12.5))
        .text_color(colors().muted)
        .child(icon(THOUGHT_ICON, colors().muted))
        .child(div().flex_1().min_w(px(0.0)).line_clamp(4).child(text))
        .into_any_element()
}

/// A whole thought, one row per line so its line breaks show.
fn thought_text(text: &str) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .ml(px(12.0))
        .text_size(px(12.5))
        .line_height(px(18.0))
        .text_color(colors().muted)
        .children(
            text.trim()
                .lines()
                .map(|line| div().min_h(px(18.0)).child(line.to_owned())),
        )
        .into_any_element()
}

/// One rounded chip per touched file, then a "+N" count for the rest.
fn file_chips(index: usize, locations: &[String]) -> Vec<AnyElement> {
    let mut chips: Vec<AnyElement> = locations
        .iter()
        .take(MAX_CHIPS)
        .enumerate()
        .map(|(chip, path)| file_chip(format!("chat-file-{index}-{chip}"), path))
        .collect();
    if locations.len() > MAX_CHIPS {
        chips.push(
            div()
                .flex_none()
                .text_size(px(11.5))
                .text_color(colors().subtle)
                .child(format!("+{}", locations.len() - MAX_CHIPS))
                .into_any_element(),
        );
    }
    chips
}

fn file_chip(id: String, path: &str) -> AnyElement {
    let full_path = path.to_owned();
    div()
        .id(SharedString::from(id))
        .flex_none()
        .flex()
        .items_center()
        .gap_1()
        .px(px(5.0))
        .py(px(1.0))
        .rounded(px(6.0))
        .bg(surface_tint(colors().elevated, colors().terminal))
        .font_family(MONO_FONT)
        .text_size(px(11.5))
        .text_color(colors().foreground)
        .tooltip(move |_, cx| tooltip(full_path.clone(), cx))
        .child(
            svg()
                .path("file-icons/file.svg")
                .size(px(11.0))
                .flex_none()
                .text_color(colors().muted),
        )
        .child(file_name(path).to_owned())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_chips_show_the_last_path_component() {
        assert_eq!(file_name("/repo/src/ui/a.rs"), "a.rs");
        assert_eq!(file_name("a.rs"), "a.rs");
        assert_eq!(file_name("dir/"), "dir");
        assert_eq!(file_name("C:\\work\\b.txt"), "b.txt");
    }

    #[test]
    fn only_pending_and_running_calls_are_running() {
        assert!(is_running(ToolStatus::Pending));
        assert!(is_running(ToolStatus::InProgress));
        assert!(!is_running(ToolStatus::Completed));
        assert!(!is_running(ToolStatus::Failed));
    }

    #[test]
    fn edit_counts_add_up_over_diffs() {
        let content = vec![
            ToolContent::Diff {
                path: "a.rs".into(),
                old_text: Some("a\nb".into()),
                new_text: "a\nc".into(),
            },
            ToolContent::Text("ignored".into()),
            ToolContent::Diff {
                path: "b.rs".into(),
                old_text: None,
                new_text: "x".into(),
            },
        ];
        assert_eq!(diff_totals(&content), (2, 1));
    }
}
