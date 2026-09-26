use super::*;

#[gpui::test]
fn late_review_delivery_does_not_unlock_another_projects_pending_review(
    cx: &mut gpui::TestAppContext,
) {
    use crate::infrastructure::git::GitCliPort;
    let (view, cx) = cx.add_window_view(|_, cx| {
        DiffView::new(
            PathBuf::from("/tmp/vibra-review-a"),
            Arc::new(GitCliPort::default()),
            cx,
        )
    });
    let comment = |id| ReviewComment {
        id,
        anchor: CommentAnchor {
            path: "main.rs".into(),
            side: CommentSide::New,
            line: 1,
        },
        excerpt: "fn main() {}".into(),
        body: "Please review this.".into(),
    };
    view.update(cx, |view, cx| {
        view.comments.push(comment(1));
        view.send_review(cx);
        let previous_delivery = view.review_delivery.as_ref().unwrap().id;
        view.set_root(PathBuf::from("/tmp/vibra-review-b"), cx);
        view.comments.push(comment(2));
        view.send_review(cx);
        view.resolve_review_delivery(previous_delivery, true, cx);
        view.resolve_review_delivery(previous_delivery, false, cx);
        assert!(
            view.review_delivery.is_some(),
            "a late result must not permit sending the new review twice"
        );
        assert_eq!(view.comments.len(), 1);
        let rejected_delivery = view.review_delivery.as_ref().unwrap().id;
        view.resolve_review_delivery(rejected_delivery, false, cx);
        assert!(view.review_delivery.is_none());
        assert_eq!(
            view.comments.len(),
            1,
            "rejected sends retain their comments"
        );
        view.send_review(cx);
        let retried_delivery = view.review_delivery.as_ref().unwrap().id;
        view.resolve_review_delivery(rejected_delivery, true, cx);
        assert!(
            view.review_delivery.is_some(),
            "a retry of the same comments gets a new identity"
        );
        view.comments.push(comment(3));
        view.resolve_review_delivery(retried_delivery, true, cx);
        assert!(view.review_delivery.is_none());
        assert_eq!(
            view.comments
                .iter()
                .map(|comment| comment.id)
                .collect::<Vec<_>>(),
            [3]
        );
    });
}

#[gpui::test]
fn changing_repository_clears_old_snapshot_before_refresh(cx: &mut gpui::TestAppContext) {
    use crate::infrastructure::git::GitCliPort;

    let old_root = PathBuf::from("/tmp/vibra-old-project");
    let new_root = PathBuf::from("/tmp/vibra-new-project");
    let change = GitFileChange {
        path: "src/main.rs".into(),
        old_path: None,
        status: GitFileStatus::Modified,
        staged: false,
        unstaged: true,
        untracked: false,
        additions: Some(1),
        deletions: Some(0),
    };
    let (view, cx) = cx.add_window_view(|_, cx| {
        let mut view = DiffView::new(old_root.clone(), Arc::new(GitCliPort::default()), cx);
        view.snapshot = Some(GitRepositorySnapshot {
            root: old_root.clone(),
            branch: "main".into(),
            changes: vec![change],
            additions: 1,
            deletions: 0,
        });
        view.status_root = Some(old_root);
        view.status_index = Arc::new(HashMap::from([(
            "src/main.rs".into(),
            GitFileStatus::Modified,
        )]));
        view
    });

    view.update(cx, |view, cx| {
        view.set_root(new_root.clone(), cx);
        assert_eq!(view.context_root, new_root);
        assert!(view.snapshot.is_none());
        assert!(view.status_root.is_none());
        assert!(view.status_index.is_empty());
        assert!(!view.select_path_if_changed("src/main.rs", cx));
    });
}

