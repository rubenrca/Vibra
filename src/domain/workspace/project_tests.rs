use std::path::Path;

use uuid::Uuid;

use super::*;

fn session_ids(snapshot: &WorkspaceSnapshot) -> Vec<Uuid> {
    snapshot
        .workspace_entries()
        .iter()
        .map(|e| e.workspace_id)
        .collect()
}

fn round_trip(snapshot: &WorkspaceSnapshot) -> WorkspaceSnapshot {
    let mut restored: WorkspaceSnapshot =
        serde_json::from_str(&serde_json::to_string(snapshot).unwrap()).unwrap();
    restored.normalize();
    restored
}

#[test]
fn projects_persist_without_sessions_and_reopening_selects_the_existing_project() {
    let mut snapshot = WorkspaceSnapshot::default();
    let a = snapshot.add_project(Path::new("/projects/a"));
    let b = snapshot.add_project(Path::new("/projects/b"));
    assert!(snapshot.selected_session().is_none());
    assert_eq!(snapshot.add_project(Path::new("/projects/a")), a);
    assert_eq!(snapshot.projects.len(), 2);
    assert_eq!(snapshot.selected_project_id, Some(a));
    assert!(snapshot.rename_project(a, "Mi proyecto"));
    assert!(snapshot.toggle_project(b));
    assert!(snapshot.move_project(b, Some(a)));
    assert!(!snapshot.move_project(b, Some(a)));
    assert_eq!(snapshot.projects[0].id, b);
    assert_eq!(round_trip(&snapshot), snapshot);
    assert!(matches!(
        snapshot.sidebar_entries()[0],
        SidebarEntry::Project {
            id,
            workspace_count: 0,
            collapsed: true,
            ..
        } if id == b
    ));
    assert!(snapshot.move_project(b, None));
    assert_eq!(snapshot.projects[1].id, b);
}

#[test]
fn project_and_workspace_names_have_a_persistable_limit() {
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/projects/a"));
    let project_id = snapshot.selected_project_id.unwrap();
    let workspace_id = snapshot.selected_workspace().unwrap().id;
    let too_long = "é".repeat(MAX_NAME_CHARS + 1);

    assert!(!snapshot.rename_project(project_id, &too_long));
    assert!(!snapshot.rename_workspace(project_id, workspace_id, &too_long));
    assert!(snapshot.rename_project(project_id, &"é".repeat(MAX_NAME_CHARS)));
    assert!(snapshot.rename_workspace(project_id, workspace_id, &"é".repeat(MAX_NAME_CHARS)));
}

#[test]
fn sidebar_entries_follow_project_and_workspace_order_and_collapse() {
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/projects/a"));
    let a = snapshot.selected_project_id.unwrap();
    snapshot.create_workspace(Path::new("/projects/b"));
    let b = snapshot.selected_project_id.unwrap();
    snapshot.create_workspace(Path::new("/projects/a"));
    let workspace_entries = snapshot.workspace_entries();

    let sidebar = snapshot.sidebar_entries();
    assert_eq!(sidebar.len(), 5);
    assert!(matches!(sidebar[0], SidebarEntry::Project { id, .. } if id == a));
    assert!(matches!(sidebar[3], SidebarEntry::Project { id, .. } if id == b));
    let shown: Vec<_> = sidebar
        .into_iter()
        .filter_map(|entry| match entry {
            SidebarEntry::Workspace { entry } => Some(entry),
            SidebarEntry::Project { .. } => None,
        })
        .collect();
    assert_eq!(shown, workspace_entries);

    assert!(snapshot.toggle_project(a));
    let sidebar = snapshot.sidebar_entries();
    assert_eq!(sidebar.len(), 3);
    assert!(
        matches!(sidebar[0], SidebarEntry::Project { id, collapsed: true, workspace_count: 2, .. } if id == a)
    );
    assert!(matches!(sidebar[1], SidebarEntry::Project { id, .. } if id == b));
}

#[test]
fn cycling_workspaces_preserves_order_with_large_offsets() {
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/projects/a"));
    let first = snapshot.selected_workspace().unwrap().id;
    snapshot.create_workspace(Path::new("/projects/b"));
    let second = snapshot.selected_workspace().unwrap().id;
    snapshot.create_workspace(Path::new("/projects/a"));

    assert!(snapshot.cycle_workspace(isize::MAX));
    assert_eq!(snapshot.selected_workspace().unwrap().id, second);
    assert!(snapshot.cycle_workspace(isize::MIN));
    assert_eq!(snapshot.selected_workspace().unwrap().id, first);
}

