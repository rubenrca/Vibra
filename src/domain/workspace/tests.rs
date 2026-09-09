use std::collections::HashSet;
use std::path::Path;

use uuid::Uuid;

use super::*;

#[test]
fn agent_task_titles_persist_and_survive_terminal_updates_without_renaming_manual_workspaces() {
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/tmp/task-one"));
    let first = snapshot.selected_session().unwrap().id;
    let project = snapshot.selected_project_id.unwrap();
    let workspace = snapshot.selected_workspace().unwrap().id;
    snapshot.rename_workspace(project, workspace, "Mi nombre");
    snapshot.create_workspace(Path::new("/tmp/task-two"));
    assert!(snapshot.update_agent_task_title(first, "Corregir login"));
    assert!(!snapshot.update_agent_task_title(first, "Corregir login"));
    assert!(!snapshot.update_agent_task_title(first, " "));
    assert!(!snapshot.update_agent_task_title(Uuid::new_v4(), "Otra tarea"));
    assert!(
        snapshot
            .selected_session()
            .unwrap()
            .agent_task_title
            .is_none()
    );
    snapshot.update_session_title(first, "zsh");
    snapshot.update_session_working_directory(first, Path::new("/tmp/new-dir"));
    let json = serde_json::to_string(&snapshot).unwrap();
    let restored: WorkspaceSnapshot = serde_json::from_str(&json).unwrap();
    let session = restored
        .terminal_sessions()
        .into_iter()
        .find(|s| s.id == first)
        .unwrap();
    assert_eq!(session.agent_task_title.as_deref(), Some("Corregir login"));
    let entry = restored
        .workspace_entries()
        .into_iter()
        .find(|e| e.workspace_id == workspace)
        .unwrap();
    assert!(entry.title_is_manual);
    assert_eq!(entry.workspace_name, "Mi nombre");
    let old = serde_json::json!({"id": first, "title": "Terminal", "workingDirectory": "/tmp"});
    assert!(
        serde_json::from_value::<SessionSnapshot>(old)
            .unwrap()
            .agent_task_title
            .is_none()
    );
}

fn uuid(value: &str) -> Uuid {
    Uuid::parse_str(value).unwrap()
}

#[test]
fn terminal_layout_matches_swift_codable_shape() {
    let layout = PaneLayoutSnapshot::terminal(uuid("AAB81C01-C781-4381-90EF-44F986F9DC74"));
    let encoded = serde_json::to_value(&layout).unwrap();

    assert_eq!(
        encoded,
        serde_json::json!({
            "terminal": { "_0": "aab81c01-c781-4381-90ef-44f986f9dc74" }
        })
    );
    assert_eq!(
        serde_json::from_value::<PaneLayoutSnapshot>(encoded).unwrap(),
        layout
    );
}

#[test]
fn split_layout_collapses_after_removing_a_terminal() {
    let first_id = Uuid::new_v4();
    let second_id = Uuid::new_v4();
    let layout = PaneLayoutSnapshot::joining(
        vec![
            PaneLayoutSnapshot::terminal(first_id),
            PaneLayoutSnapshot::terminal(second_id),
        ],
        WorkspaceSplitAxis::Horizontal,
    );

    assert_eq!(
        layout.removing_terminal(first_id),
        Some(PaneLayoutSnapshot::terminal(second_id))
    );
}

#[test]
fn legacy_split_layouts_receive_an_equal_ratio() {
    let first_id = Uuid::new_v4();
    let second_id = Uuid::new_v4();
    let encoded = serde_json::json!({
        "split": {
            "axis": "horizontal",
            "first": { "terminal": { "_0": first_id } },
            "second": { "terminal": { "_0": second_id } }
        }
    });

    let layout: PaneLayoutSnapshot = serde_json::from_value(encoded).unwrap();

    assert!(matches!(
        layout,
        PaneLayoutSnapshot::Split {
            ratio: DEFAULT_PANE_SPLIT_RATIO,
            ..
        }
    ));
}