#[test]
fn diff_source_tracks_worktree_file_changes() {
    let root = std::env::temp_dir().join(format!(
        "vibra-diff-source-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir(&root).expect("create test repository directory");
    std::fs::write(root.join("main.rs"), "fn main() {}\n").expect("write first contents");

    let change = GitFileChange {
        path: "main.rs".into(),
        old_path: None,
        status: GitFileStatus::Modified,
        staged: false,
        unstaged: true,
        untracked: false,
        additions: Some(1),
        deletions: Some(1),
    };
    let snapshot = GitRepositorySnapshot {
        root: root.clone(),
        branch: "main".into(),
        changes: vec![change.clone()],
        additions: 1,
        deletions: 1,
    };
    let before = DiffSource::new(&snapshot, &change, None, None);
    let committed_before = DiffSource::new(&snapshot, &change, Some("parent"), Some("commit"));

    let path = root.join("main.rs");
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&path, "fn test() {}\n").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    let same_size_and_mtime = DiffSource::new(&snapshot, &change, None, None);
    assert_eq!(before.worktree_version.as_ref().unwrap().length, 13);
    assert_eq!(
        before.worktree_version.as_ref().unwrap().modified,
        same_size_and_mtime
            .worktree_version
            .as_ref()
            .unwrap()
            .modified
    );
    assert_ne!(before, same_size_and_mtime);

    std::fs::write(
        root.join("main.rs"),
        "fn main() { println!(\"changed\"); }\n",
    )
    .expect("write changed contents");
    let after = DiffSource::new(&snapshot, &change, None, None);

    assert_ne!(before, after);
    assert_eq!(
        committed_before,
        DiffSource::new(&snapshot, &change, Some("parent"), Some("commit"))
    );
    assert_ne!(
        committed_before,
        DiffSource::new(&snapshot, &change, Some("parent"), Some("other-commit"))
    );
    std::fs::remove_dir_all(root).expect("remove test repository directory");
}

fn git(root: &std::path::Path, arguments: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {arguments:?}");
}

#[gpui::test]
fn loading_more_than_twelve_diffs_does_not_strand_older_files(cx: &mut gpui::TestAppContext) {
    use crate::infrastructure::git::GitCliPort;

    let root = std::env::temp_dir().join(format!(
        "vibra-many-diffs-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.name", "Vibra Test"]);
    git(&root, &["config", "user.email", "vibra@example.invalid"]);
    for index in 0..13 {
        std::fs::write(root.join(format!("file_{index}.rs")), "let value = 0;\n").unwrap();
    }
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "initial"]);
    for index in 0..13 {
        std::fs::write(root.join(format!("file_{index}.rs")), "let value = 1;\n").unwrap();
    }

    let port = Arc::new(GitCliPort::default());
    let snapshot = port.snapshot(&root).unwrap().unwrap();
    let paths: Vec<_> = snapshot
        .changes
        .iter()
        .map(|change| change.path.clone())
        .collect();
    assert_eq!(paths.len(), 13);
    let (view, cx) = cx.add_window_view(|_, cx| {
        let mut view = DiffView::new(root.clone(), port, cx);
        view.snapshot = Some(snapshot);
        view
    });
    view.update(cx, |view, cx| {
        for path in paths {
            view.load_diff(path, cx);
        }
        assert_eq!(view.pending_loads.len(), 13);
    });
    cx.run_until_parked();
    view.update(cx, |view, _| {
        assert!(view.pending_loads.is_empty());
        assert_eq!(view.documents.len(), 13);
    });
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn review_list_is_one_flat_list_with_pinned_headers_and_comments(cx: &mut gpui::TestAppContext) {
    use crate::infrastructure::git::GitCliPort;
    use std::cell::RefCell;
    use std::rc::Rc;

    let root = std::env::temp_dir().join(format!(
        "vibra-review-list-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.name", "Vibra Test"]);
    git(&root, &["config", "user.email", "vibra@example.invalid"]);
    let original: String = (1..=120)
        .map(|line| format!("let line_{line} = {line};\n"))
        .collect();
    std::fs::write(root.join("a.rs"), &original).unwrap();
    git(&root, &["add", "a.rs"]);
    git(&root, &["commit", "-qm", "initial"]);
    let changed: String = (1..=120)
        .map(|line| {
            if line % 3 == 0 {
                format!(
                    "let line_{line} = {}; // {}\n",
                    line * 2,
                    "wide ".repeat(120)
                )
            } else {
                format!("let line_{line} = {line};\n")
            }
        })
        .collect();
    std::fs::write(root.join("a.rs"), changed).unwrap();
    std::fs::write(root.join("b.rs"), "fn added() {}\n").unwrap();

    let (view, cx) = cx.add_window_view(|_, cx| {
        let mut view = DiffView::new(root.clone(), Arc::new(GitCliPort::default()), cx);
        view.set_review_expanded(true, cx);
        view.set_panel_visible(true, cx);
        view
    });
    let sent = Rc::new(RefCell::new(Vec::<String>::new()));
    let sink = sent.clone();
    cx.update(|_, cx| {
        cx.subscribe(&view, move |_, event: &DiffViewEvent, _| {
            if let DiffViewEvent::SendReview { prompt, .. } = event {
                sink.borrow_mut().push(prompt.clone());
            }
        })
        .detach();
    });
    cx.run_until_parked();
    let draw = |cx: &mut gpui::VisualTestContext| {
        cx.update(|window, cx| window.draw(cx).clear());
        cx.run_until_parked();
    };

    view.update(cx, |view, cx| {
        view.toggle_path("a.rs".into(), cx);
        view.toggle_path("b.rs".into(), cx);
    });
    cx.run_until_parked();
    draw(cx);
    draw(cx);

    view.update(cx, |view, _| {
        assert!(view.folds.is_empty(), "fold tweens settle");
        let headers = view
            .rows
            .iter()
            .filter(|row| matches!(row, ReviewRow::FileHeader { .. }))
            .count();
        assert_eq!(headers, 2);
        let bodies = view
            .rows
            .iter()
            .filter(|row| matches!(row, ReviewRow::Body { .. }))
            .count();
        assert!(bodies > 80, "every diff line is its own row: {bodies}");
        assert!(view.h_max > 0.0, "the widest line overflows the code plane");
    });

    // Draw while a cached diff is folding, before its timer can settle. GPUI
    // holds the list state mutably while invoking the row renderer.
    for layout in [DiffLayout::Unified, DiffLayout::Split] {
        view.update(cx, |view, cx| view.set_layout(layout, cx));
        draw(cx);
        for expanding in [false, true] {
            view.update(cx, |view, cx| {
                view.toggle_path("a.rs".into(), cx);
                assert_eq!(view.folds["a.rs"].expanding, expanding);
            });
            cx.update(|window, cx| window.draw(cx).clear());
            view.update(cx, |view, _| {
                assert!(
                    view.rows
                        .iter()
                        .any(|row| matches!(row, ReviewRow::Folding { .. })),
                    "the animation row must actually be rendered"
                );
            });
            cx.executor().advance_clock(FOLD_DURATION);
            cx.run_until_parked();
            draw(cx);
            view.update(cx, |view, _| assert!(view.folds.is_empty()));
        }
    }
    view.update(cx, |view, cx| view.set_layout(DiffLayout::Unified, cx));
    draw(cx);

    // Sideways gestures move only the code plane, never the file list.
    let top_before = view.read_with(cx, |view, _| view.list_state.logical_scroll_top().item_ix);
    cx.simulate_event(ScrollWheelEvent {
        position: point(px(300.0), px(400.0)),
        delta: gpui::ScrollDelta::Pixels(point(px(-80.0), px(-3.0))),
        ..Default::default()
    });
    view.update(cx, |view, _| {
        assert_eq!(view.h_offset, 80.0);
        assert_eq!(view.list_state.logical_scroll_top().item_ix, top_before);
    });

    // Scrolling into a file pins its header over the list.
    view.update(cx, |view, _| {
        view.list_state.scroll_to(ListOffset {
            item_ix: 30,
            offset_in_item: px(0.0),
        })
    });
    draw(cx);
    view.update(cx, |view, _| {
        let (file, offset) = view.sticky_header().expect("header pinned");
        assert_eq!(view.row_files[file].path, "a.rs");
        assert_eq!(offset, 0.0);
    });

    // Split pairs rows without losing the scroll position's file.
    view.update(cx, |view, cx| view.set_layout(DiffLayout::Split, cx));
    draw(cx);
    view.update(cx, |view, _| {
        assert!(view.rows.iter().any(|row| matches!(
            row,
            ReviewRow::Body {
                row: BodyRow::Split { .. },
                ..
            }
        )));
        let top = view.list_state.logical_scroll_top().item_ix;
        assert_eq!(
            view.rows[top]
                .file()
                .map(|file| view.row_files[file].path.as_str()),
            Some("a.rs")
        );
    });

    // A comment is drafted on a line, kept, and pasted as one prompt.
    view.update_in(cx, |view, window, cx| {
        view.open_draft(
            CommentAnchor {
                path: "a.rs".into(),
                side: CommentSide::New,
                line: 3,
            },
            "let line_3 = 6;".into(),
            window,
            cx,
        );
        view.draft.as_mut().unwrap().body = "Keep the original value.".into();
        view.commit_draft();
    });
    draw(cx);
    view.update(cx, |view, cx| {
        assert!(
            view.rows
                .iter()
                .any(|row| matches!(row, ReviewRow::Comment { .. }))
        );
        view.send_review(cx);
        view.send_review(cx);
        assert!(view.review_delivery.is_some());
        assert_eq!(
            view.comments.len(),
            1,
            "comments remain until delivery succeeds"
        );
        let delivery = view.review_delivery.as_ref().unwrap().id;
        view.resolve_review_delivery(delivery, true, cx);
        assert!(view.comments.is_empty());
    });
    let sent = sent.borrow();
    assert_eq!(sent.len(), 1, "a repeated click must not paste twice");
    let prompt = &sent[0];
    assert!(prompt.contains("a.rs:3"));
    assert!(prompt.contains("Keep the original value."));

    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn opening_a_review_file_preserves_comments_and_cached_diff_when_reopened(
    cx: &mut gpui::TestAppContext,
) {
    use crate::infrastructure::git::GitCliPort;
    use crate::ports::git::GitDiff;

    let path = "src/main.rs";
    let change = GitFileChange {
        path: path.into(),
        old_path: None,
        status: GitFileStatus::Modified,
        staged: false,
        unstaged: true,
        untracked: false,
        additions: Some(1),
        deletions: Some(0),
    };
    let snapshot = GitRepositorySnapshot {
        root: std::env::temp_dir().join(format!("vibra-review-reopen-{}", uuid::Uuid::new_v4())),
        branch: "main".into(),
        changes: vec![change.clone()],
        additions: 1,
        deletions: 0,
    };
    let document = Arc::new(DiffDocument::prepare(GitDiff {
        path: path.into(),
        rows: vec![GitDiffRow {
            old_line: None,
            new_line: Some(1),
            kind: GitDiffRowKind::Addition,
            text: "fn main() {}".into(),
        }],
        additions: 1,
        deletions: 0,
        binary: false,
        truncated: false,
    }));
    let (view, cx) = cx.add_window_view(|_, cx| {
        let mut view = DiffView::new(snapshot.root.clone(), Arc::new(GitCliPort::default()), cx);
        view.documents.insert(
            path.into(),
            CachedDiffDocument {
                source: DiffSource::new(&snapshot, &change, None, None),
                document: document.clone(),
            },
        );
        view.snapshot = Some(snapshot.clone());
        view
    });

    let returned_to_terminal = std::rc::Rc::new(std::cell::Cell::new(false));
    let returned = returned_to_terminal.clone();
    cx.update(|_, cx| {
        cx.subscribe(&view, move |_, event: &DiffViewEvent, _| {
            if matches!(event, DiffViewEvent::ReturnToTerminal) {
                returned.set(true);
            }
        })
        .detach();
    });
    view.update_in(cx, |view, window, cx| {
        assert!(!view.review_expanded());
        assert!(view.expanded.is_empty());
        view.open_review_path(path.into(), window, cx);
        assert!(view.review_expanded());
        assert_eq!(view.selected_review_path.as_deref(), Some(path));
        assert_eq!(view.pending_reveal.as_deref(), Some(path));
        assert!(view.expanded.contains(path));
        assert!(view.focus_handle.is_focused(window));

        view.open_draft(
            CommentAnchor {
                path: path.into(),
                side: CommentSide::New,
                line: 1,
            },
            "fn main() {}".into(),
            window,
            cx,
        );
        view.draft.as_mut().unwrap().body = "Keep this entry point.".into();
        view.commit_draft();
        let comments = view.comments.clone();

        view.on_key_down(
            &gpui::KeyDownEvent {
                keystroke: gpui::Keystroke::parse("escape").unwrap(),
                is_held: false,
            },
            window,
            cx,
        );
        assert!(!view.review_expanded());
        view.open_review_path(path.into(), window, cx);
        assert!(view.review_expanded());
        assert_eq!(view.comments, comments);
        assert_eq!(view.snapshot, Some(snapshot));
        assert!(Arc::ptr_eq(view.document(path).unwrap(), &document));
        assert!(
            view.pending_loads.is_empty(),
            "reopening reuses the prepared diff"
        );
        view.sync_rows();
        assert!(
            view.rows
                .iter()
                .any(|row| matches!(row, ReviewRow::Comment { .. }))
        );
    });
    assert!(
        returned_to_terminal.get(),
        "Escape must request terminal focus"
    );
}

#[gpui::test]
fn changing_review_scope_keeps_central_review_open(cx: &mut gpui::TestAppContext) {
    use crate::infrastructure::git::GitCliPort;

    let root = std::env::temp_dir().join(format!("vibra-review-scope-{}", uuid::Uuid::new_v4()));
    let (view, cx) =
        cx.add_window_view(|_, cx| DiffView::new(root, Arc::new(GitCliPort::default()), cx));
    view.update(cx, |view, cx| view.set_review_expanded(true, cx));

    let changed = std::rc::Rc::new(std::cell::Cell::new(false));
    let observed = changed.clone();
    cx.update(|_, cx| {
        cx.subscribe(&view, move |_, event: &DiffViewEvent, _| {
            if matches!(event, DiffViewEvent::Changed) {
                observed.set(true);
            }
        })
        .detach();
    });

    for mode in [
        GitPanelMode::Branch,
        GitPanelMode::LatestTurn,
        GitPanelMode::History,
        GitPanelMode::Worktree,
    ] {
        changed.set(false);
        view.update(cx, |view, cx| {
            view.set_mode(mode, cx);
            assert!(
                view.review_expanded(),
                "switching scope must keep the review open"
            );
            assert_eq!(view.review_title(), mode.label());
        });
        assert!(changed.get(), "scope changes must update the workspace tab");
    }
}

#[test]
fn gutters_grow_with_line_numbers_and_font() {
    let metrics = RowMetrics {
        font_size: 12.0,
        line_height: 22.0,
        hunk_height: 28.0,
        fold_height: 38.0,
        char_width: 7.0,
        wrap: false,
        h_offset: 0.0,
    };
    assert_eq!(metrics.gutter_width(9), metrics.gutter_width(999));
    assert!(metrics.gutter_width(10_000) > metrics.gutter_width(999));
    let larger = RowMetrics {
        char_width: 9.0,
        ..metrics
    };
    assert!(larger.gutter_width(999) > metrics.gutter_width(999));
}

#[test]
fn elapsed_labels_stay_short() {
    assert_eq!(elapsed_label(Duration::from_secs(20)), "started just now");
    assert_eq!(
        elapsed_label(Duration::from_secs(5 * 60)),
        "started 5 min ago"
    );
    assert_eq!(
        elapsed_label(Duration::from_secs(3 * 3600)),
        "started 3 h ago"
    );
}

fn committed_repository() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "vibra-changes-panel-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&root).unwrap();
    for arguments in [
        &["init", "-q"][..],
        &["config", "user.name", "Vibra Test"],
        &["config", "user.email", "vibra@example.invalid"],
    ] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(arguments)
                .status()
                .unwrap()
                .success()
        );
    }
    std::fs::write(root.join("notes.txt"), "one\n").unwrap();
    for arguments in [&["add", "."][..], &["commit", "-qm", "initial"]] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(arguments)
                .status()
                .unwrap()
                .success()
        );
    }
    root
}

