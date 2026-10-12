//! Tool activity. Consecutive tool calls form one collapsible group row whose
//! label says what the agent is doing. Expanded, each call is a compact line
//! (verb, title, file chips, status); a line with content expands inline.

use gpui::{AnyElement, Context, Rgba, SharedString, div, prelude::*, px, svg};

use super::AgentChatView;
use super::diff;
use super::tooltip::tooltip;
use crate::domain::agent_chat::{ChatItem, ToolCallItem, ToolContent, ToolKind, ToolStatus};
use crate::ui::theme::{MONO_FONT, colors, surface_tint};

mod content;

/// File chips shown per call before a "+N" count.
const MAX_CHIPS: usize = 3;

impl AgentChatView {
    /// One row for `items[start..end]`, the tool calls of a group. Collapsed it
    /// shows the summary; expanded, one line per call.
    pub(super) fn tool_group(
        &self,
        start: usize,
        end: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let calls: Vec<(usize, &ToolCallItem)> = self.timeline.items[start..end]
            .iter()
            .enumerate()
            .filter_map(|(offset, item)| match item {
                ChatItem::ToolCall(call) => Some((start + offset, call)),
                _ => None,
            })
            .collect();
        let (Some(&(_, first)), Some(&(_, last))) = (calls.first(), calls.last()) else {
            return div().into_any_element();
        };
        // The key comes from the first call, so it stays the same as the group grows.
        let key = group_key(&first.id);
        let expanded = self.expanded.contains(&key);
        let running: Vec<&ToolCallItem> = calls
            .iter()
            .map(|(_, call)| *call)
            .filter(|call| is_running(call.status))
            .collect();
        let active = self.is_working() && !running.is_empty();
        let kinds: Vec<ToolKind> = calls.iter().map(|(_, call)| call.kind).collect();
        let trailing = match running.last() {
            Some(call) if active => div()
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .font_family(MONO_FONT)
                .text_size(px(11.5))
                .text_color(colors().subtle)
                .child(call.title.clone())
                .into_any_element(),
            _ => div()
                .flex_none()
                .text_size(px(11.5))
                .text_color(colors().subtle)
                .child(step_count(calls.len()))
                .into_any_element(),
        };
        let header = div()
            .id(SharedString::from(format!("chat-group-{start}")))
            .flex()
            .items_center()
            .gap_2()
            .min_w(px(0.0))
            .py(px(2.0))
            .cursor_pointer()
            .text_size(px(12.5))
            .text_color(if active {
                colors().foreground
            } else {
                colors().muted
            })
            .hover(|row| row.text_color(colors().foreground))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_expanded(key.clone(), start, cx);
            }))
            .child(icon(
                if expanded {
                    "chrome-icons/chevron-down.svg"
                } else {
                    "chrome-icons/chevron-right.svg"
                },
                colors().subtle,
            ))
            .child(icon(
                kind_icon(last.kind),
                if active {
                    colors().accent
                } else {
                    colors().muted
                },
            ))
            .child(div().flex_none().child(group_label(&kinds)))
            .when(active, |row| {
                row.child(icon("chrome-icons/refresh.svg", colors().accent))
            })
            .child(trailing);
        // Built only when expanded: a collapsed group should not lay out every call.
        let body = expanded.then(|| {
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .ml(px(6.0))
                .pl_3()
                .border_l_1()
                .border_color(colors().border_subtle)
                .children(
                    calls
                        .iter()
                        .map(|(index, call)| self.tool_line(*index, start, call, cx)),
                )
        });
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            .child(header)
            .when_some(body, |group, body| group.child(body))
            .into_any_element()
    }

    /// One call inside an expanded group. A line with content toggles its
    /// content inline; `group` is the row that draws the line.
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
        let title = title_text(call).map(str::to_owned);
        let line = div()
            .id(SharedString::from(format!("chat-tool-{index}")))
            .flex()
            .items_center()
            .gap_2()
            .min_w(px(0.0))
            .px_2()
            .py(px(3.0))
            .rounded(px(5.0))
            .text_size(px(12.0))
            .when(call.status == ToolStatus::Failed, |row| {
                row.bg(surface_tint(colors().danger, colors().terminal))
            })
            .when(has_content, |row| {
                row.cursor_pointer()
                    .hover(|row| row.bg(colors().hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_expanded(key.clone(), group, cx);
                    }))
            })
            .child(
                div()
                    .flex_none()
                    .text_color(colors().muted)
                    .child(verb(call.kind)),
            )
            .when_some(title, |row, title| {
                row.child(
                    div()
                        .flex_shrink()
                        .min_w(px(0.0))
                        .truncate()
                        .font_family(MONO_FONT)
                        .text_color(colors().foreground)
                        .child(title),
                )
            })
            .children(file_chips(index, &call.locations))
            .child(div().flex_1())
            .when(added + removed > 0, |row| {
                row.child(diff_counts(added, removed))
            })
            .children(status_icon(call.status))
            .when(has_content, |row| {
                row.child(icon(
                    if expanded {
                        "chrome-icons/chevron-down.svg"
                    } else {
                        "chrome-icons/chevron-right.svg"
                    },
                    colors().subtle,
                ))
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

/// Verb that starts a tool line.
pub(super) fn verb(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => "Read",
        ToolKind::Edit => "Edit",
        ToolKind::Delete => "Delete",
        ToolKind::Move => "Move",
        ToolKind::Search => "Find",
        ToolKind::Execute => "Run",
        ToolKind::Think => "Think",
        ToolKind::Fetch => "Fetch",
        ToolKind::SwitchMode => "Switch mode",
        ToolKind::Other => "Tool",
    }
}

/// Summary of a group: what the agent is doing with its calls.
pub(super) fn group_label(kinds: &[ToolKind]) -> &'static str {
    let edits = kinds
        .iter()
        .any(|kind| matches!(kind, ToolKind::Edit | ToolKind::Delete | ToolKind::Move));
    if edits {
        "Editing files"
    } else if kinds
        .iter()
        .all(|kind| matches!(kind, ToolKind::Read | ToolKind::Search))
    {
        "Exploring the project"
    } else if kinds.iter().all(|kind| *kind == ToolKind::Execute) {
        "Running commands"
    } else {
        "Working"
    }
}

