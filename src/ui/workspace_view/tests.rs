use super::*;
use std::path::Path;

use crate::domain::workspace::WorkspaceSnapshot;
use uuid::Uuid;

#[test]
fn sidebar_branch_label_includes_tracking_and_dirty_marker() {
    let dirty = SidebarWorkspaceMeta {
        cwd: "/tmp/repo".into(),
        branch: Some("main".into()),
        ahead: 2,
        behind: 1,
        dirty: true,
    };
    assert_eq!(
        format_sidebar_branch(&dirty).as_deref(),
        Some("main* ↑2 ↓1")
    );

    let synced = SidebarWorkspaceMeta {
        cwd: "/tmp/repo".into(),
        branch: Some("feature".into()),
        ahead: 0,
        behind: 0,
        dirty: false,
    };
    // In-sync remotes stay silent so the branch line stays short.
    assert_eq!(format_sidebar_branch(&synced).as_deref(), Some("feature"));
}

#[test]
fn sidebar_path_collapses_long_segments() {
    let long = format_sidebar_path(
        "/Users/demo/very/deep/nested/project/src",
        Some(Path::new("/Users/demo")),
    );
    assert!(
        long.contains('…') || long.ends_with("project/src") || long.ends_with("nested/project"),
        "unexpected collapsed path: {long}"
    );
    assert_eq!(format_sidebar_path("", None), "—");
}

#[test]
fn directory_basename_uses_last_segment() {
    assert_eq!(directory_basename("/Users/demo/Dev/Vibra"), "Vibra");
    assert_eq!(directory_basename("/Users/demo/Dev/Vibra/"), "Vibra");
    assert_eq!(directory_basename("~"), "~");
    assert_eq!(directory_basename(""), "—");
}

#[test]
fn tab_titles_prefer_the_directory_over_generic_shell_names() {
    assert_eq!(
        tab_display_title(None, Some("zsh"), Some("/Users/demo/Dev/Vibra"), 0),
        "Vibra"
    );
    assert_eq!(
        tab_display_title(
            None,
            Some("ruben@mac: ~/Dev/Vibra/src"),
            Some("/Users/demo/Dev/Vibra"),
            0
        ),
        "Vibra"
    );
    assert_eq!(
        tab_display_title(
            None,
            Some("claude --dangerously-skip-permissions"),
            Some("/Users/demo/Dev/Vibra"),
            0,
        ),
        "claude --dangerously-skip-permissions"
    );
    assert_eq!(
        tab_display_title(
            Some("reviewer"),
            Some("claude --dangerously-skip-permissions"),
            Some("/Users/demo/Dev/Vibra"),
            0,
        ),
        "reviewer"
    );
    assert_eq!(
        tab_display_title(None, Some("claude"), Some("/Users/demo/Dev/Vibra"), 0),
        "claude"
    );
    assert_eq!(
        tab_display_title(None, Some("Terminal"), None, 2),
        "Terminal 3"
    );
}

#[test]
fn tab_titles_follow_live_cwd_without_losing_task_names() {
    for title in ["demo@mac:~/old", "demo@mac: ~/old", "/old", "~/old"] {
        assert_eq!(
            tab_display_title(None, Some(title), Some("/Dev/current"), 0),
            "current"
        );
    }
    assert_eq!(
        tab_display_title(None, Some("demo@mac:~/old"), None, 0),
        "old"
    );
    assert_eq!(
        tab_display_title(
            None,
            Some("Review: /src/parser.rs"),
            Some("/Dev/current"),
            0
        ),
        "Review: /src/parser.rs"
    );
    let name = "Revisar nombres de las sesiones";
    assert_eq!(
        tab_display_title(Some(name), Some("zsh"), Some("/Dev/current"), 0),
        name
    );
}

#[test]
fn pane_details_keep_command_and_path_for_an_alias() {
    assert_eq!(
        pane_detail_title(
            Some("reviewer"),
            Some("claude --dangerously-skip-permissions"),
            Some("/Users/demo/Dev/Vibra"),
            Some(Path::new("/Users/demo")),
        )
        .as_deref(),
        Some("claude --dangerously-skip-permissions  ·  ~/Dev/Vibra")
    );
}

struct SilentTerminalPort;

struct SilentTerminalHandle {
    events: async_channel::Receiver<crate::ports::terminal::TerminalEvent>,
    _keep_sender: async_channel::Sender<crate::ports::terminal::TerminalEvent>,
}