#[test]
fn split_without_focus_keeps_the_caller_selected() {
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/tmp/vibra-nofocus"));
    let first_id = snapshot.selected_session().unwrap().id;
    let sibling = snapshot
        .split_selected_terminal_with_focus(PaneSplitDirection::Right, false)
        .unwrap();
    assert_ne!(first_id, sibling);
    assert_eq!(snapshot.selected_session().unwrap().id, first_id);
    assert_eq!(
        snapshot.selected_tab().unwrap().layout.terminal_ids(),
        vec![first_id, sibling]
    );
}

#[test]
fn create_terminal_tab_inherits_the_selected_session_working_directory() {
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/tmp/vibra-tab-root"));
    let session_id = snapshot.selected_session().unwrap().id;
    assert!(
        snapshot
            .update_session_working_directory(session_id, Path::new("/tmp/vibra-tab-root/nested"))
    );

    let (_, created_id) = snapshot
        .create_terminal_tab_with_options(true, None)
        .unwrap();
    let created = snapshot
        .terminal_sessions()
        .into_iter()
        .find(|session| session.id == created_id)
        .unwrap();
    assert_eq!(created.working_directory, "/tmp/vibra-tab-root/nested");
}

#[test]
fn create_workspace_opens_at_the_requested_directory() {
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/tmp/vibra-ws-root"));
    snapshot.create_workspace(Path::new("/tmp/vibra-ws-root/nested"));

    let entries = snapshot.workspace_entries();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1].working_directory, "/tmp/vibra-ws-root/nested");
    assert_eq!(
        snapshot.selected_session().unwrap().working_directory,
        "/tmp/vibra-ws-root/nested"
    );
}

#[test]
fn create_workspace_inherits_the_selected_workspaces_sidebar_space() {
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/tmp/vibra-grouped-workspace"));
    let first = snapshot.selected_workspace().unwrap().id;
    let space_id = snapshot.create_sidebar_space(first, "Grupo").unwrap();

    snapshot.create_workspace(Path::new("/tmp/vibra-grouped-workspace/nested"));
    let created = snapshot.selected_workspace().unwrap().id;

    assert!(snapshot.sidebar_items.iter().any(|item| matches!(
        item,
        SidebarItemSnapshot::Space {
            id,
            collapsed: false,
            workspace_ids,
            ..
        } if *id == space_id && workspace_ids == &vec![first, created]
    )));
}

#[test]
fn create_terminal_tab_without_focus_keeps_previous_tab() {
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/tmp/vibra-tab"));
    let original_tab = snapshot.selected_tab().unwrap().id;
    let (tab_id, session_id) = snapshot
        .create_terminal_tab_with_options(false, None)
        .unwrap();
    assert_ne!(tab_id, original_tab);
    assert_eq!(snapshot.selected_tab().unwrap().id, original_tab);
    let created = snapshot
        .selected_workspace()
        .unwrap()
        .tabs
        .iter()
        .find(|tab| tab.id == tab_id)
        .unwrap();
    assert_eq!(created.selected_session_id, Some(session_id));
}

#[test]
fn pane_operations_preserve_geometry_selection_and_zoom() {
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/tmp/vibra-panes"));
    let first_id = snapshot.selected_session().unwrap().id;
    let right_id = snapshot
        .split_selected_terminal(PaneSplitDirection::Right)
        .unwrap();

    assert_eq!(
        snapshot.selected_tab().unwrap().layout.terminal_ids(),
        vec![first_id, right_id]
    );
    assert!(snapshot.focus_terminal(PaneFocusDirection::Left));
    assert_eq!(snapshot.selected_session().unwrap().id, first_id);
    assert!(snapshot.focus_terminal(PaneFocusDirection::Right));
    assert_eq!(snapshot.selected_session().unwrap().id, right_id);

    let down_id = snapshot
        .split_selected_terminal(PaneSplitDirection::Down)
        .unwrap();
    assert!(snapshot.focus_terminal(PaneFocusDirection::Up));
    assert_eq!(snapshot.selected_session().unwrap().id, right_id);
    assert!(snapshot.focus_terminal(PaneFocusDirection::Down));
    assert_eq!(snapshot.selected_session().unwrap().id, down_id);
    assert!(snapshot.resize_selected_pane(PaneResizeDirection::Down));
    assert!(snapshot.equalize_selected_panes());

    assert!(snapshot.toggle_selected_pane_zoom());
    assert_eq!(
        snapshot.selected_tab().unwrap().zoomed_session_id,
        Some(down_id)
    );
    assert!(snapshot.toggle_selected_pane_zoom());
    assert_eq!(snapshot.selected_tab().unwrap().zoomed_session_id, None);
}

