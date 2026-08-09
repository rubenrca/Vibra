use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    Context, EventEmitter, FocusHandle, Focusable, IntoElement, KeyDownEvent, MouseButton, Render,
    SharedString, Window, div, prelude::*, px,
};

use crate::ports::files::{FileSystemPort, TextFileSnapshot};
use crate::ui::theme::DARK;

pub enum EditorViewEvent {
    Close,
    Saved,
}

pub struct EditorView {
    project_root: PathBuf,
    path: PathBuf,
    contents: String,
    saved_contents: String,
    fingerprint: String,
    cursor: usize,
    focus_handle: FocusHandle,
    search_active: bool,
    search_query: String,
    current_match: usize,
    undo_stack: Vec<String>,
    redo_stack: Vec<String>,
    error: Option<SharedString>,
    close_confirmation: bool,
    file_port: Arc<dyn FileSystemPort>,
}

impl EventEmitter<EditorViewEvent> for EditorView {}

impl EditorView {
    pub fn new(
        project_root: PathBuf,
        document: TextFileSnapshot,
        file_port: Arc<dyn FileSystemPort>,
        cx: &mut Context<Self>,
    ) -> Self {
        let cursor = document.contents.len();
        Self {
            project_root,
            path: document.path,
            saved_contents: document.contents.clone(),
            contents: document.contents,
            fingerprint: document.fingerprint,
            cursor,
            focus_handle: cx.focus_handle(),
            search_active: false,
            search_query: String::new(),
            current_match: 0,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            error: None,
            close_confirmation: false,
            file_port,
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.contents != self.saved_contents
    }

    fn checkpoint(&mut self) {
        if self.undo_stack.last() != Some(&self.contents) {
            self.undo_stack.push(self.contents.clone());
            if self.undo_stack.len() > 100 {
                self.undo_stack.remove(0);
            }
        }
        self.redo_stack.clear();
    }

    fn insert(&mut self, text: &str, cx: &mut Context<Self>) {
        self.checkpoint();
        self.contents.insert_str(self.cursor, text);
        self.cursor += text.len();
        self.error = None;
        cx.notify();
    }

    fn backspace(&mut self, cx: &mut Context<Self>) {
        let previous = previous_char_boundary(&self.contents, self.cursor);
        if previous == self.cursor {
            return;
        }
        self.checkpoint();
        self.contents.replace_range(previous..self.cursor, "");
        self.cursor = previous;
        self.error = None;
        cx.notify();
    }

    fn delete(&mut self, cx: &mut Context<Self>) {
        let next = next_char_boundary(&self.contents, self.cursor);
        if next == self.cursor {
            return;
        }
        self.checkpoint();
        self.contents.replace_range(self.cursor..next, "");
        self.error = None;
        cx.notify();
    }

    fn undo(&mut self, cx: &mut Context<Self>) {
        let Some(previous) = self.undo_stack.pop() else {
            return;
        };
        self.redo_stack
            .push(std::mem::replace(&mut self.contents, previous));
        self.cursor = self.cursor.min(self.contents.len());
        cx.notify();
    }

    fn redo(&mut self, cx: &mut Context<Self>) {
        let Some(next) = self.redo_stack.pop() else {
            return;
        };
        self.undo_stack
            .push(std::mem::replace(&mut self.contents, next));
        self.cursor = self.cursor.min(self.contents.len());
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if !self.is_dirty() {
            return;
        }
        match self.file_port.save_text_file(
            &self.project_root,
            &self.path,
            &self.contents,
            &self.fingerprint,
        ) {
            Ok(fingerprint) => {
                self.fingerprint = fingerprint;
                self.saved_contents.clone_from(&self.contents);
                self.error = None;
                cx.emit(EditorViewEvent::Saved);
            }
            Err(error) => self.error = Some(error.to_string().into()),
        }
        cx.notify();
    }

    pub fn request_close(&mut self, cx: &mut Context<Self>) {
        if self.is_dirty() {
            self.close_confirmation = true;
            cx.notify();
        } else {
            cx.emit(EditorViewEvent::Close);
        }
    }

    fn save_and_close(&mut self, cx: &mut Context<Self>) {
        self.save(cx);
        if !self.is_dirty() {
            cx.emit(EditorViewEvent::Close);
        }
    }

    fn search_next(&mut self) {
        if self.search_query.is_empty() {
            return;
        }
        let matches: Vec<_> = self
            .contents
            .match_indices(&self.search_query)
            .map(|(index, _)| index)
            .collect();
        if matches.is_empty() {
            self.current_match = 0;
            return;
        }
        self.current_match = (self.current_match + 1) % matches.len();
        self.cursor = matches[self.current_match];
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.to_ascii_lowercase();
        if self.close_confirmation {
            if matches!(key.as_str(), "escape" | "esc") {
                self.close_confirmation = false;
                cx.notify();
            }
            cx.stop_propagation();
            return;
        }
        if self.search_active {
            match key.as_str() {
                "escape" | "esc" => self.search_active = false,
                "enter" | "return" => self.search_next(),
                "backspace" => {
                    self.search_query.pop();
                    self.current_match = 0;
                }
                _ if !event.keystroke.modifiers.platform
                    && !event.keystroke.modifiers.control
                    && !event.keystroke.modifiers.alt =>
                {
                    if let Some(text) = event.keystroke.key_char.as_ref() {
                        self.search_query.push_str(text);
                        self.current_match = 0;
                    }
                }
                _ => {}
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }

        if event.keystroke.modifiers.platform {
            match key.as_str() {
                "s" => self.save(cx),
                "f" => self.search_active = true,
                "z" if event.keystroke.modifiers.shift => self.redo(cx),
                "z" => self.undo(cx),
                _ => return,
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }

        match key.as_str() {
            "left" => self.cursor = previous_char_boundary(&self.contents, self.cursor),
            "right" => self.cursor = next_char_boundary(&self.contents, self.cursor),
            "up" => self.cursor = vertical_cursor(&self.contents, self.cursor, -1),
            "down" => self.cursor = vertical_cursor(&self.contents, self.cursor, 1),
            "home" => self.cursor = line_start(&self.contents, self.cursor),
            "end" => self.cursor = line_end(&self.contents, self.cursor),
            "backspace" => self.backspace(cx),
            "delete" => self.delete(cx),
            "enter" | "return" => self.insert("\n", cx),
            "tab" => self.insert("    ", cx),
            _ if !event.keystroke.modifiers.control && !event.keystroke.modifiers.alt => {
                if let Some(text) = event.keystroke.key_char.as_ref() {
                    self.insert(text, cx);
                } else {
                    return;
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn cursor_for_line(&self, line_start: usize, line: &str) -> usize {
        (line_start + line.len()).min(self.contents.len())
    }
}

impl Render for EditorView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dirty = self.is_dirty();
        let title = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string());
        let path = self.path.display().to_string();
        let error = self.error.clone();
        let search_active = self.search_active;
        let close_confirmation = self.close_confirmation;
        let search_query = self.search_query.clone();
        let match_count = if search_query.is_empty() {
            0
        } else {
            self.contents.matches(&search_query).count()
        };
        let (cursor_line, cursor_column) = line_and_column(&self.contents, self.cursor);
        let mut offset = 0;
        let lines: Vec<_> = self
            .contents
            .split('\n')
            .enumerate()
            .map(|(index, line)| {
                let line_start = offset;
                offset += line.len() + 1;
                (index, line_start, line.to_owned())
            })
            .collect();

        div()
            .size_full()
            .relative()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .key_context("Editor")
            .on_key_down(cx.listener(Self::on_key_down))
            .bg(DARK.background)
            .child(
                div()
                    .h(px(38.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .border_b_1()
                    .border_color(DARK.border_subtle)
                    .bg(DARK.titlebar)
                    .child(
                        div()
                            .min_w(px(0.0))
                            .flex_1()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(11.0))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(DARK.foreground)
                                    .child(format!("{}{}", if dirty { "● " } else { "" }, title)),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .font_family("JetBrains Mono")
                                    .text_size(px(8.0))
                                    .text_color(DARK.subtle)
                                    .child(path),
                            ),
                    )
                    .child(
                        div()
                            .id("editor-find")
                            .px_2()
                            .py_1()
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .text_xs()
                            .text_color(DARK.muted)
                            .hover(|button| button.bg(DARK.hover))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.search_active = true;
                                this.focus_handle.focus(window);
                                cx.notify();
                            }))
                            .child("Find"),
                    )
                    .child(
                        div()
                            .id("editor-save")
                            .px_2()
                            .py_1()
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .text_xs()
                            .text_color(if dirty { DARK.success } else { DARK.subtle })
                            .hover(|button| button.bg(DARK.hover))
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx)))
                            .child("Save"),
                    )
                    .child(
                        div()
                            .id("editor-close")
                            .size(px(22.0))
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_sm()
                            .text_color(DARK.muted)
                            .hover(|button| button.bg(DARK.hover).text_color(DARK.foreground))
                            .on_click(cx.listener(|this, _, _, cx| this.request_close(cx)))
                            .child("×"),
                    ),
            )
            .when(search_active, |editor| {
                editor.child(
                    div()
                        .h(px(32.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .border_b_1()
                        .border_color(DARK.border_subtle)
                        .bg(DARK.elevated)
                        .child(
                            div()
                                .flex_1()
                                .font_family("JetBrains Mono")
                                .text_size(px(9.5))
                                .text_color(DARK.foreground)
                                .child(format!("Buscar: {search_query}")),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(DARK.subtle)
                                .child(format!("{match_count} matches · ↵ next · esc close")),
                        ),
                )
            })
            .when_some(error, |editor, error| {
                editor.child(
                    div()
                        .flex_none()
                        .px_3()
                        .py_2()
                        .bg(DARK.diff_deleted_bg)
                        .text_xs()
                        .text_color(DARK.danger)
                        .child(error),
                )
            })
            .child(
                div()
                    .id("editor-scroll")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_x_scroll()
                    .overflow_y_scroll()
                    .py_2()
                    .font_family("JetBrains Mono")
                    .text_size(px(11.0))
                    .line_height(px(18.0))
                    .children(lines.into_iter().map(|(index, line_start, line)| {
                        let active = index + 1 == cursor_line;
                        let matches = !search_query.is_empty() && line.contains(&search_query);
                        let display = if active {
                            let cursor_in_line =
                                self.cursor.saturating_sub(line_start).min(line.len());
                            let cursor_in_line =
                                previous_or_same_char_boundary(&line, cursor_in_line);
                            format!("{}▏{}", &line[..cursor_in_line], &line[cursor_in_line..])
                        } else {
                            line.clone()
                        };
                        div()
                            .id(SharedString::from(format!("editor-line-{}", index + 1)))
                            .min_w_full()
                            .h(px(18.0))
                            .flex()
                            .items_center()
                            .whitespace_nowrap()
                            .bg(if active {
                                DARK.selection
                            } else if matches {
                                DARK.diff_hunk_bg
                            } else {
                                DARK.background
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    this.cursor = this.cursor_for_line(line_start, &line);
                                    this.focus_handle.focus(window);
                                    cx.notify();
                                }),
                            )
                            .child(
                                div()
                                    .w(px(54.0))
                                    .h_full()
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_end()
                                    .pr_3()
                                    .border_r_1()
                                    .border_color(DARK.border_subtle)
                                    .text_color(DARK.subtle)
                                    .child((index + 1).to_string()),
                            )
                            .child(div().pl_3().pr_6().text_color(DARK.muted).child(display))
                    })),
            )
            .child(
                div()
                    .h(px(24.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_end()
                    .px_3()
                    .border_t_1()
                    .border_color(DARK.border_subtle)
                    .bg(DARK.titlebar)
                    .font_family("JetBrains Mono")
                    .text_size(px(8.5))
                    .text_color(DARK.subtle)
                    .child(format!("Ln {cursor_line}, Col {cursor_column} · UTF-8")),
            )
            .when(close_confirmation, |editor| {
                editor.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(gpui::rgba(0x08080add))
                        .child(
                            div()
                                .w(px(400.0))
                                .p_4()
                                .rounded_lg()
                                .border_1()
                                .border_color(DARK.border_subtle)
                                .bg(DARK.elevated)
                                .shadow_lg()
                                .flex()
                                .flex_col()
                                .gap_3()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .text_color(DARK.foreground)
                                        .child("¿Cerrar con cambios sin guardar?"),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(DARK.muted)
                                        .child("Puedes guardar, seguir editando o descartar."),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_end()
                                        .gap_2()
                                        .child(
                                            div()
                                                .id("editor-keep-editing")
                                                .px_3()
                                                .py_1()
                                                .rounded(px(5.0))
                                                .cursor_pointer()
                                                .text_xs()
                                                .text_color(DARK.muted)
                                                .hover(|button| button.bg(DARK.hover))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.close_confirmation = false;
                                                    cx.notify();
                                                }))
                                                .child("Seguir"),
                                        )
                                        .child(
                                            div()
                                                .id("editor-discard-close")
                                                .px_3()
                                                .py_1()
                                                .rounded(px(5.0))
                                                .cursor_pointer()
                                                .bg(DARK.diff_deleted_bg)
                                                .text_xs()
                                                .text_color(DARK.diff_deleted)
                                                .on_click(cx.listener(|_, _, _, cx| {
                                                    cx.emit(EditorViewEvent::Close);
                                                }))
                                                .child("Descartar"),
                                        )
                                        .child(
                                            div()
                                                .id("editor-save-close")
                                                .px_3()
                                                .py_1()
                                                .rounded(px(5.0))
                                                .cursor_pointer()
                                                .bg(DARK.success)
                                                .text_xs()
                                                .text_color(DARK.terminal)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.save_and_close(cx);
                                                }))
                                                .child("Guardar y cerrar"),
                                        ),
                                ),
                        ),
                )
            })
    }
}