#[gpui::test]
fn changes_panel_commits_and_reviews_commits_from_the_graph(cx: &mut gpui::TestAppContext) {
    use crate::infrastructure::git::GitCliPort;

    let root = committed_repository();
    std::fs::write(root.join("notes.txt"), "one\ntwo\n").unwrap();
    let (view, cx) = cx.add_window_view(|_, cx| {
        let mut view = DiffView::new(root.clone(), Arc::new(GitCliPort::default()), cx);
        view.set_panel_visible(true, cx);
        view
    });
    cx.run_until_parked();

    view.update(cx, |view, cx| {
        assert_eq!(view.snapshot.as_ref().unwrap().changes.len(), 1);
        assert_eq!(view.history.as_ref().unwrap().commits.len(), 1);
        view.changes.message = "Add a second line".into();
        view.run_commit(changes_panel::CommitAction::Commit, cx);
    });
    cx.run_until_parked();

    let second = view.update(cx, |view, _| {
        assert!(view.changes.message.is_empty());
        let (feedback, failed) = view.changes.feedback.clone().unwrap();
        assert!(!failed, "{feedback}");
        assert!(view.snapshot.as_ref().unwrap().changes.is_empty());
        let history = view.history.as_ref().unwrap();
        assert_eq!(history.commits[0].subject, "Add a second line");
        history.commits[0].clone()
    });

    view.update(cx, |view, cx| {
        view.open_commit_from_graph(second.clone(), cx);
        assert_eq!(view.mode, GitPanelMode::History);
        assert!(view.review_expanded());
        assert!(view.review_focused(), "reviews open as a full tab");
        assert_eq!(view.review_title(), "Add a second line");
    });
    cx.run_until_parked();
    view.update(cx, |view, cx| {
        assert_eq!(
            view.commit_changes.as_ref().unwrap().snapshot.changes.len(),
            1
        );
        view.set_review_expanded(false, cx);
        assert_eq!(view.mode, GitPanelMode::Worktree);
        assert!(view.selected_commit.is_none());
    });
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn changes_panel_stages_and_unstages_files(cx: &mut gpui::TestAppContext) {
    use crate::infrastructure::git::GitCliPort;

    let root = committed_repository();
    std::fs::write(root.join("notes.txt"), "one\ntwo\n").unwrap();
    std::fs::write(root.join("new.txt"), "new\n").unwrap();
    let (view, cx) = cx.add_window_view(|_, cx| {
        let mut view = DiffView::new(root.clone(), Arc::new(GitCliPort::default()), cx);
        view.set_panel_visible(true, cx);
        view
    });
    cx.run_until_parked();
    view.update(cx, |view, cx| {
        view.stage_paths(vec!["notes.txt".into()], true, cx);
    });
    cx.run_until_parked();
    view.update(cx, |view, cx| {
        let changes = &view.snapshot.as_ref().unwrap().changes;
        let notes = changes
            .iter()
            .find(|change| change.path == "notes.txt")
            .unwrap();
        let new = changes
            .iter()
            .find(|change| change.path == "new.txt")
            .unwrap();
        assert!(notes.staged);
        assert!(!new.staged);
        view.stage_paths(vec!["notes.txt".into()], false, cx);
    });
    cx.run_until_parked();
    view.update(cx, |view, _| {
        assert!(
            view.snapshot
                .as_ref()
                .unwrap()
                .changes
                .iter()
                .all(|change| !change.staged)
        );
    });
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn pending_commit_results_stay_with_their_original_repository(cx: &mut gpui::TestAppContext) {
    use crate::infrastructure::git::GitCliPort;
    let root = committed_repository();
    let second = committed_repository();
    std::fs::write(root.join("notes.txt"), "one\ntwo\n").unwrap();
    let (view, cx) = cx.add_window_view(|_, cx| {
        let mut view = DiffView::new(root.clone(), Arc::new(GitCliPort::default()), cx);
        view.set_panel_visible(true, cx);
        view
    });
    cx.run_until_parked();
    view.update(cx, |view, cx| {
        view.changes.message = "Commit in first project".into();
        view.run_commit(changes_panel::CommitAction::Commit, cx);
        view.set_root(second.clone(), cx);
        assert!(view.changes.message.is_empty());
        assert!(view.changes.feedback.is_none());
        view.changes.message = "Draft in second project".into();
    });
    cx.run_until_parked();
    view.update(cx, |view, _| {
        assert_eq!(view.changes.message, "Draft in second project");
        assert!(view.changes.feedback.is_none());
        assert_eq!(
            view.snapshot.as_ref().unwrap().root,
            second.canonicalize().unwrap()
        );
    });
    let subject = std::process::Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["log", "-1", "--format=%s"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8(subject.stdout).unwrap().trim(),
        "Commit in first project"
    );
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(second).unwrap();
}