impl crate::ports::terminal::TerminalPort for SilentTerminalPort {
    fn backend_name(&self) -> &'static str {
        "silent"
    }

    fn spawn(
        &self,
        _: Uuid,
        _: &Path,
        _: &std::collections::HashMap<String, String>,
    ) -> anyhow::Result<std::sync::Arc<dyn crate::ports::terminal::TerminalHandle>> {
        let (sender, events) = async_channel::unbounded();
        Ok(std::sync::Arc::new(SilentTerminalHandle {
            events,
            _keep_sender: sender,
        }))
    }
}

impl crate::ports::terminal::TerminalHandle for SilentTerminalHandle {
    fn events(&self) -> async_channel::Receiver<crate::ports::terminal::TerminalEvent> {
        self.events.clone()
    }
    fn send_input(&self, _: Vec<u8>) -> anyhow::Result<()> {
        Ok(())
    }
    fn send_key_input(
        &self,
        input: crate::ports::terminal_keyboard::TerminalKeyInput,
    ) -> anyhow::Result<()> {
        self.send_input(input.bytes(self.input_mode()))
    }
    fn resize(&self, _: crate::ports::terminal::TerminalSize) -> anyhow::Result<()> {
        Ok(())
    }
    fn scroll(&self, _: i32) {}
    fn clear_scrollback(&self) {}
    fn snapshot(&self) -> std::sync::Arc<crate::ports::terminal::TerminalSnapshot> {
        std::sync::Arc::new(crate::ports::terminal::TerminalSnapshot {
            columns: 80,
            rows: 24,
            lines: Vec::new(),
            cursor: None,
            display_offset: 0,
            history_size: 0,
        })
    }
    fn input_mode(&self) -> crate::ports::terminal::TerminalInputMode {
        crate::ports::terminal::TerminalInputMode::default()
    }
    fn clear_selection(&self) {}
    fn start_selection(
        &self,
        _: crate::ports::terminal::TerminalSelectionType,
        _: crate::ports::terminal::TerminalPoint,
        _: crate::ports::terminal::TerminalCellSide,
    ) {
    }
    fn update_selection(
        &self,
        _: crate::ports::terminal::TerminalPoint,
        _: crate::ports::terminal::TerminalCellSide,
    ) {
    }
    fn selection_text(&self) -> Option<String> {
        None
    }
    fn search(
        &self,
        _: &str,
        _: crate::ports::terminal::TerminalSearchDirection,
    ) -> anyhow::Result<bool> {
        Ok(true)
    }
    fn hyperlink_at(&self, _: crate::ports::terminal::TerminalPoint) -> Option<String> {
        None
    }
    fn acknowledge_wakeup(&self) {}
    fn shutdown(&self) {}
}

