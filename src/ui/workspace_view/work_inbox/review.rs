use super::*;
use crate::ports::git::{GitFileChange, GitFileStatus, GitRepositorySnapshot};
use crate::ui::diff_document::DiffDocument;
use crate::ui::diff_view::{DiffView, DiffViewEvent, ExternalDiffSource};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

impl WorkspaceView {
    pub(super) fn sync_inbox_review(&mut self, url: &str, cx: &mut Context<Self>) {
        let Some(item) = self
            .work_inbox
            .github
            .items
            .iter()
            .find(|item| item.url == url)
            .cloned()
        else {
            return;
        };
        let Some(diff) = self
            .work_inbox
            .details
            .get(url)
            .and_then(|state| state.diff.data.as_ref())
        else {
            return;
        };
        let source = review_source(item, diff.clone());
        let view = if let Some(view) = self
            .work_inbox
            .details
            .get(url)
            .and_then(|state| state.review.clone())
        {
            view
        } else {
            let view =
                cx.new(|cx| DiffView::new(std::path::PathBuf::new(), self.git_port.clone(), cx));
            let key = url.to_owned();
            let subscription =
                cx.subscribe(
                    &view,
                    move |this, view, event: &DiffViewEvent, cx| match event {
                        DiffViewEvent::PreferencesChanged { split, wrap } => {
                            this.settings.diff_split = *split;
                            this.settings.diff_wrap = *wrap;
                            this.diff_view.update(cx, |diff, cx| {
                                diff.set_preferences(
                                    *split,
                                    *wrap,
                                    this.settings.diff_font_size,
                                    cx,
                                )
                            });
                            this.sync_inbox_review_preferences(cx);
                            this.persist_settings(cx);
                        }
                        DiffViewEvent::SendReview {
                            prompt,
                            delivery_id,
                        } => {
                            let accepted = this.work_inbox.selected.as_deref() == Some(&key);
                            if accepted {
                                if !this.work_inbox.composer_note.is_empty() {
                                    this.work_inbox.composer_note.push_str("\n\n");
                                }
                                this.work_inbox
                                    .composer_note
                                    .push_str(&format!("Review comments for {key}:\n{prompt}"));
                                this.open_inbox_composer(cx);
                            }
                            view.update(cx, |view, cx| {
                                view.resolve_review_delivery(*delivery_id, accepted, cx)
                            });
                        }
                        _ => cx.notify(),
                    },
                );
            let state = self.work_inbox.details.get_mut(url).expect("detail exists");
            state.review = Some(view.clone());
            state._review_subscription = Some(subscription);
            view
        };
        let full = self.work_inbox.details[url].code_mode == CodeMode::FullFile;
        view.update(cx, |view, cx| {
            view.set_preferences(
                self.settings.diff_split,
                self.settings.diff_wrap,
                self.settings.diff_font_size,
                cx,
            );
            view.set_external_source(source, cx);
            view.set_full_file(full, cx);
        });
    }

    pub(in crate::ui::workspace_view) fn sync_inbox_review_preferences(
        &self,
        cx: &mut Context<Self>,
    ) {
        for state in self.work_inbox.details.values() {
            if let Some(view) = &state.review {
                view.update(cx, |view, cx| {
                    view.set_preferences(
                        self.settings.diff_split,
                        self.settings.diff_wrap,
                        self.settings.diff_font_size,
                        cx,
                    )
                });
            }
        }
    }
}

fn review_source(item: WorkItem, diff: WorkDiff) -> ExternalDiffSource {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    diff.hash(&mut hash);
    let snapshot = GitRepositorySnapshot {
        root: std::path::PathBuf::from(&item.url),
        branch: item.reference.clone(),
        changes: diff
            .files
            .iter()
            .map(|file| GitFileChange {
                path: file.path.clone(),
                old_path: file.previous_path.clone(),
                status: if file.removed {
                    GitFileStatus::Deleted
                } else if file.previous_path.is_some() {
                    GitFileStatus::Renamed
                } else if file.patch.starts_with("@@ -0,0 ") {
                    GitFileStatus::Added
                } else {
                    GitFileStatus::Modified
                },
                staged: false,
                unstaged: false,
                untracked: false,
                additions: Some(file.additions as usize),
                deletions: Some(file.deletions as usize),
            })
            .collect(),
        additions: diff.files.iter().map(|file| file.additions as usize).sum(),
        deletions: diff.files.iter().map(|file| file.deletions as usize).sum(),
    };
    ExternalDiffSource {
        identity: format!("{}:{:x}", item.url, hash.finish()),
        snapshot,
        load: Arc::new(move |path| {
            let file = diff
                .files
                .iter()
                .find(|file| file.path == path)
                .ok_or_else(|| {
                    anyhow::anyhow!("This file is no longer part of the pull request.")
                })?;
            let (diff, sources) = work_items::load_review_file(&item, file);
            Ok(DiffDocument::prepare_with_sources(diff, Some(&sources)))
        }),
    }
}
