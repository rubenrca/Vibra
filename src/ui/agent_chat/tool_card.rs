//! Tool call cards: kind, title and status in the header, and an expandable
//! body with locations, text, diffs and client terminal output.

use gpui::{AnyElement, Context, Div, SharedString, Stateful, div, prelude::*, px, svg};

use super::AgentChatView;
use super::diff::{self, DiffKind, DiffLine};
use crate::domain::agent_chat::{ToolCallItem, ToolContent, ToolKind, ToolStatus};
use crate::ui::theme::{MONO_FONT, colors, surface_tint};

const MAX_DIFF_LINES: usize = 400;
const MAX_TERMINAL_LINES: usize = 60;
const MAX_LOCATIONS: usize = 5;

impl AgentChatView {
    pub(super) fn tool_card(
        &self,
        ix: usize,
        call: &ToolCallItem,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let expanded = self.expanded.contains(&call.id);
        let key = call.id.clone();
        let header = div()
            .id(SharedString::from(format!("chat-tool-{ix}")))
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .cursor_pointer()
            .hover(|row| row.bg(colors().hover))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_expanded(key.clone(), ix, cx);
            }))
            .child(icon(
                if expanded {
                    "chrome-icons/chevron-down.svg"
                } else {
                    "chrome-icons/chevron-right.svg"
                },
                colors().subtle,
            ))
            .child(icon(kind_icon(call.kind), colors().muted))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_size(px(12.5))
                    .when(call.kind == ToolKind::Execute, |title| {
                        title.font_family(MONO_FONT)
                    })
                    .child(call.title.clone()),
            )
            .child(status_label(call.status));
        div()
            .w_full()
            .rounded(px(8.0))
            .border_1()
            .border_color(colors().border_subtle)
            .bg(surface_tint(colors().elevated, colors().terminal))
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(header)
            .when(expanded, |card| card.child(self.tool_body(ix, call)))
            .into_any_element()
    }

    fn tool_body(&self, ix: usize, call: &ToolCallItem) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .px_3()
            .py_2()
            .border_t_1()
            .border_color(colors().border_subtle)
            .text_size(px(12.0))
            .text_color(colors().muted)
            .when(!call.locations.is_empty(), |body| {
                body.child(locations(&call.locations))
            })
            .children(
                call.content
                    .iter()
                    .enumerate()
                    .map(|(index, content)| self.tool_content(ix, index, content)),
            )
            .into_any_element()
    }

    fn tool_content(&self, ix: usize, index: usize, content: &ToolContent) -> AnyElement {
        let id = format!("chat-tool-{ix}-{index}");
        match content {
            ToolContent::Text(text) => {
                let lines = text.lines().map(strip_ansi).collect::<Vec<_>>();
                code_frame(id)
                    .children(lines.iter().map(|line| code_line(line)))
                    .into_any_element()
            }
            ToolContent::Diff {
                path,
                old_text,
                new_text,
            } => diff_block(
                id,
                path,
                &diff::changed_lines(old_text.as_deref(), new_text),
            ),
            ToolContent::Terminal { terminal_id } => self.terminal_block(id, terminal_id),
        }
    }

    fn terminal_block(&self, id: String, terminal_id: &str) -> AnyElement {
        let Some((output, exit_status)) = self.timeline.terminal(terminal_id) else {
            return div()
                .text_color(colors().subtle)
                .child("Waiting for output…")
                .into_any_element();
        };
        let lines: Vec<String> = output.lines().map(strip_ansi).collect();
        let (status, status_color) = match exit_status {
            None => ("Running…".to_owned(), colors().accent),
            Some(0) => ("Exited with code 0".to_owned(), colors().success),
            Some(code) => (format!("Exited with code {code}"), colors().danger),
        };
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                code_frame(id)
                    .when(lines.is_empty(), |frame| {
                        frame.child(code_line("No output yet.").text_color(colors().subtle))
                    })
                    .children(
                        tail_lines(&lines, MAX_TERMINAL_LINES)
                            .iter()
                            .map(|line| code_line(line)),
                    ),
            )
            .child(
                div()
                    .text_size(px(11.5))
                    .text_color(status_color)
                    .child(status),
            )
            .into_any_element()
    }
}

