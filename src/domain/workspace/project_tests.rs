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
    snapshot
        .projects
        .iter_mut()
        .find(|project| project.id == b)
        .unwrap()
        .collapsed = true;
    assert!(snapshot.move_project(b, Some(a)));
    assert!(!snapshot.move_project(b, Some(a)));
    assert_eq!(snapshot.projects[0].id, b);
    assert_eq!(round_trip(&snapshot), snapshot);
    assert!(
        matches!(snapshot.sidebar_entries()[0], SidebarEntry::Project { id, workspace_count: 0, .. } if id == a)
    );
    assert!(snapshot.move_project(b, None));
    assert_eq!(snapshot.projects[1].id, b);
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
fn moving_sessions_preserves_terminal_state_and_updates_active_project() {
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/projects/a"));
    let a = snapshot.selected_project_id.unwrap();
    let first = snapshot.selected_workspace().unwrap().id;
    snapshot
        .split_selected_terminal(PaneSplitDirection::Down)
        .unwrap();
    let panes = snapshot.selected_workspace().unwrap().clone();
    let b = snapshot.add_project(Path::new("/projects/b"));
    snapshot
        .projects
        .iter_mut()
        .find(|project| project.id == b)
        .unwrap()
        .collapsed = true;
    snapshot.select_workspace(a, first);
    assert!(snapshot.move_workspace_to_project(first, b));
    assert_eq!(snapshot.selected_project_id, Some(b));
    assert_eq!(snapshot.selected_workspace().unwrap(), &panes);
    assert!(!snapshot.selected_project().unwrap().collapsed);
    assert!(snapshot.projects[0].workspaces.as_ref().unwrap().is_empty());
    assert!(!snapshot.move_workspace_to_project(first, b));
    let second = snapshot.create_workspace_in_project(b).unwrap();
    assert_eq!(
        snapshot.selected_session().unwrap().working_directory,
        "/projects/b"
    );
    assert_eq!(session_ids(&snapshot), [first, second]);
    assert!(snapshot.move_workspace(second, Some(first)));
    assert_eq!(session_ids(&snapshot), [second, first]);
    assert!(!snapshot.move_workspace_relative(second, first, false));
    assert!(snapshot.move_workspace(second, None));
    assert_eq!(session_ids(&snapshot), [first, second]);
    snapshot
        .projects
        .iter_mut()
        .find(|project| project.id == b)
        .unwrap()
        .collapsed = true;
    assert_eq!(snapshot.sidebar_entries().len(), 3);
    assert!(snapshot.select_workspace(b, first));
    assert_eq!(snapshot.sidebar_entries().len(), 3);
    assert_eq!(round_trip(&snapshot), snapshot);
}

#[test]
fn relative_session_drop_moves_between_projects_and_retains_background_selection() {
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/projects/a"));
    let a = snapshot.selected_project_id.unwrap();
    let first = snapshot.selected_workspace().unwrap().id;
    snapshot.create_workspace(Path::new("/projects/b"));
    let b = snapshot.selected_project_id.unwrap();
    let second = snapshot.selected_workspace().unwrap().id;
    assert!(snapshot.move_workspace_relative(first, second, true));
    assert_eq!(snapshot.selected_project_id, Some(b));
    assert_eq!(snapshot.selected_workspace().unwrap().id, second);
    assert_eq!(session_ids(&snapshot), [second, first]);
    assert!(
        snapshot
            .projects
            .iter()
            .find(|p| p.id == a)
            .unwrap()
            .workspaces
            .as_ref()
            .unwrap()
            .is_empty()
    );
    assert_eq!(snapshot.workspace_order, [second, first]);
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

#[test]
fn project_navigation_restores_each_sessions_tabs_and_panes_including_empty_projects() {
    let mut snapshot = WorkspaceSnapshot::default();
    let a = snapshot.add_project(Path::new("/projects/a"));
    snapshot.create_workspace_in_project(a).unwrap();
    let selected_a = snapshot.create_workspace_in_project(a).unwrap();
    snapshot
        .create_terminal_tab_with_options(true, None)
        .unwrap();
    snapshot
        .split_selected_terminal(PaneSplitDirection::Down)
        .unwrap();
    let workspace_a = snapshot.selected_workspace().unwrap().clone();
    let b = snapshot.add_project(Path::new("/projects/b"));
    snapshot.create_workspace_in_project(b).unwrap();
    let workspace_b = snapshot.selected_workspace().unwrap().clone();
    let empty = snapshot.add_project(Path::new("/projects/empty"));
    let terminals = snapshot.terminal_sessions();

    assert!(snapshot.cycle_project(1));
    assert_eq!(snapshot.selected_project_id, Some(a));
    assert_eq!(snapshot.selected_workspace().unwrap().id, selected_a);
    assert_eq!(snapshot.selected_workspace(), Some(&workspace_a));
    assert!(snapshot.cycle_project(1));
    assert_eq!(snapshot.selected_project_id, Some(b));
    assert_eq!(snapshot.selected_workspace(), Some(&workspace_b));
    assert!(snapshot.cycle_project(1));
    assert_eq!(snapshot.selected_project_id, Some(empty));
    assert!(snapshot.selected_workspace().is_none());
    assert_eq!(snapshot.sidebar_entries().len(), 1);
    assert!(snapshot.cycle_project(-1));
    assert_eq!(snapshot.selected_workspace(), Some(&workspace_b));
    assert!(snapshot.select_project(a));
    assert!(snapshot.cycle_project(-1));
    assert_eq!(snapshot.selected_project_id, Some(empty));
    assert_eq!(snapshot.terminal_sessions(), terminals);
    assert_eq!(round_trip(&snapshot), snapshot);
}

#[test]
fn sidebar_shows_only_the_active_folder_even_with_legacy_collapsed_state() {
    let mut snapshot = WorkspaceSnapshot::default();
    let a = snapshot.add_project(Path::new("/projects/a"));
    let first = snapshot.create_workspace_in_project(a).unwrap();
    let second = snapshot.create_workspace_in_project(a).unwrap();
    let b = snapshot.add_project(Path::new("/projects/b"));
    snapshot.create_workspace_in_project(b).unwrap();
    snapshot.select_project(a);
    snapshot.projects[0].collapsed = true;
    let entries = snapshot.sidebar_entries();
    assert_eq!(entries.len(), 3);
    assert!(matches!(entries[0], SidebarEntry::Project { id, workspace_count: 2, .. } if id == a));
    let visible: Vec<_> = entries
        .iter()
        .filter_map(|entry| match entry {
            SidebarEntry::Workspace { entry } => Some(entry.workspace_id),
            _ => None,
        })
        .collect();
    assert_eq!(visible, [first, second]);
    assert!(snapshot.remove_project(a));
    assert_eq!(snapshot.selected_project_id, Some(b));
    assert_eq!(snapshot.sidebar_entries().len(), 2);
}

#[test]
fn project_navigation_is_a_noop_without_a_different_destination() {
    let mut snapshot = WorkspaceSnapshot::default();
    assert!(snapshot.sidebar_entries().is_empty());
    assert!(!snapshot.cycle_project(1));
    snapshot.add_project(Path::new("/projects/a"));
    assert!(!snapshot.cycle_project(-1));
    snapshot.add_project(Path::new("/projects/b"));
    let before = snapshot.clone();
    assert!(!snapshot.cycle_project(0));
    assert!(!snapshot.cycle_project(2));
    assert!(!snapshot.select_project(Uuid::new_v4()));
    assert_eq!(snapshot, before);
}
