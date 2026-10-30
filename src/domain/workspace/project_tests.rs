use std::path::Path;

use uuid::Uuid;

use super::*;

fn session_ids(snapshot: &WorkspaceSnapshot) -> Vec<Uuid> {
    snapshot
        .projects
        .iter()
        .flat_map(|project| project.workspaces.iter().flatten())
        .map(|workspace| workspace.id)
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
    assert!(snapshot.move_project(b, Some(a)));
    assert!(!snapshot.move_project(b, Some(a)));
    assert_eq!(snapshot.projects[0].id, b);
    assert_eq!(round_trip(&snapshot), snapshot);
    assert!(snapshot.projects[0].workspaces.as_ref().unwrap().is_empty());
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
    assert_eq!(session_ids(&snapshot).len(), 2);
    assert_eq!(snapshot.terminal_sessions().count(), 2);
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
    assert_eq!(snapshot.terminal_sessions().count(), 2);
}

#[test]
fn normalization_preserves_all_tabs_when_selected_tab_id_repeats() {
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(Path::new("/projects/a"));
    let first_tab_id = snapshot.selected_tab().unwrap().id;
    snapshot
        .open_tab_in_project(snapshot.selected_project_id.unwrap(), true)
        .unwrap();
    let workspace = &mut snapshot.projects[0].workspaces.as_mut().unwrap()[0];
    workspace.tabs[1].id = first_tab_id;
    workspace.selected_tab_id = Some(first_tab_id);

    snapshot.normalize();

    let workspace = snapshot.selected_workspace().unwrap();
    assert_eq!(workspace.tabs.len(), 2);
    assert_ne!(workspace.tabs[0].id, workspace.tabs[1].id);
    assert_eq!(snapshot.selected_tab().unwrap().id, workspace.tabs[0].id);
    assert_eq!(snapshot.terminal_sessions().count(), 2);
}