impl Focusable for EditorView {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn previous_char_boundary(text: &str, cursor: usize) -> usize {
    if cursor == 0 {
        return 0;
    }
    text[..cursor]
        .char_indices()
        .next_back()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn next_char_boundary(text: &str, cursor: usize) -> usize {
    if cursor >= text.len() {
        return text.len();
    }
    cursor
        + text[cursor..]
            .chars()
            .next()
            .map(char::len_utf8)
            .unwrap_or_default()
}

fn previous_or_same_char_boundary(text: &str, cursor: usize) -> usize {
    let mut cursor = cursor.min(text.len());
    while !text.is_char_boundary(cursor) {
        cursor -= 1;
    }
    cursor
}

fn line_start(text: &str, cursor: usize) -> usize {
    text[..cursor].rfind('\n').map_or(0, |index| index + 1)
}

fn line_end(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .find('\n')
        .map_or(text.len(), |index| cursor + index)
}

fn vertical_cursor(text: &str, cursor: usize, delta: isize) -> usize {
    let start = line_start(text, cursor);
    let column = text[start..cursor].chars().count();
    if delta < 0 {
        if start == 0 {
            return cursor;
        }
        let previous_end = start - 1;
        let previous_start = line_start(text, previous_end);
        byte_at_char_column(text, previous_start, previous_end, column)
    } else {
        let end = line_end(text, cursor);
        if end == text.len() {
            return cursor;
        }
        let next_start = end + 1;
        let next_end = line_end(text, next_start);
        byte_at_char_column(text, next_start, next_end, column)
    }
}

fn byte_at_char_column(text: &str, start: usize, end: usize, column: usize) -> usize {
    text[start..end]
        .char_indices()
        .nth(column)
        .map_or(end, |(offset, _)| start + offset)
}

fn line_and_column(text: &str, cursor: usize) -> (usize, usize) {
    let line = text[..cursor].bytes().filter(|byte| *byte == b'\n').count() + 1;
    let start = line_start(text, cursor);
    let column = text[start..cursor].chars().count() + 1;
    (line, column)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_helpers_preserve_utf8_and_vertical_columns() {
        let text = "aéx\n12\nlong";
        let after_accent = "aé".len();
        assert_eq!(previous_char_boundary(text, after_accent), 1);
        assert_eq!(next_char_boundary(text, 1), after_accent);
        assert_eq!(vertical_cursor(text, after_accent, 1), "aéx\n12".len());
        assert_eq!(line_and_column(text, after_accent), (1, 3));
    }
}
