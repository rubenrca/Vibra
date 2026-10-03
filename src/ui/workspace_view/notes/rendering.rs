//! Notes list, editor controls, and source text rendering.

use gpui::{AnyElement, Context, Div, SharedString, Stateful, div, prelude::*, px};
use uuid::Uuid;

use crate::domain::inbox::relative_time;
use crate::domain::library::Note;
use crate::infrastructure::library::unix_now;
use crate::ui::theme::{MONO_FONT, colors, surface_tint};

use super::super::navigation::{project_color, section_button, section_empty_state, section_frame};
use super::WorkspaceView;

impl WorkspaceView {
    pub(in crate::ui::workspace_view) fn notes_content(
        &mut self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self
            .selected_note_id
            .is_some_and(|id| self.library.note(id).is_none())
        {
            self.selected_note_id = None;
        }
        let selected = self.selected_note_id;
        let actions = vec![
            section_button("notes-new", "New note", true)
                .on_click(cx.listener(|this, _, window, cx| this.create_note(window, cx)))
                .into_any_element(),
        ];
        let notes = self.library.notes_by_recency();
        if notes.is_empty() {
            return section_frame(
                "Notes · In development",
                actions,
                section_empty_state(
                    "chrome-icons/notes.svg",
                    "A space for your ideas",
                    concat!(
                        "Save context, to-dos, and prompts by project. When a note is ready, ",
                        "paste it into the agent's terminal to keep editing it there.",
                    ),
                ),
            );
        }
        let now = unix_now();
        let list = div()
            .id("notes-list")
            .w(px(260.0))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .children(
                notes
                    .into_iter()
                    .map(|note| self.note_list_row(note, selected, now, cx)),
            );
        let editor = self.note_editor(selected.and_then(|id| self.library.note(id)), now, cx);
        section_frame(
            "Notes · In development",
            actions,
            div()
                .w_full()
                .max_w(px(1100.0))
                .h_full()
                .min_h(px(360.0))
                .flex()
                .gap_4()
                .child(list)
                .child(editor)
                .into_any_element(),
        )
    }

    fn note_project_name(&self, project_id: Option<Uuid>) -> Option<String> {
        project_id
            .and_then(|id| self.snapshot.project(id))
            .map(|project| project.name.clone())
    }

    fn note_list_row(
        &self,
        note: &Note,
        selected: Option<Uuid>,
        now: u64,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let id = note.id;
        let selected = selected == Some(id);
        let subtitle = match (self.note_project_name(note.project_id), note.preview()) {
            (Some(project), preview) if preview.is_empty() => {
                format!("{project} · {}", relative_time(now, note.updated_at))
            }
            (Some(project), preview) => format!("{project} · {preview}"),
            (None, preview) if preview.is_empty() => relative_time(now, note.updated_at),
            (None, preview) => preview,
        };
        div()
            .id(SharedString::from(format!("note-{id}")))
            .w_full()
            .px_3()
            .py(px(8.0))
            .rounded(px(8.0))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .cursor_pointer()
            .when(selected, |row| {
                row.bg(surface_tint(colors().selection, colors().background))
            })
            .when(!selected, |row| {
                row.hover(|row| row.bg(surface_tint(colors().hover, colors().background)))
            })
            .on_click(cx.listener(move |this, _, _, cx| this.select_note(id, cx)))
            .child(
                div()
                    .text_size(px(13.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .truncate()
                    .child(note.title()),
            )
            .child(
                div()
                    .text_size(px(11.5))
                    .text_color(colors().subtle)
                    .truncate()
                    .child(subtitle),
            )
    }

    fn note_editor(&self, note: Option<&Note>, now: u64, cx: &mut Context<Self>) -> AnyElement {
        let Some(note) = note else {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(12.5))
                .text_color(colors().subtle)
                .child("Choose a note or create a new one.")
                .into_any_element();
        };
        div()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.note_editor_controls(note, now, cx))
            .child(self.note_editor_body(note, cx))
            .child(div().text_size(px(11.5)).text_color(colors().subtle).child(
                if self.note_editing {
                    "↩ new line · ⌘↩ or Esc to finish · ⌥⌫ delete a word · ⌘V paste"
                } else {
                    "Click the note to keep writing."
                },
            ))
            .into_any_element()
    }

    fn note_editor_controls(&self, note: &Note, now: u64, cx: &mut Context<Self>) -> Div {
        let id = note.id;
        let project = self.note_project_name(note.project_id);
        let project = div()
            .id("note-project")
            .h(px(24.0))
            .px_2()
            .rounded(px(5.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .cursor_pointer()
            .text_size(px(11.5))
            .text_color(colors().muted)
            .bg(surface_tint(colors().elevated, colors().background))
            .hover(|chip| chip.text_color(colors().foreground))
            .on_click(cx.listener(move |this, _, _, cx| this.cycle_note_project(id, cx)))
            .when_some(note.project_id, |chip, project_id| {
                chip.child(
                    div()
                        .size(px(7.0))
                        .rounded_full()
                        .bg(project_color(project_id)),
                )
            })
            .child(project.unwrap_or_else(|| "No project".to_owned()));
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(project)
            .child(
                div()
                    .flex_1()
                    .text_size(px(11.5))
                    .text_color(colors().subtle)
                    .child(format!("Edited {}", relative_time(now, note.updated_at))),
            )
            .child(
                section_button("note-delete", "Delete", false)
                    .on_click(cx.listener(|this, _, _, cx| this.delete_selected_note(cx))),
            )
            .child(
                section_button("note-paste", "Paste into terminal", true)
                    .when(note.is_blank(), |button| button.opacity(0.5))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.paste_note_into_terminal(id, window, cx)
                    })),
            )
    }

    fn note_editor_body(&self, note: &Note, cx: &mut Context<Self>) -> Stateful<Div> {
        let editing = self.note_editing;
        let mut lines: Vec<String> = note.body.split('\n').map(str::to_owned).collect();
        if editing && let Some(last) = lines.last_mut() {
            last.push('▏');
        }
        div()
            .id("note-body")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .p_4()
            .rounded(px(8.0))
            .border_1()
            .border_color(if editing {
                colors().accent
            } else {
                colors().border_subtle
            })
            .bg(surface_tint(colors().elevated, colors().background))
            .cursor_text()
            .font_family(MONO_FONT)
            .text_size(px(13.0))
            .line_height(px(20.0))
            .on_click(cx.listener(|this, _, _, cx| {
                this.note_editing = true;
                cx.notify();
            }))
            .when(note.is_blank() && !editing, |body| {
                body.child(div().text_color(colors().subtle).child("Click to write."))
            })
            .children(
                lines
                    .into_iter()
                    .map(|line| div().min_h(px(20.0)).whitespace_normal().child(line)),
            )
    }
}