#[test]
fn closing_the_last_session_or_pane_keeps_the_project() {
    for close_pane in [false, true] {
        let mut snapshot = WorkspaceSnapshot::default();
        snapshot.create_workspace(Path::new("/projects/a"));
        let project_id = snapshot.selected_project_id.unwrap();
        let workspace_id = snapshot.selected_workspace().unwrap().id;
        if close_pane {
            assert!(snapshot.close_terminal(snapshot.selected_session().unwrap().id));
        } else {
            assert!(snapshot.close_workspace(project_id, workspace_id));
        }
        assert_eq!(snapshot.projects.len(), 1);
        assert_eq!(snapshot.selected_project_id, Some(project_id));
        assert!(snapshot.selected_workspace().is_none());
        assert!(snapshot.terminal_sessions().next().is_none());
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
    let sessions: Vec<_> = snapshot.terminal_sessions().cloned().collect();
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
    assert_eq!(
        snapshot.terminal_sessions().cloned().collect::<Vec<_>>(),
        sessions
    );
    assert!(snapshot.create_workspace_in_project(mixed).is_none());
    assert!(snapshot.set_project_directory(mixed, Path::new("/projects/chosen")));
    assert_eq!(
        snapshot.terminal_sessions().cloned().collect::<Vec<_>>(),
        sessions
    );
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
    assert!(flat.workspace_order.is_empty());
    assert!(
        serde_json::to_value(&flat)
            .unwrap()
            .get("workspaceOrder")
            .is_none()
    );
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
    assert_eq!(snapshot.projects[0].name, "Workspace");
    assert_eq!(snapshot.selected_workspace().unwrap().id, second);
}

#[test]
fn sessions_merge_into_one_row_of_tabs_per_project() {
    let root = std::env::temp_dir();
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(&root);
    snapshot.open_tab_in_project(snapshot.selected_project_id.unwrap(), true);
    let first_tabs: Vec<_> = snapshot
        .selected_workspace()
        .unwrap()
        .tabs
        .iter()
        .map(|tab| tab.id)
        .collect();
    let project = snapshot.selected_project_id.unwrap();
    snapshot.create_workspace_in_project(project);
    let second = snapshot.selected_workspace().unwrap().clone();
    let selected_tab = second.selected_tab_id;

    assert!(snapshot.consolidate_project_sessions());
    let project = snapshot.selected_project().unwrap();
    let workspaces = project.workspaces.as_ref().unwrap();
    assert_eq!(workspaces.len(), 1);
    let tabs: Vec<_> = workspaces[0].tabs.iter().map(|tab| tab.id).collect();
    assert_eq!(tabs.len(), 3);
    assert_eq!(workspaces[0].id, second.id, "the selected session stays");
    assert_eq!(snapshot.selected_tab().map(|tab| tab.id), selected_tab);
    assert!(first_tabs.iter().all(|tab| tabs.contains(tab)));
    assert!(!snapshot.consolidate_project_sessions());
}

#[test]
fn tabs_open_in_the_project_session_without_stealing_focus() {
    let root = std::env::temp_dir();
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(&root);
    let first_project = snapshot.selected_project_id.unwrap();
    let selected_tab = snapshot.selected_tab().unwrap().id;
    let other = snapshot.add_project(&root.join("vibra-other-project"));
    snapshot.set_project_directory(other, &root);
    snapshot.select_project(first_project);

    // A project without a session gets one, in the background.
    let (tab, _) = snapshot.open_tab_in_project(other, false).unwrap();
    assert_eq!(snapshot.selected_project_id, Some(first_project));
    assert_eq!(snapshot.selected_tab().unwrap().id, selected_tab);
    let (second_tab, _) = snapshot.open_tab_in_project(other, false).unwrap();
    let other_project = snapshot.projects.iter().find(|p| p.id == other).unwrap();
    let workspaces = other_project.workspaces.as_ref().unwrap();
    assert_eq!(workspaces.len(), 1, "tabs share the project's session");
    assert_eq!(workspaces[0].tabs.len(), 2);

    let (focused, _) = snapshot.open_tab_in_project(first_project, true).unwrap();
    assert_eq!(snapshot.selected_tab().unwrap().id, focused);
    assert_ne!(tab, second_tab);
    assert_eq!(
        snapshot
            .selected_project()
            .unwrap()
            .workspaces
            .as_ref()
            .unwrap()
            .len(),
        1
    );
}

fn locate_in_folder(cwd: &Path) -> Option<(std::path::PathBuf, String)> {
    let name = cwd.file_name()?.to_string_lossy().into_owned();
    Some((cwd.to_path_buf(), name))
}

#[test]
fn spaces_follow_their_selected_terminal_unless_the_user_named_them() {
    let mut snapshot = WorkspaceSnapshot::default();
    let space = snapshot.add_space(Path::new("/home/demo"), "~".into());
    snapshot.open_tab_in_project(space, true);
    let pane = snapshot.selected_session().unwrap().id;

    snapshot.update_session_working_directory(pane, Path::new("/home/demo/Dev/app"));
    assert!(snapshot.follow_space_locations(locate_in_folder));
    let project = snapshot.project(space).unwrap();
    assert_eq!(project.name, "app");
    assert_eq!(project.directory(), Some("/home/demo/Dev/app"));
    assert!(!snapshot.follow_space_locations(locate_in_folder));

    // A folder that is gone leaves the space where it was.
    snapshot.update_session_working_directory(pane, Path::new("/home/demo/removed"));
    assert!(!snapshot.follow_space_locations(|_| None));
    assert_eq!(snapshot.project(space).unwrap().name, "app");

    assert!(snapshot.rename_project(space, "Work"));
    snapshot.update_session_working_directory(pane, Path::new("/home/demo/Dev/site"));
    assert!(snapshot.follow_space_locations(locate_in_folder));
    let project = snapshot.project(space).unwrap();
    assert_eq!(project.name, "Work");
    assert_eq!(project.directory(), Some("/home/demo/Dev/site"));

    assert!(snapshot.name_space_after_folder(space, "site".into()));
    snapshot.update_session_working_directory(pane, Path::new("/home/demo/Dev/app"));
    assert!(snapshot.follow_space_locations(locate_in_folder));
    assert_eq!(snapshot.project(space).unwrap().name, "app");
    assert_eq!(round_trip(&snapshot), snapshot);
}

#[test]
fn spaces_follow_the_selected_tab() {
    let mut snapshot = WorkspaceSnapshot::default();
    let space = snapshot.add_space(Path::new("/work/a"), "a".into());
    snapshot.open_tab_in_directory(space, "/work/a".into(), true);
    snapshot.open_tab_in_directory(space, "/work/b".into(), true);

    assert!(snapshot.follow_space_locations(locate_in_folder));
    assert_eq!(snapshot.project(space).unwrap().name, "b");
    assert_eq!(
        snapshot.project(space).unwrap().directory(),
        Some("/work/b")
    );
}

#[test]
fn a_saved_scratch_terminal_becomes_a_space_that_follows_it() {
    let mut saved = WorkspaceSnapshot::default();
    let id = saved.add_project(Path::new("/unused"));
    saved.open_tab_in_directory(id, "/work/notes".into(), true);
    let mut value = serde_json::to_value(&saved).unwrap();
    let project = &mut value["projects"][0];
    project["name"] = "Scratch".into();
    project["rootPath"] = "".into();
    project["scratch"] = true.into();
    project.as_object_mut().unwrap().remove("nameSource");
    let mut snapshot: WorkspaceSnapshot = serde_json::from_value(value).unwrap();
    snapshot.normalize();

    assert!(snapshot.follow_space_locations(locate_in_folder));
    let space = snapshot.project(id).unwrap();
    assert_eq!(space.name, "notes");
    assert_eq!(space.directory(), Some("/work/notes"));
    let saved = serde_json::to_value(space).unwrap();
    assert!(saved.get("scratch").is_none());
}

#[test]
fn spaces_saved_before_following_keep_the_names_the_user_chose() {
    let mut snapshot = WorkspaceSnapshot::default();
    let plain = snapshot.add_project(Path::new("/work/plain"));
    let renamed = snapshot.add_project(Path::new("/work/renamed"));
    snapshot.rename_project(renamed, "Client");
    for id in [plain, renamed] {
        snapshot.project_mut(id).unwrap().name_source = None;
        snapshot.open_tab_in_directory(id, "/work/elsewhere".into(), true);
    }

    assert!(snapshot.follow_space_locations(locate_in_folder));
    let plain = snapshot.project(plain).unwrap();
    assert_eq!(plain.name, "elsewhere");
    assert_eq!(plain.name_source, Some(WorkspaceTitleSource::Automatic));
    let renamed = snapshot.project(renamed).unwrap();
    assert_eq!(renamed.name, "Client");
    assert_eq!(renamed.name_source, Some(WorkspaceTitleSource::Manual));
    assert_eq!(renamed.directory(), Some("/work/elsewhere"));
}

#[test]
fn new_spaces_never_reuse_one_and_go_away_with_their_last_terminal() {
    let mut snapshot = WorkspaceSnapshot::default();
    let first = snapshot.add_space(Path::new("/home/demo"), "~".into());
    let second = snapshot.add_space(Path::new("/home/demo"), "~".into());
    let reviewing = snapshot.add_space(Path::new("/home/demo"), "~".into());
    assert_ne!(first, second);
    assert_eq!(snapshot.selected_project_id, Some(reviewing));

    snapshot.open_tab_in_project(first, true);
    assert_eq!(snapshot.remove_empty_spaces(Some(reviewing)), vec![second]);
    assert_eq!(
        snapshot.projects.iter().map(|p| p.id).collect::<Vec<_>>(),
        vec![first, reviewing]
    );
    assert_eq!(snapshot.remove_empty_spaces(None), vec![reviewing]);
    assert_eq!(snapshot.selected_project_id, Some(first));
    assert!(snapshot.remove_empty_spaces(None).is_empty());
}