#[test]
fn legacy_migration_preserves_containers_with_duplicate_ids() {
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/projects/a"));
    let first_project_id = snapshot.projects[0].id;
    let first_workspace_id = snapshot.projects[0].workspaces.as_ref().unwrap()[0].id;
    let first_tab_id = snapshot.projects[0].workspaces.as_ref().unwrap()[0].tabs[0].id;
    snapshot.create_workspace(Path::new("/projects/b"));
    let project = &mut snapshot.projects[1];
    project.id = first_project_id;
    project.selected_workspace_id = Some(first_workspace_id);
    let workspace = &mut project.workspaces.as_mut().unwrap()[0];
    workspace.id = first_workspace_id;
    workspace.selected_tab_id = Some(first_tab_id);
    workspace.tabs[0].id = first_tab_id;
    snapshot.schema_version = 6;

    snapshot.normalize();

    assert_eq!(snapshot.projects.len(), 2);
    assert_eq!(snapshot.workspace_entries().len(), 2);
    assert_eq!(snapshot.terminal_sessions().len(), 2);
    assert_ne!(snapshot.projects[0].id, snapshot.projects[1].id);
    // The old ID identified both projects; selection keeps the first match,
    // which is what reads before normalization could access.
    assert_eq!(snapshot.selected_project_id, Some(snapshot.projects[0].id));
    let first = &snapshot.projects[0].workspaces.as_ref().unwrap()[0];
    let second_project = &snapshot.projects[1];
    let second = &second_project.workspaces.as_ref().unwrap()[0];
    assert_ne!(first.id, second.id);
    assert_ne!(first.tabs[0].id, second.tabs[0].id);
    assert_eq!(second_project.selected_workspace_id, Some(second.id));
    assert_eq!(second.selected_tab_id, Some(second.tabs[0].id));
}

#[test]
fn normalization_preserves_all_workspaces_when_ids_repeat_within_a_project() {
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/projects/a"));
    snapshot.create_workspace(Path::new("/projects/a"));
    let workspaces = snapshot.projects[0].workspaces.as_mut().unwrap();
    let first_id = workspaces[0].id;
    workspaces[1].id = first_id;
    snapshot.projects[0].selected_workspace_id = Some(first_id);

    snapshot.normalize();

    let workspaces = snapshot.projects[0].workspaces.as_ref().unwrap();
    assert_eq!(workspaces.len(), 2);
    assert_ne!(workspaces[0].id, workspaces[1].id);
    assert_eq!(snapshot.selected_workspace().unwrap().id, workspaces[0].id);
    assert_eq!(snapshot.terminal_sessions().len(), 2);
}

#[test]
fn normalization_preserves_all_tabs_when_selected_tab_id_repeats() {
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/projects/a"));
    let first_tab_id = snapshot.selected_tab().unwrap().id;
    snapshot
        .create_terminal_tab_with_options(true, None)
        .unwrap();
    let workspace = &mut snapshot.projects[0].workspaces.as_mut().unwrap()[0];
    workspace.tabs[1].id = first_tab_id;
    workspace.selected_tab_id = Some(first_tab_id);

    snapshot.normalize();

    let workspace = snapshot.selected_workspace().unwrap();
    assert_eq!(workspace.tabs.len(), 2);
    assert_ne!(workspace.tabs[0].id, workspace.tabs[1].id);
    assert_eq!(snapshot.selected_tab().unwrap().id, workspace.tabs[0].id);
    assert_eq!(snapshot.terminal_sessions().len(), 2);
}

#[test]
fn closing_the_last_session_or_pane_keeps_the_project() {
    for close_pane in [false, true] {
        let mut snapshot = WorkspaceSnapshot::default();
        snapshot.create_workspace(Path::new("/projects/a"));
        let project_id = snapshot.selected_project_id.unwrap();
        let workspace_id = snapshot.selected_workspace().unwrap().id;
        if close_pane {
            assert!(snapshot.close_selected_terminal());
        } else {
            assert!(snapshot.close_workspace(project_id, workspace_id));
        }
        assert_eq!(snapshot.projects.len(), 1);
        assert_eq!(snapshot.selected_project_id, Some(project_id));
        assert!(snapshot.selected_workspace().is_none());
        assert!(snapshot.terminal_sessions().is_empty());
        assert_eq!(round_trip(&snapshot), snapshot);
        snapshot.create_workspace_in_project(project_id).unwrap();
        assert_eq!(
            snapshot.selected_session().unwrap().working_directory,
            "/projects/a"
        );
        assert!(snapshot.remove_project(project_id));
        assert!(snapshot.projects.is_empty());
        assert!(snapshot.selected_project_id.is_none());
        assert_eq!(round_trip(&snapshot), snapshot);
    }
}

#[test]
fn legacy_spaces_become_projects_without_losing_names_order_layout_or_selection() {
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/projects/a"));
    let first = snapshot.selected_workspace().unwrap().id;
    snapshot
        .split_selected_terminal(PaneSplitDirection::Down)
        .unwrap();
    let panes = snapshot.selected_workspace().unwrap().clone();
    snapshot.create_workspace(Path::new("/projects/a"));
    let second = snapshot.selected_workspace().unwrap().id;
    let space = Uuid::new_v4();
    snapshot.schema_version = 6;
    snapshot.sidebar_items = vec![SidebarItemSnapshot::Space {
        id: space,
        name: "Feature X".into(),
        collapsed: true,
        workspace_ids: vec![second, first],
    }];
    let migrated = round_trip(&snapshot);
    assert_eq!(migrated.projects.len(), 1);
    assert_eq!(migrated.selected_project_id, Some(space));
    assert_eq!(migrated.selected_workspace().unwrap().id, second);
    let project = &migrated.projects[0];
    assert_eq!(project.name, "Feature X");
    assert_eq!(project.root_path, "/projects/a");
    assert!(project.collapsed);
    assert_eq!(session_ids(&migrated), [second, first]);
    assert_eq!(project.workspaces.as_ref().unwrap()[1], panes);
    assert!(migrated.sidebar_items.is_empty());
    assert_eq!(migrated.schema_version, CURRENT_WORKSPACE_SCHEMA_VERSION);
    assert_eq!(round_trip(&migrated), migrated);
}