#[test]
fn split_ratios_are_addressed_by_tree_path_and_clamped() {
    let first_id = Uuid::new_v4();
    let second_id = Uuid::new_v4();
    let third_id = Uuid::new_v4();
    let mut layout = PaneLayoutSnapshot::joining(
        vec![
            PaneLayoutSnapshot::terminal(first_id),
            PaneLayoutSnapshot::terminal(second_id),
        ],
        WorkspaceSplitAxis::Horizontal,
    );
    assert!(layout.split_terminal(second_id, third_id, WorkspaceSplitAxis::Vertical, false,));

    assert!(layout.set_split_ratio(&[PaneBranch::Second], u16::MAX));

    let PaneLayoutSnapshot::Split { second, .. } = layout else {
        panic!("expected root split")
    };
    assert!(matches!(
        *second,
        PaneLayoutSnapshot::Split {
            ratio: MAX_PANE_SPLIT_RATIO,
            ..
        }
    ));
}

#[test]
fn tabs_can_be_reordered_and_addressed_by_number() {
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/tmp/vibra-tab-order"));
    let first = snapshot.selected_tab().unwrap().id;
    let (second, _) = snapshot
        .create_terminal_tab_with_options(true, None)
        .unwrap();
    let (third, _) = snapshot
        .create_terminal_tab_with_options(true, None)
        .unwrap();
    assert_eq!(
        snapshot
            .selected_workspace()
            .unwrap()
            .tabs
            .iter()
            .map(|tab| tab.id)
            .collect::<Vec<_>>(),
        vec![first, second, third]
    );

    assert!(snapshot.move_tab(third, Some(first)));
    assert_eq!(
        snapshot
            .selected_workspace()
            .unwrap()
            .tabs
            .iter()
            .map(|tab| tab.id)
            .collect::<Vec<_>>(),
        vec![third, first, second]
    );
    assert_eq!(snapshot.selected_tab().unwrap().id, third);
    assert!(!snapshot.move_tab(third, Some(first)));
    assert!(snapshot.move_tab(third, None));
    assert_eq!(
        snapshot
            .selected_workspace()
            .unwrap()
            .tabs
            .iter()
            .map(|tab| tab.id)
            .collect::<Vec<_>>(),
        vec![first, second, third]
    );

    assert!(snapshot.select_tab_number(1));
    assert_eq!(snapshot.selected_tab().unwrap().id, first);
    assert!(!snapshot.select_tab_number(1));
    assert!(snapshot.select_tab_number(8));
    assert_eq!(snapshot.selected_tab().unwrap().id, third);
    assert!(snapshot.select_tab(first));
    assert!(snapshot.select_tab_number(9));
    assert_eq!(snapshot.selected_tab().unwrap().id, third);
}

#[test]
fn sidebar_workspaces_can_be_reordered_across_projects() {
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/tmp/vibra-sidebar-a"));
    let first = snapshot.selected_workspace().unwrap().id;
    snapshot.create_workspace(Path::new("/tmp/vibra-sidebar-b"));
    let second = snapshot.selected_workspace().unwrap().id;
    snapshot.create_workspace(Path::new("/tmp/vibra-sidebar-a"));
    let third = snapshot.selected_workspace().unwrap().id;

    let entry_ids = |snapshot: &WorkspaceSnapshot| {
        snapshot
            .workspace_entries()
            .into_iter()
            .map(|entry| entry.workspace_id)
            .collect::<Vec<_>>()
    };
    assert_eq!(entry_ids(&snapshot), vec![first, second, third]);

    assert!(snapshot.move_workspace(third, Some(first)));
    assert_eq!(entry_ids(&snapshot), vec![third, first, second]);
    assert!(!snapshot.move_workspace(third, Some(first)));
    assert!(snapshot.move_workspace(third, None));
    assert_eq!(entry_ids(&snapshot), vec![first, second, third]);

    let second_project = snapshot
        .workspace_entries()
        .into_iter()
        .find(|entry| entry.workspace_id == second)
        .unwrap()
        .project_id;
    assert!(snapshot.close_workspace(second_project, second));
    assert_eq!(entry_ids(&snapshot), vec![first, third]);
    assert_eq!(snapshot.workspace_order, vec![first, third]);
}

