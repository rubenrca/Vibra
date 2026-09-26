//! Notes: plain text per project, pasted into a terminal when it becomes a
//! prompt. Pasting never submits, so the note can still be edited there.

use gpui::{AnyElement, Context, KeyDownEvent, SharedString, Window, div, prelude::*, px};
use uuid::Uuid;

use crate::domain::inbox::relative_time;
use crate::infrastructure::library::unix_now;
use crate::ui::terminal::TerminalInsertStatus;
use crate::ui::theme::{MONO_FONT, colors, surface_tint};

use super::navigation::{project_color, section_button, section_empty_state, section_frame};
use super::{WorkspaceSection, WorkspaceView};
use crate::ui::text_edit::{TextKeyOutcome, apply_text_key};

impl WorkspaceView {
    pub(super) fn create_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(previous) = self.selected_note_id {
            self.library.discard_blank_note(previous);
        }
        let id = self
            .library
            .create_note(self.snapshot.selected_project_id, unix_now());
        self.selected_note_id = Some(id);
        self.note_editing = true;
        if self.workspace_section != WorkspaceSection::Notes {
            self.select_section(WorkspaceSection::Notes, window, cx);
        }
        cx.notify();
    }

    fn select_note(&mut self, id: Uuid, cx: &mut Context<Self>) {
        if let Some(previous) = self.selected_note_id.filter(|previous| *previous != id) {
            self.library.discard_blank_note(previous);
        }
        self.selected_note_id = Some(id);
        self.note_editing = true;
        cx.notify();
    }

    fn delete_selected_note(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected_note_id.take() else {
            return;
        };
        self.note_editing = false;
        if self.library.delete_note(id) {
            self.persist_library(cx);
        }
        cx.notify();
    }

    fn cycle_note_project(&mut self, id: Uuid, cx: &mut Context<Self>) {
        let projects: Vec<Option<Uuid>> = std::iter::once(None)
            .chain(
                self.snapshot
                    .projects
                    .iter()
                    .map(|project| Some(project.id)),
            )
            .collect();
        let Some(note) = self.library.note_mut(id) else {
            return;
        };
        let index = projects
            .iter()
            .position(|project| *project == note.project_id)
            .unwrap_or(0);
        note.project_id = projects[(index + 1) % projects.len()];
        self.persist_library(cx);
    }

    /// Use the selected project only for unassigned notes. An assigned note
    /// must never fall through to a terminal in another project.
    pub(super) fn project_active_session(&self, project_id: Option<Uuid>) -> Option<Uuid> {
        project_id
            .or(self.snapshot.selected_project_id)
            .and_then(|id| {
                self.snapshot
                    .projects
                    .iter()
                    .find(|project| project.id == id)
            })
            .and_then(|project| {
                let workspaces = project.workspaces.as_deref().unwrap_or_default();
                let workspace = project
                    .selected_workspace_id
                    .and_then(|id| workspaces.iter().find(|workspace| workspace.id == id))
                    .or_else(|| workspaces.first())?;
                workspace.primary_session().map(|session| session.id)
            })
    }

    pub(super) fn paste_note_into_terminal(
        &mut self,
        id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(note) = self.library.note(id).cloned() else {
            return;
        };
        let text = note.body.trim().to_owned();
        if text.is_empty() {
            return;
        }
        let Some(target) = self.project_active_session(note.project_id) else {
            self.library_error =
                Some("Abre una terminal en el proyecto para pegar la nota.".into());
            cx.notify();
            return;
        };
        let Some(terminal) = self.terminals.get(&target).cloned() else {
            return;
        };
        let status = terminal.update(cx, |terminal, cx| {
            terminal.insert_external_text(&text, Uuid::new_v4(), cx)
        });
        if status == TerminalInsertStatus::Rejected {
            self.library_error = Some(
                "No se pudo pegar la nota: la terminal está ocupada o rechazó el texto.".into(),
            );
            cx.notify();
            return;
        }
        self.library_error = None;
        self.open_pane(target, window, cx);
    }

    /// Routes typing into the selected note while Notes is on screen.
    pub(super) fn handle_note_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        if self.workspace_section != WorkspaceSection::Notes || !self.note_editing {
            return false;
        }
        let Some(id) = self.selected_note_id else {
            return false;
        };
        let Some(mut body) = self.library.note(id).map(|note| note.body.clone()) else {
            return false;
        };
        let outcome = apply_text_key(
            &mut body,
            event.keystroke.key.as_str(),
            event.keystroke.key_char.as_deref(),
            &event.keystroke.modifiers,
            true,
            || cx.read_from_clipboard().and_then(|item| item.text()),
        );
        match outcome {
            TextKeyOutcome::Edited => {
                if self.library.set_note_body(id, body, unix_now()) {
                    self.persist_library(cx);
                }
                true
            }
            TextKeyOutcome::Submit | TextKeyOutcome::Cancel => {
                self.note_editing = false;
                if self.library.discard_blank_note(id) {
                    self.selected_note_id = None;
                }
                cx.notify();
                true
            }
            TextKeyOutcome::NextField | TextKeyOutcome::PreviousField => true,
            TextKeyOutcome::Unhandled => false,
        }
    }

    fn project_name(&self, project_id: Option<Uuid>) -> Option<String> {
        project_id.and_then(|id| {
            self.snapshot
                .projects
                .iter()
                .find(|project| project.id == id)
                .map(|project| project.name.clone())
        })
    }

    pub(super) fn notes_content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let now = unix_now();
        if self
            .selected_note_id
            .is_some_and(|id| self.library.note(id).is_none())
        {
            self.selected_note_id = None;
        }
        let selected = self.selected_note_id;
        let actions = vec![
            section_button("notes-new", "Nueva nota", true)
                .on_click(cx.listener(|this, _, window, cx| this.create_note(window, cx)))
                .into_any_element(),
        ];
        let notes = self.library.notes_by_recency();
        if notes.is_empty() {
            return section_frame(
                "Notes",
                actions,
                section_empty_state(
                    "chrome-icons/notes.svg",
                    "Un espacio para tus ideas",
                    "Guarda contexto, pendientes y prompts por proyecto. Cuando una nota esté lista, pégala en la terminal del agente para seguir editándola ahí.",
                ),
            );
        }

        let list = div()
            .id("notes-list")
            .w(px(260.0))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .children(notes.into_iter().map(|note| {
                let id = note.id;
                let is_selected = selected == Some(id);
                let project = self.project_name(note.project_id);
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
                    .when(is_selected, |row| {
                        row.bg(surface_tint(colors().selection, colors().background))
                    })
                    .when(!is_selected, |row| {
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
                            .child(match (project, note.preview()) {
                                (Some(project), preview) if preview.is_empty() => {
                                    format!("{project} · {}", relative_time(now, note.updated_at))
                                }
                                (Some(project), preview) => format!("{project} · {preview}"),
                                (None, preview) if preview.is_empty() => {
                                    relative_time(now, note.updated_at)
                                }
                                (None, preview) => preview,
                            }),
                    )
            }));

        let editor = match selected.and_then(|id| self.library.note(id)).cloned() {
            None => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(12.5))
                .text_color(colors().subtle)
                .child("Elige una nota o crea una nueva.")
                .into_any_element(),
            Some(note) => {
                let id = note.id;
                let editing = self.note_editing;
                let project = self.project_name(note.project_id);
                let mut lines: Vec<String> = note.body.split('\n').map(str::to_owned).collect();
                let blank = note.is_blank();
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .h_full()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
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
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.cycle_note_project(id, cx)
                                    }))
                                    .when_some(note.project_id, |chip, project_id| {
                                        chip.child(
                                            div()
                                                .size(px(7.0))
                                                .rounded_full()
                                                .bg(project_color(project_id)),
                                        )
                                    })
                                    .child(project.unwrap_or_else(|| "Sin proyecto".to_owned())),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(11.5))
                                    .text_color(colors().subtle)
                                    .child(format!(
                                        "Editada {}",
                                        relative_time(now, note.updated_at)
                                    )),
                            )
                            .child(section_button("note-delete", "Eliminar", false).on_click(
                                cx.listener(|this, _, _, cx| this.delete_selected_note(cx)),
                            ))
                            .child(
                                section_button("note-paste", "Pegar en la terminal", true)
                                    .when(blank, |button| button.opacity(0.5))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.paste_note_into_terminal(id, window, cx)
                                    })),
                            ),
                    )
                    .child(
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
                            .when(blank && !editing, |body| {
                                body.child(
                                    div()
                                        .text_color(colors().subtle)
                                        .child("Haz clic para escribir."),
                                )
                            })
                            .children({
                                let last = lines.len().saturating_sub(1);
                                if editing {
                                    lines[last].push('▏');
                                }
                                lines.into_iter().map(|line| {
                                    div().min_h(px(20.0)).whitespace_normal().child(line)
                                })
                            }),
                    )
                    .child(div().text_size(px(11.5)).text_color(colors().subtle).child(
                        if editing {
                            "↩ nueva línea · ⌘↩ o Esc terminan · ⌥⌫ borra una palabra · ⌘V pega"
                        } else {
                            "Haz clic en la nota para seguir escribiendo."
                        },
                    ))
                    .into_any_element()
            }
        };

        section_frame(
            "Notes",
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
}
