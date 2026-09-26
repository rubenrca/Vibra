use super::*;
use std::path::Path;

use crate::domain::workspace::WorkspaceSnapshot;
use uuid::Uuid;

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

/// Every write to a terminal, tagged with the pane that received it.
type SentInput = Arc<std::sync::Mutex<Vec<(Uuid, Vec<u8>)>>>;

struct SilentTerminalHandle {
    events: async_channel::Receiver<crate::ports::terminal::TerminalEvent>,
    _keep_sender: async_channel::Sender<crate::ports::terminal::TerminalEvent>,
    inputs: SentInput,
    session_id: Uuid,
}

/// Records what each terminal was sent, as the shell would read it.
struct RecordingTerminalPort {
    inputs: SentInput,
}

impl crate::ports::terminal::TerminalPort for RecordingTerminalPort {
    fn backend_name(&self) -> &'static str {
        "recording"
    }

    fn spawn(
        &self,
        session_id: Uuid,
        _: &Path,
        _: &std::collections::HashMap<String, String>,
    ) -> anyhow::Result<Arc<dyn crate::ports::terminal::TerminalHandle>> {
        let (sender, events) = async_channel::unbounded();
        Ok(Arc::new(SilentTerminalHandle {
            events,
            _keep_sender: sender,
            inputs: self.inputs.clone(),
            session_id,
        }))
    }
}

impl crate::ports::terminal::TerminalPort for SilentTerminalPort {
    fn backend_name(&self) -> &'static str {
        "silent"
    }

    fn spawn(
        &self,
        session_id: Uuid,
        _: &Path,
        _: &std::collections::HashMap<String, String>,
    ) -> anyhow::Result<std::sync::Arc<dyn crate::ports::terminal::TerminalHandle>> {
        let (sender, events) = async_channel::unbounded();
        Ok(std::sync::Arc::new(SilentTerminalHandle {
            events,
            _keep_sender: sender,
            inputs: Default::default(),
            session_id,
        }))
    }
}

struct CountingSilentTerminalPort {
    spawns: Arc<std::sync::atomic::AtomicUsize>,
}

impl crate::ports::terminal::TerminalPort for CountingSilentTerminalPort {
    fn backend_name(&self) -> &'static str {
        "silent-counting"
    }

    fn spawn(
        &self,
        session_id: Uuid,
        cwd: &Path,
        environment: &std::collections::HashMap<String, String>,
    ) -> anyhow::Result<Arc<dyn crate::ports::terminal::TerminalHandle>> {
        self.spawns
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        crate::ports::terminal::TerminalPort::spawn(
            &SilentTerminalPort,
            session_id,
            cwd,
            environment,
        )
    }
}