/// Whether a call is still going, so its group shows as active.
pub(super) fn is_running(status: ToolStatus) -> bool {
    matches!(status, ToolStatus::Pending | ToolStatus::InProgress)
}

/// One-line summary of a call, for example in a permission request.
pub(super) fn call_summary(call: &ToolCallItem) -> String {
    format!("{} {}", verb(call.kind), call.title)
}

/// Settled group summary with its call count, for example "3 steps".
fn step_count(count: usize) -> String {
    if count == 1 {
        "1 step".to_owned()
    } else {
        format!("{count} steps")
    }
}

/// Stable key for a group's expansion. The prefix keeps it apart from call ids.
fn group_key(first_call_id: &str) -> String {
    format!("group:{first_call_id}")
}

/// The argument shown after the verb. A title that a location already names
/// is left out, because the file chip shows it.
fn title_text(call: &ToolCallItem) -> Option<&str> {
    let title = call.title.trim();
    let named_by_location = call
        .locations
        .iter()
        .any(|path| path == title || path.ends_with(title));
    (!title.is_empty() && !named_by_location).then_some(title)
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

fn kind_icon(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => "chrome-icons/files.svg",
        ToolKind::Edit => "chrome-icons/pencil.svg",
        ToolKind::Delete => "chrome-icons/trash.svg",
        ToolKind::Move => "chrome-icons/folder-open.svg",
        ToolKind::Search => "chrome-icons/search.svg",
        ToolKind::Execute => "chrome-icons/terminal.svg",
        ToolKind::Think => "chrome-icons/sparkles.svg",
        ToolKind::Fetch => "chrome-icons/open-external.svg",
        ToolKind::SwitchMode => "chrome-icons/settings.svg",
        ToolKind::Other => "chrome-icons/ellipsis.svg",
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
        .rounded(px(5.0))
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
    fn group_label_follows_what_the_calls_do() {
        assert_eq!(
            group_label(&[ToolKind::Read, ToolKind::Search]),
            "Exploring the project"
        );
        assert_eq!(
            group_label(&[ToolKind::Read, ToolKind::Edit]),
            "Editing files"
        );
        assert_eq!(
            group_label(&[ToolKind::Execute, ToolKind::Execute]),
            "Running commands"
        );
        assert_eq!(group_label(&[ToolKind::Execute, ToolKind::Read]), "Working");
        assert_eq!(group_label(&[ToolKind::Think]), "Working");
    }

    #[test]
    fn file_chips_show_the_last_path_component() {
        assert_eq!(file_name("/repo/src/ui/a.rs"), "a.rs");
        assert_eq!(file_name("a.rs"), "a.rs");
        assert_eq!(file_name("dir/"), "dir");
        assert_eq!(file_name("C:\\work\\b.txt"), "b.txt");
    }

    #[test]
    fn step_counts_are_singular_for_one_call() {
        assert_eq!(step_count(1), "1 step");
        assert_eq!(step_count(3), "3 steps");
    }

    #[test]
    fn a_title_already_shown_as_a_location_is_not_repeated() {
        let mut call = ToolCallItem {
            id: "c".into(),
            title: "src/main.rs".into(),
            kind: ToolKind::Read,
            status: ToolStatus::Completed,
            content: Vec::new(),
            locations: vec!["/repo/src/main.rs".into()],
        };
        assert_eq!(title_text(&call), None);
        call.title = "^export type Block =".into();
        assert_eq!(title_text(&call), Some("^export type Block ="));
    }

    #[test]
    fn verbs_name_the_kind_of_work() {
        assert_eq!(verb(ToolKind::Search), "Find");
        assert_eq!(verb(ToolKind::Execute), "Run");
        assert_eq!(verb(ToolKind::Fetch), "Fetch");
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
