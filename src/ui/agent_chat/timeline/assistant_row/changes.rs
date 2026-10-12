//! The changed-files card under a turn's final answer: each file the turn's
//! tool calls edited, with its added and removed lines. A row unfolds that
//! file's diff inline. Counts add up over the edits to one path, like the tool
//! cards' totals, and the diff shows every edit in order.

use std::path::Path;

use gpui::{AnyElement, Context, Div, FontWeight, SharedString, div, prelude::*, px, svg};

use super::super::super::AgentChatView;
use super::super::super::diff::{self, DiffKind, DiffLine};
use super::super::super::tooltip::tooltip;
use super::super::super::turns::turn_range;
use crate::domain::agent_chat::{ChatItem, ToolContent};
use crate::ui::theme::{MONO_FONT, colors, surface_tint};

/// Diff lines drawn per file, so a rewrite of a huge file stays cheap to lay out.
const MAX_DIFF_LINES: usize = 400;

/// One file the turn changed: the changed lines of each edit, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ChangedFile {
    /// The path as the agent sent it.
    pub path: String,
    /// Changed lines per edit. An edit that changes no lines is left out.
    pub edits: Vec<Vec<DiffLine>>,
}

impl ChangedFile {
    /// Added and removed lines over every edit.
    pub fn counts(&self) -> (usize, usize) {
        self.edits.iter().fold((0, 0), |(added, removed), lines| {
            let (more_added, more_removed) = diff::line_counts(lines);
            (added + more_added, removed + more_removed)
        })
    }
}

/// Files the items' tool calls edited, merged by path in first-seen order.
/// Edits that change no lines do not make a file appear.
pub(super) fn changed_files(items: &[ChatItem]) -> Vec<ChangedFile> {
    let mut files: Vec<ChangedFile> = Vec::new();
    for item in items {
        let ChatItem::ToolCall(call) = item else {
            continue;
        };
        for content in &call.content {
            let ToolContent::Diff {
                path,
                old_text,
                new_text,
            } = content
            else {
                continue;
            };
            let lines = diff::changed_lines(old_text.as_deref(), new_text);
            if lines.is_empty() {
                continue;
            }
            match files.iter_mut().find(|file| file.path == *path) {
                Some(file) => file.edits.push(lines),
                None => files.push(ChangedFile {
                    path: path.clone(),
                    edits: vec![lines],
                }),
            }
        }
    }
    files
}

/// `path` relative to the project `root` when it lies inside it, else `path` as sent.
pub(super) fn display_path(root: Option<&Path>, path: &str) -> String {
    root.and_then(|root| Path::new(path).strip_prefix(root).ok())
        .filter(|relative| !relative.as_os_str().is_empty())
        .map_or_else(
            || path.to_owned(),
            |relative| relative.to_string_lossy().into_owned(),
        )
}

/// "1 changed file" or "N changed files".
fn files_label(count: usize) -> String {
    match count {
        1 => "1 changed file".to_owned(),
        count => format!("{count} changed files"),
    }
}

impl AgentChatView {
    /// The card for `turn`'s changed files, or `None` when its tool calls edited
    /// none. `index` is the answer's row, which an unfolded diff re-measures.
    pub(super) fn changed_files_card(
        &self,
        index: usize,
        turn: usize,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let files = changed_files(&self.timeline.items[turn_range(&self.timeline, turn)]);
        if files.is_empty() {
            return None;
        }
        let root = self
            .file_search
            .as_ref()
            .map(|search| search.root.as_path());
        let (added, removed) = files.iter().fold((0, 0), |(added, removed), file| {
            let (more_added, more_removed) = file.counts();
            (added + more_added, removed + more_removed)
        });
        let header = div()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py(px(8.0))
            .text_size(px(12.0))
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors().foreground)
                    .child(files_label(files.len())),
            )
            .child(
                div()
                    .text_color(colors().success)
                    .child(format!("+{added}")),
            )
            .child(
                div()
                    .text_color(colors().danger)
                    .child(format!("\u{2212}{removed}")),
            );
        Some(
            div()
                .flex()
                .flex_col()
                .overflow_hidden()
                .rounded(px(10.0))
                .bg(surface_tint(colors().elevated, colors().terminal))
                .child(header)
                .children(
                    files
                        .iter()
                        .map(|file| self.changed_file(index, turn, root, file, cx)),
                )
                .into_any_element(),
        )
    }

    /// One file's row: icon, path and counts. Clicking it unfolds the diff below.
    fn changed_file(
        &self,
        index: usize,
        turn: usize,
        root: Option<&Path>,
        file: &ChangedFile,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = format!("changes-{turn}-{}", file.path);
        let expanded = self.expanded.contains(&key);
        let (added, removed) = file.counts();
        let full_path = file.path.clone();
        let row = div()
            .id(SharedString::from(key.clone()))
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py(px(4.0))
            .min_w(px(0.0))
            .cursor_pointer()
            .hover(|row| row.bg(colors().hover))
            .tooltip(move |_, cx| tooltip(full_path.clone(), cx))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_expanded(key.clone(), index, cx);
            }))
            .child(
                svg()
                    .path("chrome-icons/file-diff.svg")
                    .size(px(13.0))
                    .flex_none()
                    .text_color(colors().muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .font_family(MONO_FONT)
                    .text_size(px(11.5))
                    .text_color(colors().foreground)
                    .child(display_path(root, &file.path)),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_1()
                    .font_family(MONO_FONT)
                    .text_size(px(11.5))
                    .child(
                        div()
                            .text_color(colors().success)
                            .child(format!("+{added}")),
                    )
                    .child(
                        div()
                            .text_color(colors().danger)
                            .child(format!("\u{2212}{removed}")),
                    ),
            );
        div()
            .flex()
            .flex_col()
            .child(row)
            .when(expanded, |file_row| {
                file_row.child(file_diff(
                    SharedString::from(format!("changes-diff-{turn}-{}", file.path)),
                    file,
                ))
            })
            .into_any_element()
    }
}