impl crate::ports::terminal::TerminalHandle for SilentTerminalHandle {
    fn events(&self) -> async_channel::Receiver<crate::ports::terminal::TerminalEvent> {
        self.events.clone()
    }
    fn send_input(&self, input: Vec<u8>) -> anyhow::Result<()> {
        self.inputs.lock().unwrap().push((self.session_id, input));
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

            // Global pages hide native terminal surfaces without changing the session
            // the user will return to or restarting any terminal process.
            let retained_snapshot = view.snapshot.clone();
            let terminal_count = view.terminals.len();
            for section in [
                WorkspaceSection::Inbox,
                WorkspaceSection::Notes,
                WorkspaceSection::Automations,
            ] {
                view.select_section(section, window, cx);
                assert!(
                    view.terminals
                        .values()
                        .all(|terminal| !terminal.read(cx).is_surface_visible()),
                    "global pages must hide all terminal surfaces"
                );
                assert_eq!(view.snapshot, retained_snapshot);
                assert_eq!(view.terminals.len(), terminal_count);

                // Cmd-W closes the global page, never its hidden terminal.
                view.close_terminal(&CloseTerminal, window, cx);
                assert_eq!(view.workspace_section, WorkspaceSection::Workspace);
                assert!(
                    view.terminals[&second_session]
                        .read(cx)
                        .is_surface_visible()
                );
                assert!(
                    view.terminals[&second_session]
                        .read(cx)
                        .focus_handle(cx)
                        .is_focused(window)
                );
                assert!(!view.terminals[&first_session].read(cx).is_surface_visible());
                assert!(!view.terminals[&other_session].read(cx).is_surface_visible());
                assert_eq!(view.snapshot, retained_snapshot);
                assert_eq!(view.terminals.len(), terminal_count);
            }

            // Opening a workspace utility from global navigation restores the
            // active session and keeps the tools in the right sidebar.
            view.select_section(WorkspaceSection::Notes, window, cx);
            view.set_workspace_mode(RightSidebarMode::Files, cx);
            assert_eq!(view.workspace_section, WorkspaceSection::Workspace);
            assert_eq!(view.right_sidebar_mode, RightSidebarMode::Files);
            assert!(view.right_sidebar_visible);
            assert!(
                view.terminals[&second_session]
                    .read(cx)
                    .is_surface_visible()
            );
            assert_eq!(view.snapshot, retained_snapshot);

            // The right-sidebar shortcut also returns keyboard focus to the
            // terminal when invoked from a global page, even if tools were open.
            view.select_section(WorkspaceSection::Notes, window, cx);
            view.toggle_right_sidebar(&ToggleRightSidebar, window, cx);
            assert_eq!(view.workspace_section, WorkspaceSection::Workspace);
            assert!(view.right_sidebar_visible);
            assert!(
                view.terminals[&second_session]
                    .read(cx)
                    .is_surface_visible()
            );
            assert!(
                view.terminals[&second_session]
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
            );
            assert_eq!(view.snapshot, retained_snapshot);
            assert_eq!(view.terminals.len(), terminal_count);

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

#[gpui::test]
fn central_review_preserves_terminals_when_sidebar_closes_and_restores_terminal_navigation(
    cx: &mut gpui::TestAppContext,
) {
    use crate::infrastructure::files::LocalFileSystemPort;
    use crate::infrastructure::git::GitCliPort;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let root = std::env::temp_dir().join(format!("vibra-central-review-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let repository = WorkspaceRepository::at(root.join("workspace.json"));
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(&root);
    snapshot.create_terminal_tab_with_options(true, None);
    let first_tab = snapshot.selected_workspace().unwrap().tabs[0].id;
    let first_session = snapshot.selected_workspace().unwrap().tabs[0].sessions[0].id;
    let selected_session = snapshot.selected_session().unwrap().id;
    repository.save(&snapshot).unwrap();
    let settings_repository = SettingsRepository::at(root.join("settings.json"));
    settings_repository
        .save(&AppSettings {
            agent_notifications: false,
            ..AppSettings::default()
        })
        .unwrap();
    let spawns = Arc::new(AtomicUsize::new(0));
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
                            terminal_port: Arc::new(CountingSilentTerminalPort {
                                spawns: spawns.clone(),
                            }),
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
    let initial_spawns = spawns.load(Ordering::SeqCst);
    assert_eq!(initial_spawns, 2);

    window
        .update(cx, |view, _, cx| {
            assert!(view.has_project_context());
            assert!(
                view.terminals[&selected_session]
                    .read(cx)
                    .is_surface_visible()
            );
            view.set_workspace_mode(RightSidebarMode::Diff, cx);
            view.diff_view
                .update(cx, |diff, cx| diff.set_review_expanded(true, cx));
        })
        .unwrap();

    // Let the DiffView event reach the workspace, as it does for a file click.
    window
        .update(cx, |view, _, cx| {
            assert!(view.diff_view.read(cx).review_expanded());
            // A review opens as a full tab; shown beside the terminal, the
            // terminal keeps painting.
            assert!(view.review_covers_terminal(cx));
            view.diff_view
                .update(cx, |diff, cx| diff.set_review_focused(false, cx));
        })
        .unwrap();

    window
        .update(cx, |view, _, cx| {
            assert!(
                view.terminals[&selected_session]
                    .read(cx)
                    .is_surface_visible()
            );
            view.diff_view
                .update(cx, |diff, cx| diff.set_review_focused(true, cx));
        })
        .unwrap();

    window
        .update(cx, |view, _, cx| {
            assert!(view.visible_terminal_ids(cx).is_empty());
            assert!(
                view.terminals
                    .values()
                    .all(|terminal| !terminal.read(cx).is_surface_visible())
            );
            assert_eq!(view.snapshot, snapshot);
            assert_eq!(spawns.load(Ordering::SeqCst), initial_spawns);
            view.set_right_sidebar_visible(false, false, cx);
            assert!(!view.right_sidebar_visible);
            assert!(
                view.diff_view.read(cx).review_expanded(),
                "the file navigator can close independently of the central review"
            );
            assert!(view.visible_terminal_ids(cx).is_empty());
            assert!(
                view.terminals
                    .values()
                    .all(|terminal| !terminal.read(cx).is_surface_visible())
            );
        })
        .unwrap();

    window
        .update(cx, |view, window, cx| {
            // Cmd-W closes the review, leaving its terminal session alive.
            view.close_terminal(&CloseTerminal, window, cx);
            assert!(!view.diff_view.read(cx).review_expanded());
            assert!(
                view.terminals[&selected_session]
                    .read(cx)
                    .is_surface_visible()
            );
            assert!(
                view.terminals[&selected_session]
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
            );
            assert_eq!(view.snapshot, snapshot);
            assert_eq!(spawns.load(Ordering::SeqCst), initial_spawns);
            view.diff_view
                .update(cx, |diff, cx| diff.set_review_expanded(true, cx));
            assert!(
                view.diff_view.read(cx).review_focused(),
                "reviews reopen as a full tab"
            );
        })
        .unwrap();

    window
        .update(cx, |view, window, cx| {
            assert!(
                view.terminals
                    .values()
                    .all(|terminal| !terminal.read(cx).is_surface_visible())
            );
            view.record_navigation(cx);
            view.select_tab(first_tab, window, cx);
            // The review stays open as a tab behind the terminal tab.
            assert!(view.diff_view.read(cx).review_expanded());
            assert!(!view.review_visible(cx));
            assert_eq!(view.snapshot.selected_tab().unwrap().id, first_tab);
            assert!(view.terminals[&first_session].read(cx).is_surface_visible());
            assert!(
                !view.terminals[&selected_session]
                    .read(cx)
                    .is_surface_visible()
            );
            assert!(
                view.terminals[&first_session]
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
            );
            assert_eq!(view.terminals.len(), initial_spawns);
            assert_eq!(spawns.load(Ordering::SeqCst), initial_spawns);

            // Back returns to the review tab; forward to the terminal tab.
            view.navigate_back(&crate::NavigateBack, window, cx);
            assert!(view.review_visible(cx));
            view.navigate_forward(&crate::NavigateForward, window, cx);
            assert!(!view.review_visible(cx));
            assert_eq!(view.snapshot.selected_tab().unwrap().id, first_tab);

            // With two terminal tabs, ⌘3 is the review tab and ⌘W closes it.
            view.go_to_tab(&crate::GoToTab { index: 3 }, window, cx);
            assert!(view.review_visible(cx));
            view.close_terminal(&CloseTerminal, window, cx);
            assert!(!view.diff_view.read(cx).review_expanded());
            assert_eq!(view.terminals.len(), initial_spawns);
            view.flush_persist(cx);
            if let Some(queue) = &view.persistence_queue {
                queue.wait_for_idle();
            }
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

type RecordingWorkspace = (
    PathBuf,
    WorkspaceSnapshot,
    SentInput,
    gpui::WindowHandle<WorkspaceView>,
);

fn open_recording_workspace(cx: &mut gpui::TestAppContext, name: &str) -> RecordingWorkspace {
    use crate::infrastructure::files::LocalFileSystemPort;
    use crate::infrastructure::git::GitCliPort;

    let root = std::env::temp_dir().join(format!("vibra-{name}-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let repository = WorkspaceRepository::at(root.join("workspace.json"));
    let mut snapshot = WorkspaceSnapshot::default();
    snapshot.create_workspace(&root);
    snapshot.split_selected_terminal(PaneSplitDirection::Right);
    repository.save(&snapshot).unwrap();
    let settings_repository = SettingsRepository::at(root.join("settings.json"));
    settings_repository
        .save(&AppSettings {
            agent_notifications: false,
            ..AppSettings::default()
        })
        .unwrap();
    let inputs: SentInput = Default::default();
    let port = RecordingTerminalPort {
        inputs: inputs.clone(),
    };
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
                            terminal_port: Arc::new(port),
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
    (root, snapshot, inputs, window)
}

#[gpui::test]
fn scheduled_automations_run_in_a_new_session_without_stealing_focus(
    cx: &mut gpui::TestAppContext,
) {
    use crate::domain::inbox::InboxKind;
    use crate::domain::library::AutomationSchedule;

    let (root, snapshot, inputs, window) = open_recording_workspace(cx, "automation");
    let selected_workspace = snapshot.selected_workspace().unwrap().id;
    window
        .update(cx, |view, window, cx| {
            let id = view
                .library
                .save_automation(
                    None,
                    "Resumen",
                    "echo hola",
                    None,
                    AutomationSchedule::Daily { hour: 0, minute: 0 },
                    "09:00",
                )
                .unwrap();
            let session = view.run_automation(id, Some(42), false, cx).unwrap();

            assert_eq!(
                view.snapshot.selected_workspace().unwrap().id,
                selected_workspace,
                "a scheduled run keeps what the user is looking at"
            );
            let workspace = view
                .snapshot
                .workspace_entries()
                .into_iter()
                .find(|entry| entry.workspace_name == "Resumen")
                .expect("the run opens a session named after the automation");
            assert!(workspace.title_is_manual);
            assert!(view.terminals.contains_key(&session));
            assert!(
                inputs
                    .lock()
                    .unwrap()
                    .contains(&(session, b"echo hola\r".to_vec()))
            );
            let automation = view.library.automation(id).unwrap();
            assert_eq!(automation.last_scheduled_slot, Some(42));
            assert!(automation.last_run_at.is_some());
            let item = view.inbox.items().next().unwrap();
            assert_eq!(item.kind, InboxKind::AutomationStarted);
            assert_eq!(item.pane_id, Some(session));
            assert!(!item.read);

            // Opening it from the Inbox shows the run and acknowledges it.
            view.select_section(WorkspaceSection::Inbox, window, cx);
            view.open_pane(session, window, cx);
            assert_eq!(view.workspace_section, WorkspaceSection::Workspace);
            assert_eq!(view.snapshot.selected_session().unwrap().id, session);
            assert_eq!(view.inbox.unread_count(), 0);
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn agents_that_finish_in_another_pane_land_in_the_inbox(cx: &mut gpui::TestAppContext) {
    use crate::domain::inbox::InboxKind;
    use crate::ports::terminal::TerminalAgentPresence;

    let (root, snapshot, _, window) = open_recording_workspace(cx, "inbox");
    let selected = snapshot.selected_session().unwrap().id;
    let other = snapshot
        .selected_tab()
        .unwrap()
        .sessions
        .iter()
        .map(|session| session.id)
        .find(|id| *id != selected)
        .unwrap();
    let presence = |state| TerminalAgentPresence {
        kind: "Codex".into(),
        kind_source: TerminalAgentKindSource::Process,
        state,
        process_id: Some(7),
    };
    window
        .update(cx, |view, window, cx| {
            view.window_is_active = true;
            for (session_id, state) in [
                (selected, AgentRuntimeState::Working),
                (other, AgentRuntimeState::Working),
                (selected, AgentRuntimeState::Idle),
                (other, AgentRuntimeState::Idle),
            ] {
                view.handle_terminal_view_event(
                    &TerminalViewEvent::AgentPresenceChanged {
                        session_id,
                        presence: Some(presence(state)),
                    },
                    cx,
                );
            }
            let items: Vec<_> = view.inbox.items().cloned().collect();
            assert_eq!(items.len(), 2);
            assert_eq!(items[0].pane_id, Some(other));
            assert_eq!(items[0].kind, InboxKind::Finished);
            assert_eq!(items[0].title, "Codex terminó");
            assert!(!items[0].read);
            assert!(
                items[1].read,
                "the pane the user is watching does not count as unread"
            );
            view.handle_terminal_view_event(
                &TerminalViewEvent::Activated { session_id: other },
                cx,
            );
            assert_eq!(view.inbox.unread_count(), 0);
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn notes_are_typed_saved_and_pasted_into_the_project_terminal(cx: &mut gpui::TestAppContext) {
    let (root, snapshot, inputs, window) = open_recording_workspace(cx, "notes");
    let selected = snapshot.selected_session().unwrap().id;
    window
        .update(cx, |view, window, cx| {
            view.create_note(window, cx);
            assert_eq!(view.workspace_section, WorkspaceSection::Notes);
        })
        .unwrap();
    cx.run_until_parked();
    cx.simulate_keystrokes(window.into(), "h->h i->i enter o->o shift-k->K");
    let note_id = window
        .update(cx, |view, _, _| {
            let note = view.library.note(view.selected_note_id.unwrap()).unwrap();
            assert_eq!(note.body, "hi\noK");
            assert_eq!(note.title(), "hi");
            assert_eq!(note.project_id, snapshot.selected_project_id);
            note.id
        })
        .unwrap();
    // A multi-line paste into a shell without bracketed paste asks first;
    // keep one line so it reaches the shell directly.
    cx.simulate_keystrokes(window.into(), "cmd-backspace escape");
    window
        .update(cx, |view, window, cx| {
            assert!(!view.note_editing);
            view.save_library_blocking();
            let saved = crate::infrastructure::library::LibraryRepository::in_directory(&root)
                .load()
                .unwrap();
            assert_eq!(saved.notes.len(), 1);
            view.paste_note_into_terminal(note_id, window, cx);
            assert_eq!(view.workspace_section, WorkspaceSection::Workspace);
            let sent = inputs.lock().unwrap();
            let (target, bytes) = sent.last().unwrap();
            assert_eq!(*target, selected);
            let text = String::from_utf8_lossy(bytes);
            assert_eq!(text, "hi");
            assert!(!text.ends_with('\r'), "a pasted note is never submitted");
            drop(sent);
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn explorer_creates_entries_inside_the_project(cx: &mut gpui::TestAppContext) {
    let (root, _, _, window) = open_recording_workspace(cx, "explorer");
    window
        .update(cx, |view, window, cx| {
            view.confirm_new_entry(root.clone(), "docs/plan.md", false, cx);
            assert!(root.join("docs/plan.md").is_file());
            assert_eq!(view.selected_file_path, Some(root.join("docs/plan.md")));
            assert!(view.expanded_directories.contains(&root.join("docs")));
            view.confirm_new_entry(root.clone(), "docs/plan.md", false, cx);
            assert!(view.persistence_error.is_some(), "existing files are kept");
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn pane_header_enlarges_restores_and_closes_panes(cx: &mut gpui::TestAppContext) {
    let (root, snapshot, _, window) = open_recording_workspace(cx, "pane-header");
    let panes: Vec<Uuid> = snapshot
        .selected_tab()
        .unwrap()
        .sessions
        .iter()
        .map(|session| session.id)
        .collect();
    assert_eq!(panes.len(), 2);
    window
        .update(cx, |view, window, cx| {
            view.toggle_pane_zoom_for(panes[0], window, cx);
            let tab = view.snapshot.selected_tab().unwrap();
            assert_eq!(tab.zoomed_session_id, Some(panes[0]));
            assert!(view.visible_terminal_ids(cx).contains(&panes[0]));
            assert!(!view.visible_terminal_ids(cx).contains(&panes[1]));

            view.toggle_pane_zoom_for(panes[0], window, cx);
            assert_eq!(
                view.snapshot.selected_tab().unwrap().zoomed_session_id,
                None
            );

            view.close_pane(panes[1], window, cx);
            let tab = view.snapshot.selected_tab().unwrap();
            assert_eq!(tab.sessions.len(), 1);
            assert_eq!(tab.sessions[0].id, panes[0]);
            assert!(!view.terminals.contains_key(&panes[1]));
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn review_tab_survives_panel_and_section_changes(cx: &mut gpui::TestAppContext) {
    let (root, _, _, window) = open_recording_workspace(cx, "review-tab");
    window
        .update(cx, |view, _, cx| {
            view.diff_view
                .update(cx, |diff, cx| diff.set_review_expanded(true, cx));
        })
        .unwrap();
    window
        .update(cx, |view, window, cx| {
            assert!(
                view.review_covers_terminal(cx),
                "reviews open as a full tab"
            );
            view.set_workspace_mode(RightSidebarMode::Files, cx);
            assert!(view.review_visible(cx));
            view.select_section(WorkspaceSection::Inbox, window, cx);
            assert!(view.diff_view.read(cx).review_expanded());
            assert!(!view.review_visible(cx));
            view.select_section(WorkspaceSection::Workspace, window, cx);
            assert!(view.review_visible(cx));
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