#[test]
fn sidebar_spaces_are_created_collapsed_persisted_and_removed() {
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/tmp/vibra-sidebar-a"));
    let first = snapshot.selected_workspace().unwrap().id;
    snapshot.create_workspace(Path::new("/tmp/vibra-sidebar-b"));
    let second = snapshot.selected_workspace().unwrap().id;

    let space_id = snapshot.create_sidebar_space(first, "Vibra").unwrap();
    assert!(matches!(
        &snapshot.sidebar_entries()[0],
        SidebarEntry::Space {
            id,
            name,
            collapsed: false,
            workspace_count: 1,
        } if *id == space_id && name == "Vibra"
    ));
    assert!(matches!(
        &snapshot.sidebar_entries()[1],
        SidebarEntry::Workspace { entry, space_id: Some(id) }
            if entry.workspace_id == first && *id == space_id
    ));
    assert!(matches!(
        &snapshot.sidebar_entries()[2],
        SidebarEntry::Workspace { entry, space_id: None }
            if entry.workspace_id == second
    ));

    assert!(snapshot.toggle_sidebar_space(space_id));
    assert_eq!(snapshot.sidebar_entries().len(), 2);
    assert!(snapshot.rename_sidebar_space(space_id, "Trabajo"));

    let json = serde_json::to_string(&snapshot).unwrap();
    let mut restored: WorkspaceSnapshot = serde_json::from_str(&json).unwrap();
    restored.normalize();
    assert!(restored.sidebar_items.iter().any(|item| matches!(
        item,
        SidebarItemSnapshot::Space { id, name, collapsed: true, workspace_ids }
            if *id == space_id && name == "Trabajo" && workspace_ids == &vec![first]
    )));
    assert_eq!(restored.workspace_order, vec![first, second]);

    assert!(restored.remove_sidebar_space(space_id));
    assert!(!restored.remove_sidebar_space(space_id));
    assert_eq!(
        restored
            .sidebar_entries()
            .into_iter()
            .filter_map(|entry| match entry {
                SidebarEntry::Workspace { entry, .. } => Some(entry.workspace_id),
                SidebarEntry::Space { .. } => None,
            })
            .collect::<Vec<_>>(),
        vec![first, second]
    );
}

#[test]
fn legacy_workspace_order_migrates_to_sidebar_items() {
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/tmp/vibra-sidebar-a"));
    let first = snapshot.selected_workspace().unwrap().id;
    snapshot.create_workspace(Path::new("/tmp/vibra-sidebar-b"));
    let second = snapshot.selected_workspace().unwrap().id;
    snapshot.workspace_order = vec![second, first];
    snapshot.sidebar_items.clear();

    snapshot.normalize();

    assert_eq!(
        snapshot.sidebar_items,
        vec![
            SidebarItemSnapshot::Workspace {
                workspace_id: second
            },
            SidebarItemSnapshot::Workspace {
                workspace_id: first
            },
        ]
    );
    assert_eq!(snapshot.schema_version, CURRENT_WORKSPACE_SCHEMA_VERSION);
}