#[test]
fn repeated_legacy_space_ids_preserve_both_named_groups() {
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/projects/a"));
    let first = snapshot.selected_workspace().unwrap().id;
    snapshot.create_workspace(Path::new("/projects/a"));
    let second = snapshot.selected_workspace().unwrap().id;
    let duplicated_id = Uuid::new_v4();
    snapshot.schema_version = 6;
    snapshot.sidebar_items = vec![
        SidebarItemSnapshot::Space {
            id: duplicated_id,
            name: "Primero".into(),
            collapsed: true,
            workspace_ids: vec![first],
        },
        SidebarItemSnapshot::Space {
            id: duplicated_id,
            name: "Segundo".into(),
            collapsed: false,
            workspace_ids: vec![second],
        },
    ];

    snapshot.normalize();

    assert_eq!(snapshot.projects.len(), 2);
    assert_eq!(snapshot.projects[0].id, duplicated_id);
    assert_eq!(snapshot.projects[0].name, "Primero");
    assert!(snapshot.projects[0].collapsed);
    assert_eq!(
        snapshot.projects[0].workspaces.as_ref().unwrap()[0].id,
        first
    );
    assert_ne!(snapshot.projects[1].id, duplicated_id);
    assert_eq!(snapshot.projects[1].name, "Segundo");
    assert!(!snapshot.projects[1].collapsed);
    assert_eq!(
        snapshot.projects[1].workspaces.as_ref().unwrap()[0].id,
        second
    );
    assert_eq!(snapshot.selected_project_id, Some(snapshot.projects[1].id));
}

#[test]
fn mixed_and_empty_legacy_spaces_require_a_folder_without_changing_existing_cwds() {
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/projects/a"));
    let first = snapshot.selected_workspace().unwrap().id;
    snapshot.create_workspace(Path::new("/projects/b"));
    let second = snapshot.selected_workspace().unwrap().id;
    let sessions = snapshot.terminal_sessions();
    let mixed = Uuid::new_v4();
    let empty = Uuid::new_v4();
    snapshot.schema_version = 6;
    snapshot.sidebar_items = vec![
        SidebarItemSnapshot::Space {
            id: mixed,
            name: "Trabajo".into(),
            collapsed: false,
            workspace_ids: vec![first, second],
        },
        SidebarItemSnapshot::Space {
            id: empty,
            name: "Ideas".into(),
            collapsed: true,
            workspace_ids: vec![],
        },
    ];
    snapshot.normalize();
    assert_eq!(snapshot.projects.len(), 2);
    assert!(snapshot.projects.iter().all(|p| p.directory().is_none()));
    assert_eq!(snapshot.terminal_sessions(), sessions);
    assert!(snapshot.create_workspace_in_project(mixed).is_none());
    assert!(snapshot.set_project_directory(mixed, Path::new("/projects/chosen")));
    assert_eq!(snapshot.terminal_sessions(), sessions);
    snapshot.create_workspace_in_project(mixed).unwrap();
    assert_eq!(
        snapshot.selected_session().unwrap().working_directory,
        "/projects/chosen"
    );
    assert_eq!(round_trip(&snapshot), snapshot);
}

#[test]
fn legacy_flat_order_and_spacers_preserve_all_sessions_once() {
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/projects/a"));
    let first = snapshot.selected_workspace().unwrap().id;
    snapshot.create_workspace(Path::new("/projects/b"));
    let second = snapshot.selected_workspace().unwrap().id;
    let mut flat = snapshot.clone();
    flat.schema_version = 4;
    flat.workspace_order = vec![second, first, second, Uuid::new_v4()];
    flat.normalize();
    assert_eq!(session_ids(&flat), [second, first]);
    assert_eq!(round_trip(&flat), flat);

    let space = Uuid::new_v4();
    snapshot.schema_version = 5;
    snapshot.sidebar_items = vec![
        SidebarItemSnapshot::Spacer { id: space },
        SidebarItemSnapshot::Workspace {
            workspace_id: second,
        },
        SidebarItemSnapshot::Workspace {
            workspace_id: second,
        },
        SidebarItemSnapshot::Workspace {
            workspace_id: Uuid::new_v4(),
        },
    ];
    snapshot.normalize();
    assert_eq!(session_ids(&snapshot), [second, first]);
    assert_eq!(snapshot.projects[0].id, space);
    assert_eq!(snapshot.projects[0].name, "Espacio");
    assert_eq!(snapshot.selected_workspace().unwrap().id, second);
}
