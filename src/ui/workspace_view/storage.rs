//! View-level save scheduling and errors. The persistence worker owns disk ordering.

use std::time::Duration;

use gpui::{Context, SharedString, Timer};

use super::WorkspaceView;
use crate::infrastructure::persistence::{
    DocumentKind, FinishError, SaveResult, save_final_blocking,
};

impl WorkspaceView {
    pub(super) fn persist(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        if self.workspace_load_error.is_some() {
            return;
        }
        self.persist_generation = self.persist_generation.wrapping_add(1);
        let generation = self.persist_generation;
        self._persist_task = Some(cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(400)).await;
            let _ = this.update(cx, |this, cx| {
                if this.persist_generation != generation {
                    return;
                }
                this.flush_persist(cx);
            });
        }));
    }

    pub(super) fn flush_persist(&mut self, cx: &mut Context<Self>) {
        if self.workspace_load_error.is_some() {
            return;
        }
        if self.persistence_queue.as_ref().is_some_and(|queue| {
            queue
                .save_workspace(self.persist_generation, self.snapshot.clone())
                .is_ok()
        }) {
            return;
        }
        self.workspace_save_error = self
            .repository
            .save(&self.snapshot)
            .err()
            .map(|error| SharedString::from(format!("Could not save: {error}")));
        cx.notify();
    }

    pub(super) fn persist_settings(&mut self, cx: &mut Context<Self>) {
        if self.settings_load_error.is_some() {
            cx.notify();
            return;
        }
        self.settings_generation = self.settings_generation.wrapping_add(1);
        if self.persistence_queue.as_ref().is_some_and(|queue| {
            queue
                .save_settings(self.settings_generation, self.settings.clone())
                .is_ok()
        }) {
            cx.notify();
            return;
        }
        self.settings_save_error = self
            .settings_repository
            .save(&self.settings)
            .err()
            .map(|error| format!("Could not save settings: {error}").into());
        cx.notify();
    }

    pub(super) fn apply_persistence_result(&mut self, result: SaveResult, cx: &mut Context<Self>) {
        let (generation, error) = match result.kind {
            DocumentKind::Workspace => (self.persist_generation, &mut self.workspace_save_error),
            DocumentKind::Settings => (self.settings_generation, &mut self.settings_save_error),
        };
        if result.error.is_some() || result.generation == generation {
            *error = result.error.map(Into::into);
            cx.notify();
        }
    }

    pub(super) fn save_final_direct(&self) {
        let workspace = (self.persist_generation > 0 && self.workspace_load_error.is_none())
            .then(|| self.snapshot.clone());
        let settings = ((self.settings_generation > 0 || self.window_size_persist_generation > 0)
            && self.settings_load_error.is_none())
        .then(|| self.settings.clone());
        if workspace.is_none() && settings.is_none() {
            return;
        }
        match save_final_blocking(
            self.repository.clone(),
            self.settings_repository.clone(),
            workspace,
            settings,
        ) {
            Ok(()) => {}
            Err(FinishError::Save(error)) => eprintln!("{error}"),
            Err(FinishError::Unavailable) => {
                eprintln!("Final save unavailable")
            }
        }
    }
}