fn icon(path: &'static str, color: gpui::Rgba) -> AnyElement {
    svg()
        .path(path)
        .size(px(14.0))
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

fn status_label(status: ToolStatus) -> AnyElement {
    let (icon_path, label, color) = match status {
        ToolStatus::Pending => (None, "Pending", colors().subtle),
        ToolStatus::InProgress => (Some("chrome-icons/refresh.svg"), "Running", colors().accent),
        ToolStatus::Completed => (Some("chrome-icons/check.svg"), "Done", colors().success),
        ToolStatus::Failed => (Some("chrome-icons/close.svg"), "Failed", colors().danger),
    };
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap_1()
        .text_size(px(11.5))
        .text_color(color)
        .when_some(icon_path, |row, path| {
            row.child(
                svg()
                    .path(path)
                    .size(px(12.0))
                    .flex_none()
                    .text_color(color),
            )
        })
        .child(label)
        .into_any_element()
}

fn locations(paths: &[String]) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .font_family(MONO_FONT)
        .text_size(px(11.5))
        .text_color(colors().subtle)
        .children(
            paths
                .iter()
                .take(MAX_LOCATIONS)
                .map(|path| div().truncate().child(path.clone())),
        )
        .when(paths.len() > MAX_LOCATIONS, |list| {
            list.child(format!("+{} more", paths.len() - MAX_LOCATIONS))
        })
        .into_any_element()
}

/// Scrollable monospace frame shared by text, diffs and terminal output.
fn code_frame(id: String) -> Stateful<Div> {
    div()
        .id(SharedString::from(id))
        .max_h(px(240.0))
        .overflow_y_scroll()
        .py_1()
        .rounded(px(6.0))
        .bg(surface_tint(colors().terminal, colors().elevated))
        .font_family(MONO_FONT)
        .text_size(px(11.5))
        .line_height(px(16.0))
        .text_color(colors().foreground)
}

fn code_line(text: &str) -> Div {
    div()
        .w_full()
        .min_h(px(16.0))
        .px_2()
        .child(if text.is_empty() {
            " ".to_owned()
        } else {
            text.to_owned()
        })
}

fn diff_block(id: String, path: &str, lines: &[DiffLine]) -> AnyElement {
    let (added, removed) = diff::line_counts(lines);
    let hidden = lines.len().saturating_sub(MAX_DIFF_LINES);
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate()
                        .font_family(MONO_FONT)
                        .child(path.to_owned()),
                )
                .child(
                    div()
                        .text_color(colors().diff_added)
                        .child(format!("+{added}")),
                )
                .child(
                    div()
                        .text_color(colors().diff_deleted)
                        .child(format!("-{removed}")),
                ),
        )
        .child(
            code_frame(id)
                .children(lines.iter().take(MAX_DIFF_LINES).map(diff_row))
                .when(hidden > 0, |frame| {
                    frame.child(
                        code_line(&format!("… {hidden} more lines")).text_color(colors().subtle),
                    )
                }),
        )
        .into_any_element()
}

fn diff_row(line: &DiffLine) -> Div {
    let (sign, background, color) = match line.kind {
        DiffKind::Added => ("+", colors().diff_added_bg, colors().diff_added),
        DiffKind::Removed => ("-", colors().diff_deleted_bg, colors().diff_deleted),
    };
    code_line(&format!("{sign} {}", line.text))
        .bg(background)
        .text_color(color)
}

/// The last `count` lines, so long command output keeps its end visible.
fn tail_lines(lines: &[String], count: usize) -> &[String] {
    &lines[lines.len().saturating_sub(count)..]
}

/// Removes ANSI escape sequences (colors, cursor moves) from one line of output.
/// Only CSI sequences are handled; anything else is kept as text.
fn strip_ansi(line: &str) -> String {
    let mut output = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            // A CSI sequence ends at the first byte in 0x40..=0x7e.
            for next in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&next) {
                    break;
                }
            }
            continue;
        }
        output.push(character);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_removes_color_sequences_only() {
        assert_eq!(strip_ansi("\u{1b}[31merror\u{1b}[0m: x"), "error: x");
        assert_eq!(strip_ansi("plain [text]"), "plain [text]");
        assert_eq!(strip_ansi("\u{1b}[2K"), "");
    }

    #[test]
    fn tail_keeps_the_last_lines() {
        let lines: Vec<String> = ["a", "b", "c"].map(str::to_owned).to_vec();
        assert_eq!(tail_lines(&lines, 2), ["b".to_owned(), "c".to_owned()]);
        assert_eq!(tail_lines(&lines, 10).len(), 3);
        assert!(tail_lines(&[], 2).is_empty());
    }
}