/// The changed lines of every edit to one file, in order, in a scrollable
/// monospace frame. A gap separates one edit from the next.
fn file_diff(id: SharedString, file: &ChangedFile) -> AnyElement {
    let total: usize = file.edits.iter().map(Vec::len).sum();
    let mut rows: Vec<AnyElement> = Vec::new();
    let mut drawn = 0;
    'edits: for (number, edit) in file.edits.iter().enumerate() {
        if number > 0 {
            rows.push(div().h(px(8.0)).into_any_element());
        }
        for line in edit {
            if drawn == MAX_DIFF_LINES {
                break 'edits;
            }
            rows.push(diff_row(line).into_any_element());
            drawn += 1;
        }
    }
    let hidden = total - drawn;
    div()
        .px_3()
        .pb_2()
        .child(
            div()
                .id(id)
                .max_h(px(240.0))
                .overflow_y_scroll()
                .py_1()
                .rounded(px(8.0))
                .bg(surface_tint(colors().terminal, colors().elevated))
                .font_family(MONO_FONT)
                .text_size(px(11.5))
                .line_height(px(16.0))
                .text_color(colors().foreground)
                .children(rows)
                .when(hidden > 0, |frame| {
                    frame.child(
                        div()
                            .w_full()
                            .min_h(px(16.0))
                            .px_2()
                            .text_color(colors().subtle)
                            .child(format!("\u{2026} {hidden} more lines")),
                    )
                }),
        )
        .into_any_element()
}

/// One changed line, tinted like the tool cards' diffs.
fn diff_row(line: &DiffLine) -> Div {
    let (sign, background, color) = match line.kind {
        DiffKind::Added => ("+", colors().diff_added_bg, colors().diff_added),
        DiffKind::Removed => ("-", colors().diff_deleted_bg, colors().diff_deleted),
    };
    div()
        .w_full()
        .min_h(px(16.0))
        .px_2()
        .bg(background)
        .text_color(color)
        .child(format!("{sign} {}", line.text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::agent_chat::{ToolCallItem, ToolKind, ToolStatus};

    fn edit(path: &str, old: Option<&str>, new: &str) -> ChatItem {
        ChatItem::ToolCall(ToolCallItem {
            id: format!("call-{path}"),
            title: path.to_owned(),
            kind: ToolKind::Edit,
            status: ToolStatus::Completed,
            content: vec![ToolContent::Diff {
                path: path.to_owned(),
                old_text: old.map(str::to_owned),
                new_text: new.to_owned(),
            }],
            locations: Vec::new(),
        })
    }

    fn message() -> ChatItem {
        ChatItem::AssistantMessage {
            text: "done".to_owned(),
        }
    }

    #[test]
    fn files_merge_by_path_in_first_seen_order() {
        let items = vec![
            edit("src/a.rs", Some("x"), "y"),
            message(),
            edit("src/b.rs", None, "new\nfile"),
            edit("src/a.rs", Some("p"), "q\nr"),
        ];
        let files = changed_files(&items);
        let paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(paths, ["src/a.rs", "src/b.rs"]);
        assert_eq!(files[0].edits.len(), 2);
        // Each edit counts on its own: -x +y, then -p +q +r.
        assert_eq!(files[0].counts(), (3, 2));
        // A new file is all added lines.
        assert_eq!(files[1].counts(), (2, 0));
    }

    #[test]
    fn edits_that_change_no_lines_do_not_list_a_file() {
        assert!(changed_files(&[edit("a.rs", Some("same"), "same")]).is_empty());
    }

    #[test]
    fn tool_calls_without_diffs_contribute_nothing() {
        let text_only = ChatItem::ToolCall(ToolCallItem {
            id: "ls".to_owned(),
            title: "ls".to_owned(),
            kind: ToolKind::Execute,
            status: ToolStatus::Completed,
            content: vec![ToolContent::Text("listing".to_owned())],
            locations: Vec::new(),
        });
        assert!(changed_files(&[text_only, message()]).is_empty());
    }

    #[test]
    fn paths_inside_the_project_show_relative_to_its_root() {
        let root = Path::new("/work/vibra");
        assert_eq!(
            display_path(Some(root), "/work/vibra/src/main.rs"),
            "src/main.rs"
        );
        assert_eq!(display_path(Some(root), "src/main.rs"), "src/main.rs");
        assert_eq!(
            display_path(Some(root), "/elsewhere/main.rs"),
            "/elsewhere/main.rs"
        );
        // A sibling that only shares the name's prefix is outside the root.
        assert_eq!(
            display_path(Some(root), "/work/vibra-old/a.rs"),
            "/work/vibra-old/a.rs"
        );
        assert_eq!(display_path(Some(root), "/work/vibra"), "/work/vibra");
        assert_eq!(display_path(None, "/work/vibra/a.rs"), "/work/vibra/a.rs");
    }

    #[test]
    fn the_header_counts_files_in_words() {
        assert_eq!(files_label(1), "1 changed file");
        assert_eq!(files_label(3), "3 changed files");
    }
}
