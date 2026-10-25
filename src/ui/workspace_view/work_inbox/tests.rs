use super::super::tests::open_recording_workspace;
use super::*;
use crate::domain::work_items::{WorkDetail, fixture};
use crate::infrastructure::work_items::WorkItemsPage;
use std::collections::HashSet;

#[gpui::test]
fn inbox_reuses_the_workspace_diff_and_prepares_review_comments_without_execution(
    cx: &mut gpui::TestAppContext,
) {
    use crate::ui::diff_view::DiffViewEvent;
    let (root, _, inputs, window) = open_recording_workspace(cx, "inbox-shared-diff");
    let item = fixture();
    let review = window
        .update(cx, |view, _, cx| {
            view.work_inbox.github.items.push(item.clone());
            view.work_inbox.selected = Some(item.url.clone());
            view.work_inbox
                .details
                .entry(item.url.clone())
                .or_default()
                .diff
                .data = Some(WorkDiff::default());
            view.sync_inbox_review(&item.url, cx);
            let review = view.work_inbox.details[&item.url].review.clone().unwrap();
            assert_ne!(review.entity_id(), view.diff_view.entity_id());
            view.sync_inbox_review(&item.url, cx);
            assert_eq!(
                review.entity_id(),
                view.work_inbox.details[&item.url]
                    .review
                    .as_ref()
                    .unwrap()
                    .entity_id()
            );
            view.set_inbox_code_mode(CodeMode::FullFile, cx);
            view.work_inbox.composer_note = "Existing instructions".into();
            review
        })
        .unwrap();
    let writes = inputs.lock().unwrap().len();
    review.update(cx, |_, cx| {
        cx.emit(DiffViewEvent::PreferencesChanged {
            split: true,
            wrap: true,
        });
        cx.emit(DiffViewEvent::SendReview {
            prompt: "main.rs:2: Please simplify this function.".into(),
            delivery_id: Uuid::new_v4(),
        });
    });
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert!(view.settings.diff_split && view.settings.diff_wrap);
            assert!(view.work_inbox.composer_open);
            assert!(
                view.work_inbox
                    .composer_note
                    .starts_with("Existing instructions\n\n")
            );
            assert!(view.work_inbox.composer_note.contains(&item.url));
            assert!(
                view.work_inbox
                    .composer_note
                    .contains("Please simplify this function.")
            );
            assert_eq!(inputs.lock().unwrap().len(), writes);
            view.work_inbox.selected = None;
            review.update(cx, |_, cx| {
                cx.emit(DiffViewEvent::SendReview {
                    prompt: "Obsolete selection".into(),
                    delivery_id: Uuid::new_v4(),
                });
            });
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, window, _| {
            assert!(!view.work_inbox.composer_note.contains("Obsolete selection"));
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn work_inbox_cached_list_stays_visible_during_refresh_and_network_failure() {
    let mut feed = SourceFeed {
        generation: 1,
        loading: true,
        ..Default::default()
    };
    let page = || WorkItemsPage {
        items: vec![fixture()],
        connected: true,
        ..Default::default()
    };
    assert!(!feed.restore(0, page()));
    assert!(feed.items.is_empty());
    assert!(feed.restore(1, page()));
    assert_eq!(feed.items.len(), 1);
    assert!(feed.loading);
    assert!(feed.fetched.is_none());
    assert!(feed.accept(1, Err(anyhow::anyhow!("offline"))));
    assert_eq!(feed.items.len(), 1);
    assert!(!feed.loading);
    assert!(feed.error.is_some());
    assert!(!feed.restore(1, WorkItemsPage::default()));
    assert_eq!(feed.items.len(), 1);
    // Live data replaces the snapshot, including a now-empty inbox.
    assert!(feed.accept(
        1,
        Ok(WorkItemsPage {
            connected: true,
            ..Default::default()
        })
    ));
    assert!(feed.items.is_empty());
    assert!(feed.error.is_none());
}

#[gpui::test]
fn inbox_pruning_retains_active_writes_and_forgets_closed_discussions(
    cx: &mut gpui::TestAppContext,
) {
    let (root, snapshot, _, window) = open_recording_workspace(cx, "inbox-pruning");
    let pane = snapshot.selected_session().unwrap().id;
    window
        .update(cx, |view, window, cx| {
            view.work_inbox
                .details
                .entry("posting".into())
                .or_default()
                .posting = true;
            view.work_inbox
                .details
                .entry("mutation".into())
                .or_default()
                .mutation_busy = true;
            view.work_inbox
                .details
                .entry("discussion".into())
                .or_default();
            view.work_inbox
                .details
                .entry("obsolete".into())
                .or_default();
            view.work_inbox
                .discussion_panes
                .insert("discussion".into(), pane);

            view.prune_inbox_details();
            assert_eq!(view.work_inbox.details.len(), 3);
            assert!(view.work_inbox.details.contains_key("posting"));
            assert!(view.work_inbox.details.contains_key("mutation"));
            assert!(view.work_inbox.details.contains_key("discussion"));

            assert!(view.snapshot.close_terminal(pane));
            view.reconcile_terminal_views(cx);
            assert!(view.work_inbox.discussion_panes.is_empty());
            view.work_inbox.details.get_mut("posting").unwrap().posting = false;
            view.work_inbox
                .details
                .get_mut("mutation")
                .unwrap()
                .mutation_busy = false;
            view.prune_inbox_details();
            assert!(view.work_inbox.details.is_empty());
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn entering_inbox_scopes_the_active_project_and_reselects_visible_open_work(
    cx: &mut gpui::TestAppContext,
) {
    let (root, snapshot, _, window) = open_recording_workspace(cx, "inbox-project-scope");
    let first_project = snapshot.selected_project_id.unwrap();
    let other_root = root.join("other");
    std::fs::create_dir_all(&other_root).unwrap();
    window
        .update(cx, |view, window, cx| {
            let second_project = view.snapshot.add_project(&other_root);
            view.snapshot.select_project(first_project);
            let mut first = fixture();
            first.project_id = Some(first_project);
            let mut second = first.clone();
            second.url = "https://github.com/other/app/issues/1".into();
            second.project_id = Some(second_project);
            let mut closed = first.clone();
            closed.url = "https://github.com/demo/app/issues/43".into();
            closed.status = WorkStatus::Closed;
            let mut merged = closed.clone();
            merged.url = "https://github.com/demo/app/pull/44".into();
            merged.kind = WorkKind::PullRequest;
            merged.status = WorkStatus::Merged;
            view.work_inbox.github.items = vec![
                closed.clone(),
                second.clone(),
                first.clone(),
                merged.clone(),
            ];
            view.work_inbox.selected = Some(second.url.clone());
            view.settings.inbox.hidden_projects = vec![first_project];
            view.settings.inbox.tab = RepositoryTab::Issues;
            view.select_section(WorkspaceSection::Inbox, window, cx);
            assert_eq!(view.work_inbox.filter.project, Some(first_project));
            assert_eq!(
                view.work_inbox.selected.as_deref(),
                Some(first.url.as_str())
            );
            assert!(
                !view
                    .work_inbox
                    .filter
                    .matches(&second, &view.settings.inbox)
            );
            assert!(
                !view
                    .work_inbox
                    .filter
                    .matches(&closed, &view.settings.inbox)
            );
            assert!(
                !view
                    .work_inbox
                    .filter
                    .matches(&merged, &view.settings.inbox)
            );
            assert_eq!(view.inbox_query().status, Some(WorkStatus::Open));
            assert!(view.inbox_project_selected(first_project));
            assert!(!view.inbox_project_selected(second_project));

            // A manual broader selection survives refreshes and clicks while already in Inbox.
            view.toggle_inbox_project(second_project, cx);
            assert!(
                view.work_inbox
                    .filter
                    .matches(&second, &view.settings.inbox)
            );
            assert!(view.work_inbox.filter.matches(&first, &view.settings.inbox));
            view.select_section(WorkspaceSection::Inbox, window, cx);
            assert!(view.work_inbox.filter.project.is_none());

            view.select_section(WorkspaceSection::Workspace, window, cx);
            view.snapshot.select_project(second_project);
            view.select_section(WorkspaceSection::Inbox, window, cx);
            assert_eq!(view.work_inbox.filter.project, Some(second_project));
            assert_eq!(
                view.work_inbox.selected.as_deref(),
                Some(second.url.as_str())
            );

            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn work_inbox_prefetch_does_not_select_or_mark_tasks_read_until_visible(
    cx: &mut gpui::TestAppContext,
) {
    let (root, _, _, window) = open_recording_workspace(cx, "inbox-prefetch");
    window
        .update(cx, |view, window, cx| {
            let item = fixture();
            view.work_inbox.github.apply(Ok(WorkItemsPage {
                items: vec![item.clone()],
                connected: true,
                ..Default::default()
            }));
            view.settings.inbox.tab = RepositoryTab::Issues;
            view.inbox_items_updated(true, cx);
            assert!(view.work_inbox.selected.is_none());
            assert!(item.unread(&view.settings.inbox.seen));
            assert!(view.work_inbox.details.is_empty());
            view.workspace_section = WorkspaceSection::Inbox;
            view.work_inbox.activity = true;
            view.inbox_items_updated(false, cx);
            assert!(view.work_inbox.selected.is_none());
            assert!(item.unread(&view.settings.inbox.seen));
            view.work_inbox.activity = false;
            view.inbox_items_updated(false, cx);
            assert_eq!(view.work_inbox.selected.as_deref(), Some(item.url.as_str()));
            assert!(!item.unread(&view.settings.inbox.seen));
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn work_inbox_feed_keeps_its_items_through_errors_and_ignores_stale_results() {
    let mut inbox = WorkInbox::default();
    inbox.github.apply(Ok(WorkItemsPage {
        items: vec![fixture()],
        connected: true,
        ..Default::default()
    }));
    assert_eq!(inbox.github.items.len(), 1);
    assert!(inbox.github.error.is_none());
    inbox
        .github
        .apply(Err(anyhow::anyhow!("Network unavailable")));
    assert_eq!(inbox.github.items.len(), 1);
    assert!(inbox.github.error.is_some());
    inbox.github.apply(Ok(WorkItemsPage {
        connected: true,
        warning: Some("PR query unavailable".into()),
        failed_scopes: vec![("demo/app".into(), WorkKind::Issue)],
        ..Default::default()
    }));
    assert_eq!(inbox.github.items.len(), 1);
    assert!(inbox.github.error.is_some());
    inbox.github.generation = 2;
    assert!(!inbox.github.accept(1, Ok(WorkItemsPage::default())));
    assert_eq!(inbox.github.items.len(), 1);
    assert!(inbox.github.accept(2, Ok(WorkItemsPage::default())));
    assert!(inbox.github.items.is_empty());
    assert!(inbox.github.error.is_none());
}

#[gpui::test]
fn work_inbox_prepares_a_draft_then_starts_a_linked_session_in_its_project(
    cx: &mut gpui::TestAppContext,
) {
    let (root, snapshot, inputs, window) = open_recording_workspace(cx, "work-inbox-start");
    let original = snapshot.selected_project_id.unwrap();
    let other_root = root.join("other");
    std::fs::create_dir_all(&other_root).unwrap();
    window
        .update(cx, |view, window, cx| {
            let other = view.snapshot.add_project(&other_root);
            view.snapshot.select_project(original);
            let mut item = fixture();
            item.project_id = Some(other);
            view.work_inbox.github.items.push(item.clone());
            view.select_section(WorkspaceSection::Inbox, window, cx);
            view.select_work_item(&item.url, cx);
            assert_eq!(view.work_inbox.target_project, Some(other));
            assert!(!item.unread(&view.settings.inbox.seen));
            let before = view.snapshot.terminal_sessions().count();
            let writes = inputs.lock().unwrap().len();
            view.open_inbox_composer(cx);
            assert_eq!(view.snapshot.terminal_sessions().count(), before);
            assert_eq!(inputs.lock().unwrap().len(), writes);
            view.work_inbox.composer_note = "Conserva los cambios del usuario".into();
            view.start_work_item(window, cx);
            assert_eq!(view.snapshot.selected_project_id, Some(other));
            assert_eq!(view.workspace_section, WorkspaceSection::Workspace);
            assert_eq!(view.snapshot.terminal_sessions().count(), before + 1);
            let pane = view.snapshot.selected_session().unwrap().id;
            let sent = inputs.lock().unwrap();
            let (target, bytes) = sent.last().unwrap();
            assert_eq!(*target, pane);
            let command = String::from_utf8(bytes.clone()).unwrap();
            assert!(command.starts_with("/bin/sh -c "));
            assert!(command.ends_with('\r'));
            assert!(!command.contains('\n'));
            // The recording port does not run commands: clean its prompt file.
            let name = command
                .split("vibra-inbox-")
                .nth(1)
                .unwrap()
                .split(".txt")
                .next()
                .unwrap();
            let path = std::env::temp_dir().join(format!("vibra-inbox-{name}.txt"));
            assert!(
                std::fs::read_to_string(&path)
                    .unwrap()
                    .contains("Conserva los cambios del usuario")
            );
            std::fs::remove_file(path).unwrap();
            drop(sent);
            assert_eq!(view.settings.inbox.linked_sessions[&item.url], vec![pane]);
            let saved = serde_json::to_string(&view.settings.inbox).unwrap();
            let restored: crate::domain::work_items::InboxPreferences =
                serde_json::from_str(&saved).unwrap();
            assert_eq!(restored.linked_sessions[&item.url], vec![pane]);
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn details_discard_stale_loads_and_preserve_data_on_refresh_error() {
    let mut load = LoadState::<String> {
        revision: 1,
        loading: true,
        ..Default::default()
    };
    load.invalidate();
    load.apply(1, Ok("old response".into()));
    assert!(load.data.is_none());
    load.apply(2, Ok("current response".into()));
    load.revision = 3;
    load.apply(3, Err(anyhow::anyhow!("offline")));
    assert_eq!(load.data.as_deref(), Some("current response"));
    assert!(load.error.is_some());
}

#[gpui::test]
fn inbox_discussion_keeps_workspace_selection_and_isolates_terminal_visibility(
    cx: &mut gpui::TestAppContext,
) {
    let (root, snapshot, inputs, window) = open_recording_workspace(cx, "inbox-discussion");
    window
        .update(cx, |view, window, cx| {
            let mut item = fixture();
            item.project_id = snapshot.selected_project_id;
            view.work_inbox.github.items.push(item.clone());
            view.select_section(WorkspaceSection::Inbox, window, cx);
            view.select_work_item(&item.url, cx);
            let selected = view.snapshot.selected_session().unwrap().id;
            view.open_inbox_discussion(window, cx);
            let pane = view.visible_inbox_terminal().unwrap();
            assert_ne!(pane, selected);
            assert_eq!(view.snapshot.selected_session().unwrap().id, selected);
            assert_eq!(view.workspace_section, WorkspaceSection::Inbox);
            assert_eq!(view.visible_terminal_ids(cx), HashSet::from([pane]));
            let sent = inputs.lock().unwrap();
            let command = String::from_utf8(sent.last().unwrap().1.clone()).unwrap();
            let name = command
                .split("vibra-inbox-")
                .nth(1)
                .unwrap()
                .split(".txt")
                .next()
                .unwrap();
            let path = std::env::temp_dir().join(format!("vibra-inbox-{name}.txt"));
            let prompt = std::fs::read_to_string(&path).unwrap();
            assert!(prompt.contains(&item.url));
            assert!(prompt.contains("read-only remote queries"));
            std::fs::remove_file(path).unwrap();
            let writes = sent.len();
            drop(sent);
            view.open_inbox_discussion(window, cx);
            assert_eq!(inputs.lock().unwrap().len(), writes);
            view.work_inbox.discussion_open = false;
            assert!(view.visible_terminal_ids(cx).is_empty());
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn task_drafts_and_pr_confirmation_stay_with_their_original_item(cx: &mut gpui::TestAppContext) {
    let (root, _, inputs, window) = open_recording_workspace(cx, "inbox-targets");
    window
        .update(cx, |view, window, cx| {
            let first = fixture();
            let mut second = fixture();
            second.url = "https://github.com/demo/app/pull/43".into();
            second.title = "Second task".into();
            second.kind = WorkKind::PullRequest;
            view.work_inbox.github.items = vec![first.clone(), second.clone()];
            view.select_section(WorkspaceSection::Inbox, window, cx);
            view.select_work_item(&first.url, cx);
            view.work_inbox
                .details
                .entry(first.url.clone())
                .or_default()
                .draft = "Unsent comment".into();
            view.select_work_item(&second.url, cx);
            view.work_inbox
                .details
                .entry(second.url.clone())
                .or_default()
                .summary
                .data = Some(WorkDetail {
                head_oid: "original-head".into(),
                ..Default::default()
            });
            let writes = inputs.lock().unwrap().len();
            view.propose_inbox_pr_action(&second.url, PrAction::Squash);
            view.work_inbox
                .details
                .get_mut(&second.url)
                .unwrap()
                .summary
                .data
                .as_mut()
                .unwrap()
                .head_oid = "new-head".into();
            assert_eq!(
                view.work_inbox.confirmation.as_ref().unwrap().head_oid,
                "original-head"
            );
            assert_eq!(inputs.lock().unwrap().len(), writes);
            view.select_work_item(&first.url, cx);
            assert!(view.work_inbox.confirmation.is_none());
            assert_eq!(view.work_inbox.details[&first.url].draft, "Unsent comment");
            view.work_inbox.search_editing = true;
            view.work_inbox.filter.query = "Second".into();
            let event = gpui::KeyDownEvent {
                keystroke: gpui::Keystroke {
                    key_char: Some("x".into()),
                    ..gpui::Keystroke::parse("x").unwrap()
                },
                is_held: false,
            };
            assert!(view.handle_inbox_key(&event, window, cx));
            assert!(view.work_inbox.selected.is_none());
            assert!(view.work_inbox.search_editing);
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn work_inbox_missing_target_never_falls_back_to_another_project(cx: &mut gpui::TestAppContext) {
    let (root, snapshot, inputs, window) = open_recording_workspace(cx, "work-inbox-missing");
    window
        .update(cx, |view, window, cx| {
            let mut item = fixture();
            item.project_id = Some(Uuid::new_v4());
            view.work_inbox.github.items.push(item.clone());
            view.select_work_item(&item.url, cx);
            let before = view.snapshot.terminal_sessions().count();
            let writes = inputs.lock().unwrap().len();
            view.start_work_item(window, cx);
            assert_eq!(view.snapshot.terminal_sessions().count(), before);
            assert_eq!(inputs.lock().unwrap().len(), writes);
            assert_eq!(
                view.snapshot.selected_project_id,
                snapshot.selected_project_id
            );
            assert!(view.work_inbox.action_error.is_some());
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

fn key(text: &str) -> gpui::KeyDownEvent {
    let mut keystroke = gpui::Keystroke::parse(text).unwrap();
    if keystroke.key_char.is_none() && keystroke.key.chars().count() == 1 {
        keystroke.key_char = Some(keystroke.key.clone());
    }
    gpui::KeyDownEvent {
        keystroke,
        is_held: false,
    }
}

#[gpui::test]
fn review_tabs_group_by_relation_and_keys_walk_tasks_and_tabs(cx: &mut gpui::TestAppContext) {
    use crate::domain::work_items::{WorkRelation, pull_fixture};
    let (root, _, _, window) = open_recording_workspace(cx, "inbox-keys");
    window
        .update(cx, |view, window, cx| {
            let mut other = pull_fixture();
            other.url = "https://github.com/demo/app/pull/1".into();
            other.updated_at = 300;
            let mut review = pull_fixture();
            review.url = "https://github.com/demo/app/pull/2".into();
            review.relation = WorkRelation::ReviewRequested;
            review.updated_at = 200;
            let mut mine = pull_fixture();
            mine.url = "https://github.com/demo/app/pull/3".into();
            mine.relation = WorkRelation::Authored;
            mine.updated_at = 250;
            let mut issue = fixture();
            issue.relation = WorkRelation::Assigned;
            issue.updated_at = 100;
            view.work_inbox.github.items =
                vec![other.clone(), mine.clone(), review.clone(), issue.clone()];
            view.select_section(WorkspaceSection::Inbox, window, cx);
            view.work_inbox.filter.project = None;
            view.work_inbox.selected = None;
            view.sync_inbox_selection(cx);
            // Pull requests open first. Needs review leads even though other
            // pull requests are newer; issues wait in their own tab.
            assert_eq!(
                view.ordered_inbox_items()
                    .iter()
                    .map(|item| item.url.as_str())
                    .collect::<Vec<_>>(),
                [review.url.as_str(), mine.url.as_str(), other.url.as_str()]
            );
            assert_eq!(
                view.work_inbox.selected.as_deref(),
                Some(review.url.as_str())
            );

            assert!(view.handle_inbox_key(&key("2"), window, cx));
            assert!(view.work_inbox.detail_tab == DetailTab::Code);
            assert!(view.handle_inbox_key(&key("j"), window, cx));
            assert_eq!(view.work_inbox.selected.as_deref(), Some(mine.url.as_str()));
            // Pull request to pull request keeps the tab.
            assert!(view.work_inbox.detail_tab == DetailTab::Code);
            assert!(view.handle_inbox_key(&key("j"), window, cx));
            assert!(view.handle_inbox_key(&key("j"), window, cx));
            assert_eq!(
                view.work_inbox.selected.as_deref(),
                Some(other.url.as_str())
            );
            assert!(view.handle_inbox_key(&key("3"), window, cx));
            assert!(view.work_inbox.detail_tab == DetailTab::Commits);
            assert!(view.handle_inbox_key(&key("k"), window, cx));
            assert!(view.handle_inbox_key(&key("k"), window, cx));
            assert_eq!(
                view.work_inbox.selected.as_deref(),
                Some(review.url.as_str())
            );

            // Issues have their own tab, with only a conversation.
            assert!(view.handle_inbox_key(&key("i"), window, cx));
            assert!(view.settings.inbox.tab == RepositoryTab::Issues);
            assert_eq!(
                view.work_inbox.selected.as_deref(),
                Some(issue.url.as_str())
            );
            assert!(view.work_inbox.detail_tab == DetailTab::Summary);
            // Coming back returns to the pull request left there.
            assert!(view.handle_inbox_key(&key("p"), window, cx));
            assert_eq!(
                view.work_inbox.selected.as_deref(),
                Some(review.url.as_str())
            );

            view.work_inbox
                .collapsed_groups
                .insert(WorkRelation::ReviewRequested);
            view.sync_inbox_selection(cx);
            assert_eq!(view.work_inbox.selected.as_deref(), Some(mine.url.as_str()));
            assert_eq!(view.inbox_card_child_index(&other.url), Some(4));

            assert!(view.handle_inbox_key(&key("shift-/"), window, cx));
            assert!(view.work_inbox.menu == Some(InboxMenu::Shortcuts));
            assert!(view.handle_inbox_key(&key("escape"), window, cx));
            assert!(view.work_inbox.menu.is_none());
            // Typed text never triggers shortcuts.
            view.work_inbox.search_editing = true;
            assert!(view.handle_inbox_key(&key("j"), window, cx));
            assert_eq!(view.work_inbox.filter.query, "j");
            view.work_inbox.search_editing = false;
            assert!(!view.handle_inbox_key(&key("cmd-j"), window, cx));

            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn viewed_marks_and_threads_reach_the_pull_request_review(cx: &mut gpui::TestAppContext) {
    use crate::domain::work_items::{FileViewed, WorkThread, pull_fixture};
    let (root, _, _, window) = open_recording_workspace(cx, "inbox-viewed");
    let item = pull_fixture();
    window
        .update(cx, |view, window, cx| {
            view.work_inbox.github.items.push(item.clone());
            view.select_section(WorkspaceSection::Inbox, window, cx);
            view.select_work_item(&item.url, cx);
            let state = view.work_inbox.details.entry(item.url.clone()).or_default();
            state.summary.data = Some(WorkDetail {
                head_oid: "b".repeat(40),
                viewer_review_oid: "c".repeat(40),
                viewed: [("src/lib.rs".to_owned(), FileViewed::Viewed)].into(),
                threads: vec![
                    WorkThread {
                        id: "T1".into(),
                        path: "src/lib.rs".into(),
                        line: Some(3),
                        ..Default::default()
                    },
                    WorkThread {
                        id: "T2".into(),
                        path: "src/lib.rs".into(),
                        line: None,
                        outdated: true,
                        ..Default::default()
                    },
                ],
                ..Default::default()
            });
            state.diff.data = Some(WorkDiff {
                files: vec![crate::domain::work_items::WorkDiffFile {
                    path: "src/lib.rs".into(),
                    patch: "@@ -1,2 +1,3 @@\n a\n+b\n c".into(),
                    ..Default::default()
                }],
                truncated: false,
            });
            view.sync_inbox_review(&item.url, cx);
            assert_eq!(view.inbox_since_base(&item.url), Some("c".repeat(40)));
            let review = view.work_inbox.details[&item.url].review.clone().unwrap();
            let (viewed, threads) = {
                let review = review.read(cx);
                (review.viewed_marks(), review.remote_thread_ids())
            };
            assert_eq!(viewed.get("src/lib.rs"), Some(&FileViewed::Viewed));
            assert_eq!(threads, ["T1"], "outdated threads stay in the summary");
            // Unchecking shows at once, before GitHub answers.
            view.work_inbox
                .details
                .get_mut(&item.url)
                .unwrap()
                .viewed_pending
                .insert("src/lib.rs".into(), false);
            view.sync_inbox_annotations(&item.url, cx);
            assert_eq!(
                review.read(cx).viewed_marks().get("src/lib.rs"),
                Some(&FileViewed::Unviewed)
            );
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn popovers_keep_running_work_and_reviews_keep_their_text(cx: &mut gpui::TestAppContext) {
    use crate::domain::work_items::pull_fixture;
    let (root, _, _, window) = open_recording_workspace(cx, "inbox-popovers");
    window
        .update(cx, |view, window, cx| {
            let first = pull_fixture();
            let mut second = pull_fixture();
            second.url = "https://github.com/demo/app/pull/8".into();
            view.work_inbox.github.items = vec![first.clone(), second.clone()];
            view.select_section(WorkspaceSection::Inbox, window, cx);
            view.select_work_item(&first.url, cx);

            // A confirmation opens beside its control and closes like a menu.
            view.confirm_pr_action_at(&first.url, PrAction::Close, (40.0, 60.0), cx);
            assert!(view.work_inbox.menu == Some(InboxMenu::Confirm));
            assert_eq!(view.work_inbox.menu_position, (40.0, 60.0));
            assert!(view.handle_inbox_key(&key("escape"), window, cx));
            assert!(view.work_inbox.confirmation.is_none());
            // While the action runs, it stays to show the outcome.
            view.confirm_pr_action_at(&first.url, PrAction::Close, (40.0, 60.0), cx);
            view.work_inbox
                .details
                .get_mut(&first.url)
                .unwrap()
                .mutation_busy = true;
            assert!(!view.dismiss_inbox_menu());
            assert!(view.work_inbox.menu == Some(InboxMenu::Confirm));
            assert!(view.work_inbox.confirmation.is_some());
            let state = view.work_inbox.details.get_mut(&first.url).unwrap();
            state.mutation_busy = false;

            // Closing the review keeps what was written and stops typing into it.
            view.work_inbox.menu = Some(InboxMenu::Review);
            let draft = &mut view
                .work_inbox
                .details
                .get_mut(&first.url)
                .unwrap()
                .review_draft;
            draft.open = true;
            draft.editing = true;
            draft.body = "Looks good".into();
            assert!(view.dismiss_inbox_menu());
            let draft = &view.work_inbox.details[&first.url].review_draft;
            assert!(!draft.open && !draft.editing);
            assert_eq!(draft.body, "Looks good");

            // Moving to another task closes it the same way.
            view.work_inbox.menu = Some(InboxMenu::Review);
            let draft = &mut view
                .work_inbox
                .details
                .get_mut(&first.url)
                .unwrap()
                .review_draft;
            draft.open = true;
            draft.editing = true;
            view.select_work_item(&second.url, cx);
            assert!(view.work_inbox.menu.is_none());
            assert!(!view.work_inbox.details[&first.url].review_draft.editing);
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn history_links_commits_to_pull_requests_and_keeps_the_list_task(cx: &mut gpui::TestAppContext) {
    use crate::domain::work_items::pull_fixture;
    use crate::ports::git::{GitCommit, GitHistory};
    let (root, snapshot, _, window) = open_recording_workspace(cx, "inbox-history");
    let project = snapshot.selected_project_id.unwrap();
    window
        .update(cx, |view, window, cx| {
            let mut open = pull_fixture();
            open.project_id = Some(project);
            let mut merged = pull_fixture();
            merged.url = "https://github.com/demo/app/pull/5".into();
            merged.reference = "#5".into();
            merged.project_id = Some(project);
            merged.status = WorkStatus::Merged;
            merged.pull.as_mut().unwrap().head_oid = "f".repeat(40);
            view.work_inbox.github.items = vec![open.clone(), merged.clone()];
            view.select_section(WorkspaceSection::Inbox, window, cx);
            view.select_work_item(&open.url, cx);

            let commit = |sha: char, subject: &str| GitCommit {
                sha: sha.to_string().repeat(40),
                short_sha: sha.to_string().repeat(7),
                subject: subject.into(),
                author: "Ana".into(),
                date: "2026-10-21".into(),
                parents: Vec::new(),
                refs: Vec::new(),
            };
            let head = commit('a', "Add search");
            let squash = commit('b', "Old feature (#5)");
            let unknown = commit('c', "Unrelated (#99)");
            let plain = commit('d', "Plain");
            view.work_inbox.history.log.data = Some(HistoryLog {
                root: root.clone(),
                current_only: false,
                history: GitHistory {
                    branch: "main".into(),
                    head: head.sha.clone(),
                    total: 4,
                    commits: vec![head.clone(), squash.clone(), unknown.clone(), plain.clone()],
                    truncated: false,
                },
                graph: Vec::new(),
            });

            assert!(view.handle_inbox_key(&key("h"), window, cx));
            assert!(view.settings.inbox.tab == RepositoryTab::History);
            // History has no list: the pull request stays selected for later.
            view.sync_inbox_selection(cx);
            assert_eq!(view.work_inbox.selected.as_deref(), Some(open.url.as_str()));

            // A branch head with an open pull request, a squash merge, a merge
            // the feed does not have, and a commit without one.
            let pull = view.commit_pull(&head).unwrap();
            assert!(pull.number == 7 && !pull.merged && pull.item.is_some());
            let pull = view.commit_pull(&squash).unwrap();
            assert!(pull.number == 5 && pull.merged);
            assert_eq!(
                pull.item.map(|item| item.url.as_str()),
                Some(merged.url.as_str())
            );
            let pull = view.commit_pull(&unknown).unwrap();
            assert!(pull.number == 99 && pull.item.is_none());
            assert!(view.commit_pull(&plain).is_none());

            // J walks the graph and Escape closes the open commit.
            assert!(view.handle_inbox_key(&key("j"), window, cx));
            assert_eq!(
                view.work_inbox.history.selected.as_deref(),
                Some(head.sha.as_str())
            );
            assert!(view.handle_inbox_key(&key("j"), window, cx));
            assert_eq!(
                view.work_inbox.history.selected.as_deref(),
                Some(squash.sha.as_str())
            );
            assert!(view.handle_inbox_key(&key("escape"), window, cx));
            assert!(view.work_inbox.history.selected.is_none());

            // A commit's pull request opens in its tab, even past the filters.
            view.open_commit_pull(5, cx);
            assert!(view.settings.inbox.tab == RepositoryTab::PullRequests);
            assert_eq!(
                view.work_inbox.selected.as_deref(),
                Some(merged.url.as_str())
            );
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
