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

            view.select_work_source(WorkSource::Linear, cx);
            assert!(view.work_inbox.filter.project.is_none());
            view.select_work_source(WorkSource::GitHub, cx);
            assert_eq!(view.work_inbox.filter.project, Some(second_project));
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
            view.inbox_source_updated(WorkSource::GitHub, true, cx);
            assert!(view.work_inbox.selected.is_none());
            assert!(item.unread(&view.settings.inbox.seen));
            assert!(view.work_inbox.details.is_empty());
            view.workspace_section = WorkspaceSection::Inbox;
            view.work_inbox.activity = true;
            view.inbox_source_updated(WorkSource::GitHub, false, cx);
            assert!(view.work_inbox.selected.is_none());
            assert!(item.unread(&view.settings.inbox.seen));
            view.work_inbox.activity = false;
            view.inbox_source_updated(WorkSource::GitHub, false, cx);
            assert_eq!(view.work_inbox.selected.as_deref(), Some(item.url.as_str()));
            assert!(!item.unread(&view.settings.inbox.seen));
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn work_inbox_sources_fail_independently_and_ignore_stale_results() {
    let mut inbox = WorkInbox::default();
    inbox.github.apply(Ok(WorkItemsPage {
        items: vec![fixture()],
        connected: true,
        ..Default::default()
    }));
    inbox
        .linear
        .apply(Err(anyhow::anyhow!("Linear unavailable")));
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