#[gpui::test]
fn startup_preserves_unavailable_theme_preference(cx: &mut gpui::TestAppContext) {
    use crate::infrastructure::files::LocalFileSystemPort;
    use crate::infrastructure::git::GitCliPort;

    let root = std::env::temp_dir().join(format!("vibra-theme-startup-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let settings_repository = SettingsRepository::at(root.join("settings.json"));
    let selected_theme = format!("missing-{}", Uuid::new_v4());
    settings_repository
        .save(&AppSettings {
            theme_id: selected_theme.clone(),
            ..AppSettings::default()
        })
        .unwrap();
    let repository = WorkspaceRepository::at(root.join("workspace.json"));
    let window = cx
        .update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                let focus_handle = cx.focus_handle();
                cx.new(|cx| {
                    WorkspaceView::new(
                        WorkspaceDependencies {
                            repository,
                            settings_repository,
                            terminal_port: Arc::new(SilentTerminalPort),
                            file_port: Arc::new(LocalFileSystemPort),
                            git_port: Arc::new(GitCliPort::default()),
                        },
                        root.clone(),
                        focus_handle,
                        cx,
                    )
                })
            })
        })
        .unwrap();
    window
        .update(cx, |view, window, _| {
            assert_eq!(view.settings.theme_id, selected_theme);
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn switching_tabs_and_workspaces_hides_offscreen_terminals(cx: &mut gpui::TestAppContext) {
    use crate::infrastructure::files::LocalFileSystemPort;
    use crate::infrastructure::git::GitCliPort;
    use crate::infrastructure::persistence::WorkspaceRepository;
    use crate::infrastructure::settings::SettingsRepository;
    use std::sync::Arc;

    let root = std::env::temp_dir().join(format!("vibra-surface-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let repository = WorkspaceRepository::at(root.join("workspace.json"));
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(&root);
    snapshot.create_terminal_tab_with_options(true, None);
    let first_project = snapshot.selected_project_id.unwrap();
    let first_workspace = snapshot.selected_workspace().unwrap().id;
    snapshot.create_workspace(&root);
    let second_workspace = snapshot.selected_workspace().unwrap().id;
    repository.save(&snapshot).unwrap();
    let settings_repository = SettingsRepository::at(root.join("settings.json"));
    let settings = crate::infrastructure::settings::AppSettings {
        agent_notifications: false,
        ..crate::infrastructure::settings::AppSettings::default()
    };
    settings_repository.save(&settings).unwrap();

    let window = cx
        .update(|cx| {
            cx.open_window(Default::default(), |window, cx| {
                let focus_handle = cx.focus_handle();
                focus_handle.focus(window);
                cx.new(|cx| {
                    WorkspaceView::new(
                        WorkspaceDependencies {
                            repository,
                            settings_repository,
                            terminal_port: Arc::new(SilentTerminalPort),
                            file_port: Arc::new(LocalFileSystemPort),
                            git_port: Arc::new(GitCliPort::default()),
                        },
                        root.clone(),
                        focus_handle,
                        cx,
                    )
                })
            })
        })
        .unwrap();

    window
        .update(cx, |view, window, cx| {
            let first = view
                .snapshot
                .projects
                .iter()
                .flat_map(|project| project.workspaces.as_deref().unwrap_or_default())
                .find(|workspace| workspace.id == first_workspace)
                .unwrap();
            let first_tab = first.tabs[0].id;
            let second_tab = first.tabs[1].id;
            let first_session = first.tabs[0].sessions[0].id;
            let second_session = first.tabs[1].sessions[0].id;
            let other_session = view.snapshot.selected_session().unwrap().id;
            assert_eq!(
                view.snapshot.selected_workspace().unwrap().id,
                second_workspace
            );
            assert!(view.terminals[&other_session].read(cx).is_surface_visible());
            assert!(!view.terminals[&first_session].read(cx).is_surface_visible());
            assert!(
                !view.terminals[&second_session]
                    .read(cx)
                    .is_surface_visible()
            );

            view.select_workspace(first_project, first_workspace, window, cx);
            assert!(
                view.terminals[&second_session]
                    .read(cx)
                    .is_surface_visible()
            );
            assert!(
                !view.terminals[&other_session].read(cx).is_surface_visible(),
                "sessions in the unselected workspace must stop cwd/agent polls"
            );

            view.select_tab(first_tab, window, cx);
            assert!(view.terminals[&first_session].read(cx).is_surface_visible());
            assert!(
                !view.terminals[&second_session]
                    .read(cx)
                    .is_surface_visible(),
                "the unselected tab must not keep cwd/agent polls running"
            );

            view.select_tab(second_tab, window, cx);
            assert!(!view.terminals[&first_session].read(cx).is_surface_visible());
            assert!(
                view.terminals[&second_session]
                    .read(cx)
                    .is_surface_visible()
            );

            // Changing a terminal's cwd must not switch project ownership or Files.
            view.handle_terminal_view_event(
                &TerminalViewEvent::WorkingDirectoryChanged {
                    session_id: second_session,
                    path: root.join("another-folder"),
                },
                cx,
            );
            assert_eq!(view.project_root(), root);
            assert_eq!(view.snapshot.selected_project_id, Some(first_project));

            assert!(
                view.snapshot
                    .close_workspace(first_project, first_workspace)
            );
            assert!(
                view.snapshot
                    .close_workspace(first_project, second_workspace)
            );
            view.reconcile_terminal_views(cx);
            view.apply_workspace_selection_change(window, cx);
            assert!(view.terminals.is_empty());
            assert_eq!(view.project_root(), root);
            assert!(view.has_project_context());
            assert_eq!(view.snapshot.selected_project_id, Some(first_project));
            view.flush_persist(cx);
            if let Some(queue) = &view.persistence_queue {
                queue.wait_for_idle();
            }
            assert_eq!(view.repository.load().unwrap().unwrap(), view.snapshot);

            assert!(view.snapshot.remove_project(first_project));
            view.apply_workspace_selection_change(window, cx);
            assert!(!view.has_project_context());
            assert!(view.project_files.is_empty());
            assert!(view.files_watch.is_none());
            view.flush_persist(cx);
            if let Some(queue) = &view.persistence_queue {
                queue.wait_for_idle();
            }
            assert!(view.repository.load().unwrap().unwrap().projects.is_empty());
        })
        .unwrap();

    window
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