#[test]
fn empty_sidebar_spaces_accept_dragged_workspaces() {
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/tmp/vibra-sidebar-a"));
    let first = snapshot.selected_workspace().unwrap().id;
    snapshot.create_workspace(Path::new("/tmp/vibra-sidebar-b"));
    let second = snapshot.selected_workspace().unwrap().id;

    let space_id = snapshot.create_empty_sidebar_space("Clientes");
    assert!(matches!(
        snapshot.sidebar_entries().last(),
        Some(SidebarEntry::Space {
            id,
            workspace_count: 0,
            ..
        }) if *id == space_id
    ));

    assert!(snapshot.toggle_sidebar_space(space_id));
    assert!(snapshot.move_workspace_to_space(second, space_id));
    assert_eq!(snapshot.workspace_order, vec![first, second]);
    assert!(matches!(
        &snapshot.sidebar_items[1],
        SidebarItemSnapshot::Space {
            collapsed: false,
            ..
        }
    ));
    assert!(matches!(
        snapshot.sidebar_entries().last(),
        Some(SidebarEntry::Workspace {
            entry,
            space_id: Some(id),
        }) if entry.workspace_id == second && *id == space_id
    ));
    assert!(!snapshot.move_workspace_to_space(second, space_id));

    // Dropping an adjacent ungrouped row before a grouped row must still
    // move it into the group, even though the flat order does not change.
    assert!(snapshot.move_workspace(first, Some(second)));
    assert_eq!(snapshot.workspace_order, vec![first, second]);
    assert!(matches!(
        &snapshot.sidebar_items[0],
        SidebarItemSnapshot::Space { workspace_ids, .. }
            if workspace_ids == &vec![first, second]
    ));
    assert!(!snapshot.move_workspace(first, Some(second)));

    assert!(snapshot.move_workspace_relative(second, first, false));
    assert_eq!(snapshot.workspace_order, vec![second, first]);
    assert!(snapshot.move_workspace_relative(second, first, true));
    assert_eq!(snapshot.workspace_order, vec![first, second]);
    assert!(!snapshot.move_workspace_relative(second, first, true));
}

#[test]
fn swapping_panes_exchanges_terminals_and_keeps_split_geometry() {
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/tmp/vibra-pane-swap"));
    let first = snapshot.selected_session().unwrap().id;
    let second = snapshot
        .split_selected_terminal(PaneSplitDirection::Right)
        .unwrap();
    assert!(snapshot.set_selected_split_ratio(&[], 7_000));
    assert_eq!(
        snapshot.selected_tab().unwrap().layout.terminal_ids(),
        vec![first, second]
    );

    assert!(snapshot.swap_tab_terminals(second, first));
    assert_eq!(snapshot.selected_session().unwrap().id, second);
    match &snapshot.selected_tab().unwrap().layout {
        PaneLayoutSnapshot::Split {
            ratio,
            first: left,
            second: right,
            ..
        } => {
            assert_eq!(*ratio, 7_000);
            assert_eq!(left.terminal_ids(), vec![second]);
            assert_eq!(right.terminal_ids(), vec![first]);
        }
        PaneLayoutSnapshot::Terminal { .. } => panic!("expected a split"),
    }
    assert!(!snapshot.swap_tab_terminals(second, second));
}

#[test]
fn normalization_repairs_stale_selection() {
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/tmp/vibra-gpui-test"));
    snapshot.selected_project_id = Some(Uuid::new_v4());

    snapshot.normalize();

    assert_eq!(snapshot.projects.len(), 1);
    assert_eq!(snapshot.selected_project_id, Some(snapshot.projects[0].id));
    assert!(snapshot.selected_session().is_some());
}

#[test]
fn legacy_sessions_migrate_without_data_loss() {
    let session = SessionSnapshot::new("/tmp/vibra-legacy".into());
    let session_id = session.id;
    let mut snapshot = WorkspaceSnapshot {
        schema_version: 0,
        projects: vec![ProjectSnapshot {
            id: Uuid::new_v4(),
            name: "Legacy".into(),
            root_path: "/tmp/vibra-legacy".into(),
            sessions: vec![session],
            selected_session_id: Some(session_id),
            visible_session_ids: Some(vec![session_id]),
            split_axis: None,
            tabs: None,
            selected_tab_id: None,
            workspaces: None,
            selected_workspace_id: None,
        }],
        selected_project_id: None,
        workspace_order: Vec::new(),
        sidebar_items: Vec::new(),
    };

    snapshot.normalize();

    assert!(
        snapshot.projects[0].sessions.is_empty(),
        "schema 3 must not dual-write legacy session copies"
    );
    assert!(snapshot.projects[0].tabs.is_none());
    assert_eq!(snapshot.selected_session().unwrap().id, session_id);
}

