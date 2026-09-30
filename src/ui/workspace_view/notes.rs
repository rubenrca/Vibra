//! Notes: plain text per project, pasted into a terminal when it becomes a
//! prompt. Pasting never submits, so the note can still be edited there.

use gpui::{Context, KeyDownEvent, Window};
use uuid::Uuid;

use crate::domain::library::MAX_NOTE_CHARS;
use crate::infrastructure::library::unix_now;
use crate::ui::terminal::TerminalInsertStatus;

use super::projects::next_library_project;
use super::{WorkspaceSection, WorkspaceView};
use crate::ui::text_edit::{TextKeyOutcome, apply_text_key};

mod rendering;

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
        let Some(note) = self.library.note_mut(id) else {
            return;
        };
        note.project_id = next_library_project(&self.snapshot.projects, note.project_id);
        self.persist_library(cx);
    }

    /// Use the selected project only for unassigned notes. An assigned note
    /// must never fall through to a terminal in another project.
    pub(super) fn project_active_session(&self, project_id: Option<Uuid>) -> Option<Uuid> {
        project_id
            .or(self.snapshot.selected_project_id)
            .and_then(|id| self.snapshot.project(id))
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
            self.library_error = Some("Open a terminal in the project to paste the note.".into());
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
            self.library_error =
                Some("Could not paste the note: the terminal is busy or rejected the text.".into());
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
                if body.chars().count() > MAX_NOTE_CHARS {
                    self.library_error = Some(
                        format!("Notes can contain up to {MAX_NOTE_CHARS} characters.").into(),
                    );
                    cx.notify();
                    return true;
                }
                let cleared_error = self.library_error.take().is_some();
                if self.library.set_note_body(id, body, unix_now()) {
                    self.persist_library(cx);
                } else if cleared_error {
                    cx.notify();
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
}
