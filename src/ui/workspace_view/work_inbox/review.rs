use super::*;
use crate::domain::work_items::FileViewed;
use crate::ports::git::{GitFileChange, GitFileStatus, GitRepositorySnapshot};
use crate::ui::diff_document::DiffDocument;
use crate::ui::diff_rows::{CommentAnchor, CommentSide, RemoteThread};
use crate::ui::diff_view::{DiffView, DiffViewEvent, ExternalDiffSource};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// The three reviews a pull request can show in Review.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReviewKind {
    /// Every change of the pull request (Code tab).
    Pull,
    /// Changes since the head the user last reviewed (Code tab).
    Since,
    /// One commit (Commits tab).
    Commit,
}

impl WorkspaceView {
    fn new_inbox_review(
        &mut self,
        url: &str,
        kind: ReviewKind,
        cx: &mut Context<Self>,
    ) -> (gpui::Entity<DiffView>, gpui::Subscription) {
        let view = cx.new(|cx| DiffView::new(std::path::PathBuf::new(), self.git_port.clone(), cx));
        let key = url.to_owned();
        let subscription =
            cx.subscribe(
                &view,
                move |this, view, event: &DiffViewEvent, cx| match event {
                    DiffViewEvent::PreferencesChanged { split, wrap } => {
                        this.settings.diff_split = *split;
                        this.settings.diff_wrap = *wrap;
                        this.diff_view.update(cx, |diff, cx| {
                            diff.set_preferences(*split, *wrap, this.settings.diff_font_size, cx)
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
                    DiffViewEvent::ToggleViewed { path, viewed } if kind != ReviewKind::Commit => {
                        this.set_inbox_file_viewed(&key, path, *viewed, cx);
                    }
                    _ => cx.notify(),
                },
            );
        (view, subscription)
    }

    pub(super) fn sync_inbox_review(&mut self, url: &str, cx: &mut Context<Self>) {
        self.sync_pull_review(url, ReviewKind::Pull, cx);
    }

    /// Create or update the review of one kind from its loaded diff.
    pub(super) fn sync_pull_review(&mut self, url: &str, kind: ReviewKind, cx: &mut Context<Self>) {
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
        let Some(state) = self.work_inbox.details.get(url) else {
            return;
        };
        let (diff, identity) = match kind {
            ReviewKind::Pull => (state.diff.data.clone(), String::new()),
            ReviewKind::Since => (
                state.since.data.clone(),
                format!("since:{}", state.since_base),
            ),
            ReviewKind::Commit => {
                let Some(oid) = state.selected_commit.clone() else {
                    return;
                };
                (
                    state
                        .commit_diffs
                        .get(&oid)
                        .and_then(|diff| diff.data.clone()),
                    format!("commit:{oid}"),
                )
            }
        };
        let Some(diff) = diff else {
            return;
        };
        let existing = match kind {
            ReviewKind::Pull => state.review.clone(),
            ReviewKind::Since => state.since_review.clone(),
            ReviewKind::Commit => state.commit_review.clone(),
        };
        let view = match existing {
            Some(view) => view,
            None => {
                let (view, subscription) = self.new_inbox_review(url, kind, cx);
                let state = self.work_inbox.details.get_mut(url).expect("detail exists");
                match kind {
                    ReviewKind::Pull => {
                        state.review = Some(view.clone());
                        state._review_subscription = Some(subscription);
                    }
                    ReviewKind::Since => {
                        state.since_review = Some(view.clone());
                        state._since_review_subscription = Some(subscription);
                    }
                    ReviewKind::Commit => {
                        state.commit_review = Some(view.clone());
                        state._commit_review_subscription = Some(subscription);
                    }
                }
                view
            }
        };
        let source = review_source(item, diff, &identity);
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
        self.sync_inbox_annotations(url, cx);
    }

    /// Push GitHub's viewed marks and published threads into the reviews.
    pub(super) fn sync_inbox_annotations(&mut self, url: &str, cx: &mut Context<Self>) {
        let Some(state) = self.work_inbox.details.get(url) else {
            return;
        };
        let detail = state.summary.data.as_ref();
        let mut viewed: HashMap<String, FileViewed> = detail
            .map(|detail| detail.viewed.clone().into_iter().collect())
            .unwrap_or_default();
        for (path, pending) in &state.viewed_pending {
            viewed.insert(
                path.clone(),
                if *pending {
                    FileViewed::Viewed
                } else {
                    FileViewed::Unviewed
                },
            );
        }
        let threads: Vec<RemoteThread> = detail
            .map(|detail| {
                detail
                    .threads
                    .iter()
                    .filter(|thread| !thread.outdated)
                    .filter_map(|thread| {
                        Some(RemoteThread {
                            id: thread.id.clone(),
                            anchor: CommentAnchor {
                                path: thread.path.clone(),
                                side: if thread.left {
                                    CommentSide::Old
                                } else {
                                    CommentSide::New
                                },
                                line: thread.line? as usize,
                            },
                            resolved: thread.resolved,
                            comments: thread
                                .comments
                                .iter()
                                .map(|comment| (comment.author.clone(), comment.body.clone()))
                                .collect(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let has_detail = detail.is_some();
        // The "since" review compares two heads: its new side is the current
        // head, like the pull request's, but its old side is not the base.
        for (view, kind) in [
            (state.review.clone(), ReviewKind::Pull),
            (state.since_review.clone(), ReviewKind::Since),
            (state.commit_review.clone(), ReviewKind::Commit),
        ] {
            let Some(view) = view else {
                continue;
            };
            let annotated = kind != ReviewKind::Commit;
            let viewed = (annotated && has_detail).then(|| viewed.clone());
            let threads = threads
                .iter()
                .filter(|thread| match kind {
                    ReviewKind::Pull => true,
                    ReviewKind::Since => thread.anchor.side == CommentSide::New,
                    ReviewKind::Commit => false,
                })
                .cloned()
                .collect();
            view.update(cx, |view, cx| {
                view.set_viewed(viewed, cx);
                view.set_remote_threads(threads, cx);
            });
        }
    }

    pub(in crate::ui::workspace_view) fn sync_inbox_review_preferences(
        &self,
        cx: &mut Context<Self>,
    ) {
        let views = self
            .work_inbox
            .details
            .values()
            .flat_map(|state| [&state.review, &state.since_review, &state.commit_review])
            .chain([&self.work_inbox.history.review])
            .flatten();
        for view in views {
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

fn review_source(item: WorkItem, diff: WorkDiff, identity: &str) -> ExternalDiffSource {
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
        identity: format!("{}:{identity}:{:x}", item.url, hash.finish()),
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