#[test]
fn workspace_entries_surface_the_selected_session_working_directory() {
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/tmp/vibra-sidebar"));
    let session_id = snapshot.selected_session().unwrap().id;
    assert!(
        snapshot
            .update_session_working_directory(session_id, Path::new("/tmp/vibra-sidebar/nested"))
    );

    let entries = snapshot.workspace_entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].working_directory, "/tmp/vibra-sidebar/nested");
    // Automatic titles follow the primary session basename after `cd`.
    assert_eq!(entries[0].workspace_name, "nested");
    assert_eq!(
        snapshot.selected_workspace().unwrap().title_source,
        Some(WorkspaceTitleSource::Automatic)
    );
}

#[test]
fn manual_workspace_title_survives_working_directory_changes() {
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/tmp/vibra-manual-title"));
    let project_id = snapshot.selected_project_id.unwrap();
    let workspace_id = snapshot.selected_workspace().unwrap().id;
    assert!(snapshot.rename_workspace(project_id, workspace_id, "Mi sesión"));
    let session_id = snapshot.selected_session().unwrap().id;

    assert!(
        snapshot.update_session_working_directory(
            session_id,
            Path::new("/tmp/vibra-manual-title/deep")
        )
    );
    assert_eq!(snapshot.selected_workspace().unwrap().name, "Mi sesión");
    assert_eq!(
        snapshot.selected_workspace().unwrap().title_source,
        Some(WorkspaceTitleSource::Manual)
    );
}

#[test]
fn terminal_title_updates_the_canonical_and_legacy_views() {
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/tmp/vibra-title"));
    let session_id = snapshot.selected_session().unwrap().id;

    assert!(snapshot.update_session_title(session_id, "zsh — tests"));
    assert_eq!(snapshot.selected_session().unwrap().title, "zsh — tests");
    assert!(
        snapshot.projects[0].sessions.is_empty(),
        "title updates must not rewrite the Swift-era sessions vector"
    );
    assert!(!snapshot.update_session_title(session_id, "zsh — tests"));
}

#[test]
fn global_session_operations_find_unselected_projects_workspaces_and_tabs() {
    let mut snapshot = WorkspaceSnapshot::default();
    let root = Path::new("/tmp/vibra-global");
    snapshot.create_workspace(root);
    snapshot.create_workspace(root);
    let (tab_id, session_id) = snapshot
        .create_terminal_tab_with_options(false, None)
        .unwrap();
    let project_id = snapshot.selected_project_id;
    let workspace_id = snapshot.selected_workspace().unwrap().id;
    snapshot.create_workspace(Path::new("/tmp/vibra-other"));
    let selection = snapshot.selected_session().unwrap().id;

    assert!(snapshot.update_session_title(session_id, "  background  "));
    assert!(
        snapshot
            .update_session_working_directory(session_id, Path::new("/tmp/vibra-global/nested"))
    );
    assert_eq!(snapshot.selected_session().unwrap().id, selection);

    assert!(snapshot.select_terminal_global(session_id));
    assert_eq!(snapshot.selected_project_id, project_id);
    assert_eq!(snapshot.selected_workspace().unwrap().id, workspace_id);
    assert_eq!(snapshot.selected_tab().unwrap().id, tab_id);
    let session = snapshot.selected_session().unwrap();
    assert_eq!(session.id, session_id);
    assert_eq!(session.title, "background");
    assert_eq!(session.working_directory, "/tmp/vibra-global/nested");
    // Selecting an already selected session still reports success.
    assert!(snapshot.select_terminal_global(session_id));
}

#[test]
fn unknown_session_operations_leave_the_snapshot_unchanged() {
    for mut snapshot in [WorkspaceSnapshot::default(), {
        let mut snapshot = WorkspaceSnapshot::default();
        snapshot.create_workspace(Path::new("/tmp/vibra-known"));
        snapshot
    }] {
        let before = snapshot.clone();
        let unknown = Uuid::new_v4();
        assert!(!snapshot.select_terminal_global(unknown));
        assert!(!snapshot.update_session_title(unknown, "unknown"));
        assert!(!snapshot.update_session_working_directory(unknown, Path::new("/tmp/unknown")));
        assert_eq!(snapshot, before);
    }
}

