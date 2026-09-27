//! Remote reviews share the Workspace renderer, without inspecting a checkout.

use super::*;

type DocumentLoader = dyn Fn(&str) -> anyhow::Result<DiffDocument> + Send + Sync;

pub(crate) struct ExternalDiffSource {
    pub identity: String,
    pub snapshot: GitRepositorySnapshot,
    pub load: Arc<DocumentLoader>,
}

impl DiffView {
    pub(crate) fn set_external_source(
        &mut self,
        source: ExternalDiffSource,
        cx: &mut Context<Self>,
    ) {
        if self
            .external
            .as_ref()
            .is_some_and(|current| current.identity == source.identity)
        {
            return;
        }
        self.mode = GitPanelMode::Worktree;
        self._poll_task = Task::ready(());
        self.panel_visible = false;
        self.review_expanded = true;
        self.snapshot_settled = true;
        self.error = None;
        let snapshot = source.snapshot.clone();
        self.external = Some(source);
        self.apply_snapshot(snapshot, cx);
        cx.notify();
    }

    pub(crate) fn set_full_file(&mut self, full: bool, cx: &mut Context<Self>) {
        if self.full_file == full {
            return;
        }
        self.full_file = full;
        self.fold_reveals.clear();
        if full {
            for (path, cached) in &self.documents {
                self.fold_reveals
                    .insert(path.clone(), all_folds(&cached.document));
            }
        }
        cx.notify();
    }

    pub(super) fn source_for_change(
        &self,
        snapshot: &GitRepositorySnapshot,
        change: &GitFileChange,
        against: Option<&str>,
        head: Option<&str>,
    ) -> DiffSource {
        if let Some(external) = &self.external {
            DiffSource {
                repository: snapshot.root.clone(),
                change: change.clone(),
                against: None,
                head: Some(external.identity.clone()),
                worktree_version: None,
            }
        } else {
            DiffSource::new(snapshot, change, against, head)
        }
    }
}

pub(super) fn all_folds(document: &DiffDocument) -> HashMap<usize, FoldReveal> {
    document
        .folds
        .iter()
        .enumerate()
        .map(|(index, fold)| {
            (
                index,
                FoldReveal {
                    head: fold.len,
                    tail: 0,
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::git::GitCliPort;
    use crate::ports::git::GitDiffSources;

    fn source(identity: &str, files: usize) -> ExternalDiffSource {
        let name = identity.to_owned();
        ExternalDiffSource {
            identity: identity.into(),
            snapshot: GitRepositorySnapshot {
                root: PathBuf::from("https://github.com/example/repo/pull/1"),
                branch: "#1".into(),
                changes: (0..files)
                    .map(|index| GitFileChange {
                        path: format!("{index}.rs"),
                        old_path: None,
                        status: GitFileStatus::Modified,
                        staged: false,
                        unstaged: false,
                        untracked: false,
                        additions: Some(1),
                        deletions: Some(1),
                    })
                    .collect(),
                additions: files,
                deletions: files,
            },
            load: Arc::new(move |path| {
                let patch = format!("@@ -2 +2 @@\n-fn old() {{}}\n+fn {name}() {{}}\n");
                let diff = crate::infrastructure::git::parse_diff_patch(path, patch.as_bytes());
                Ok(DiffDocument::prepare_with_sources(
                    diff,
                    Some(&GitDiffSources {
                        old: Some("// before\nfn old() {}\n// after\n".into()),
                        new: Some(format!("// before\nfn {name}() {{}}\n// after\n")),
                    }),
                ))
            }),
        }
    }

    #[gpui::test]
    fn remote_review_uses_shared_rows_and_folds_and_ignores_obsolete_loads(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = cx.add_window_view(|_, cx| {
            DiffView::new(PathBuf::new(), Arc::new(GitCliPort::default()), cx)
        });
        view.update(cx, |view, cx| {
            view.set_external_source(source("first", 6), cx);
            view.expand_all(cx);
            assert_eq!(view.pending_loads.len(), 4);
            view.set_external_source(source("latest", 6), cx);
            view.set_full_file(true, cx);
            view.set_panel_visible(true, cx);
            view.refresh_now(cx);
            assert!(
                view._snapshot_task.is_none(),
                "remote reviews do not read a checkout"
            );
        });
        cx.run_until_parked();
        view.update(cx, |view, cx| {
            assert_eq!(view.documents.len(), 6);
            assert!(view.pending_loads.is_empty());
            for cached in view.documents.values() {
                assert_eq!(cached.source.head.as_deref(), Some("latest"));
                assert!(cached.document.full_context);
                assert!(
                    cached
                        .document
                        .diff
                        .rows
                        .iter()
                        .any(|row| row.text == "fn latest() {}")
                );
                assert!(
                    cached
                        .document
                        .highlights
                        .iter()
                        .any(|spans| !spans.is_empty())
                );
            }
            view.sync_rows();
            assert!(!view.rows.iter().any(|row| matches!(
                row,
                ReviewRow::Body {
                    row: BodyRow::Fold { .. },
                    ..
                }
            )));
            view.set_full_file(false, cx);
            view.set_preferences(true, true, 15.0, cx);
            view.sync_rows();
            assert!(view.rows.iter().any(|row| matches!(
                row,
                ReviewRow::Body {
                    row: BodyRow::Fold { .. },
                    ..
                }
            )));
            assert!(view.rows.iter().any(|row| matches!(
                row,
                ReviewRow::Body {
                    row: BodyRow::Split { .. },
                    ..
                }
            )));
            assert_eq!(view.layout, DiffLayout::Split);
            assert!(view.wrap);
            assert_eq!(view.font_size, 15.0);
            let document = view.documents["0.rs"].document.clone();
            view.set_external_source(source("latest", 6), cx);
            assert!(Arc::ptr_eq(&document, &view.documents["0.rs"].document));
            view.collapse_all(cx);
            view.sync_rows();
            assert_eq!(view.rows.len(), 6);
        });
    }

    #[gpui::test]
    fn failed_remote_files_are_visible_and_do_not_retry_forever(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| {
            DiffView::new(PathBuf::new(), Arc::new(GitCliPort::default()), cx)
        });
        view.update(cx, |view, cx| {
            let mut remote = source("error", 6);
            remote.load = Arc::new(|_| anyhow::bail!("Remote file unavailable"));
            view.set_external_source(remote, cx);
            view.expand_all(cx);
        });
        cx.run_until_parked();
        view.update(cx, |view, _| {
            assert!(view.pending_loads.is_empty());
            assert_eq!(view.documents.len(), 6);
            assert!(view.documents.values().all(|cached| {
                cached.document.diff.rows.iter().any(|row| {
                    row.kind == GitDiffRowKind::Notice
                        && row.text.contains("Remote file unavailable")
                })
            }));
        });
    }
}