#[test]
fn background_session_directory_changes_preserve_the_workspace_title() {
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/tmp/vibra-primary"));
    let session_id = snapshot
        .split_selected_terminal_with_focus(PaneSplitDirection::Right, false)
        .unwrap();
    let name = snapshot.selected_workspace().unwrap().name.clone();
    let path = Path::new("/tmp/background");

    assert!(snapshot.update_session_working_directory(session_id, path));
    assert_eq!(snapshot.selected_workspace().unwrap().name, name);
    let before = snapshot.clone();
    assert!(!snapshot.update_session_working_directory(session_id, path));
    assert!(!snapshot.update_session_title(session_id, "  "));
    assert_eq!(snapshot, before);
}

#[test]
fn painted_session_ids_follow_tab_workspace_and_zoom() {
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/tmp/vibra-paint-a"));
    let first_tab = snapshot.selected_tab().unwrap().id;
    let first_session = snapshot.selected_session().unwrap().id;
    snapshot.create_terminal_tab_with_options(true, None);
    let second_tab = snapshot.selected_tab().unwrap().id;
    let second_session = snapshot.selected_session().unwrap().id;
    assert_ne!(first_session, second_session);

    assert_eq!(
        snapshot.painted_session_ids(),
        HashSet::from([second_session]),
        "new tab is selected and should be the only painted session"
    );

    assert!(snapshot.select_tab(first_tab));
    assert_eq!(
        snapshot.painted_session_ids(),
        HashSet::from([first_session])
    );
    assert!(snapshot.select_tab(second_tab));
    assert_eq!(
        snapshot.painted_session_ids(),
        HashSet::from([second_session])
    );

    snapshot.create_workspace(Path::new("/tmp/vibra-paint-b"));
    let other_workspace = snapshot.selected_workspace().unwrap().id;
    let other_session = snapshot.selected_session().unwrap().id;
    let other_project = snapshot.selected_project_id.unwrap();
    assert_eq!(
        snapshot.painted_session_ids(),
        HashSet::from([other_session])
    );
    let first_project = snapshot.projects[0].id;
    let first_workspace = snapshot.projects[0].workspaces.as_ref().unwrap()[0].id;
    assert!(snapshot.select_workspace(first_project, first_workspace));
    assert_eq!(
        snapshot.painted_session_ids(),
        HashSet::from([second_session]),
        "returning to the first workspace paints its selected tab"
    );
    assert!(snapshot.select_workspace(other_project, other_workspace));
    assert_eq!(
        snapshot.painted_session_ids(),
        HashSet::from([other_session])
    );

    snapshot.split_selected_terminal(PaneSplitDirection::Right);
    let split_ids = snapshot.painted_session_ids();
    assert_eq!(split_ids.len(), 2);
    assert!(snapshot.toggle_selected_pane_zoom());
    let zoomed = snapshot.selected_session().unwrap().id;
    assert_eq!(snapshot.painted_session_ids(), HashSet::from([zoomed]));
    assert!(snapshot.toggle_selected_pane_zoom());
    assert_eq!(snapshot.painted_session_ids(), split_ids);
}

#[test]
fn accidental_root_workspace_relocates_every_session() {
    let target = std::env::temp_dir().join(format!("VibraDev-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&target).unwrap();
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/"));
    snapshot.create_terminal_tab_with_options(true, None);

    assert!(snapshot.relocate_root(Path::new("/"), &target));

    let project = &snapshot.projects[0];
    assert_eq!(project.root_path, target.to_string_lossy());
    assert_eq!(project.name, target.file_name().unwrap().to_string_lossy());
    assert!(
        snapshot
            .terminal_sessions()
            .iter()
            .all(|session| { session.working_directory == target.to_string_lossy() })
    );
    assert!(!snapshot.relocate_root(Path::new("/"), &target));
    std::fs::remove_dir_all(target).unwrap();
}
