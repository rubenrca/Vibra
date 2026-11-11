use super::*;
use crate::infrastructure::automation::AgentRuntimeState;
use crate::ports::terminal::{TerminalAgentKindSource, TerminalAgentStateSource};
use crate::ui::terminal::TerminalViewEvent;
use crate::{CloseTerminal, NextProject, ShowSettings, ToggleRightSidebar};
use gpui::Focusable;
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
    snapshot.open_tab_in_project(snapshot.selected_project_id.unwrap(), true);
    let first_project = snapshot.selected_project_id.unwrap();
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
            // Earlier sessions of a project load as tabs of the selected one.
            let merged = view.snapshot.selected_workspace().unwrap().clone();
            assert_eq!(merged.id, second_workspace);
            assert_eq!(merged.tabs.len(), 3, "no terminal stays out of reach");
            assert_eq!(
                view.snapshot
                    .selected_project()
                    .unwrap()
                    .workspaces
                    .as_ref()
                    .unwrap()
                    .len(),
                1
            );
            let other_session = view.snapshot.selected_session().unwrap().id;
            let first_tab = merged.tabs[1].id;
            let second_tab = merged.tabs[2].id;
            let first_session = merged.tabs[1].sessions[0].id;
            let second_session = merged.tabs[2].sessions[0].id;
            assert!(view.terminals[&other_session].read(cx).is_surface_visible());
            assert!(!view.terminals[&first_session].read(cx).is_surface_visible());
            assert!(
                !view.terminals[&second_session]
                    .read(cx)
                    .is_surface_visible()
            );
            assert_eq!(
                view.repository.load().unwrap().unwrap(),
                view.snapshot,
                "the merge is saved"
            );

            view.select_tab(first_tab, window, cx);
            assert!(
                !view.terminals[&other_session].read(cx).is_surface_visible(),
                "the unselected tab must stop cwd/agent polls"
            );
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
            for section in [WorkspaceSection::Inbox, WorkspaceSection::Settings] {
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
            view.select_section(WorkspaceSection::Inbox, window, cx);
            view.set_workspace_mode(RightSidebarMode::Files, cx);
            assert_eq!(view.workspace_section, WorkspaceSection::Workspace);
            assert_eq!(view.right_sidebar_mode, RightSidebarMode::Files);
            assert!(view.settings.right_sidebar_visible);
            assert!(
                view.terminals[&second_session]
                    .read(cx)
                    .is_surface_visible()
            );
            assert_eq!(view.snapshot, retained_snapshot);

            // Repository keeps the right sidebar as it was, and its shortcut
            // hides and shows it there without leaving Repository.
            view.select_section(WorkspaceSection::Inbox, window, cx);
            assert!(view.settings.right_sidebar_visible);
            view.toggle_right_sidebar(&ToggleRightSidebar, window, cx);
            assert_eq!(view.workspace_section, WorkspaceSection::Inbox);
            assert!(!view.settings.right_sidebar_visible);
            view.toggle_right_sidebar(&ToggleRightSidebar, window, cx);
            assert_eq!(view.workspace_section, WorkspaceSection::Inbox);
            assert!(view.settings.right_sidebar_visible);

            // From Settings, the shortcut returns keyboard focus to the
            // terminal, even if tools were open.
            view.select_section(WorkspaceSection::Settings, window, cx);
            view.toggle_right_sidebar(&ToggleRightSidebar, window, cx);
            assert_eq!(view.workspace_section, WorkspaceSection::Workspace);
            assert!(view.settings.right_sidebar_visible);
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
                    .close_workspace(first_project, second_workspace)
            );
            view.reconcile_terminal_views(cx);
            view.show_terminal_tab(window, cx);
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
            view.show_terminal_tab(window, cx);
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
    snapshot.open_tab_in_project(snapshot.selected_project_id.unwrap(), true);
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
            assert!(!view.settings.right_sidebar_visible);
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

pub(super) fn open_recording_workspace(
    cx: &mut gpui::TestAppContext,
    name: &str,
) -> RecordingWorkspace {
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
fn focusing_another_pane_swaps_the_front_of_its_tab(cx: &mut gpui::TestAppContext) {
    let (root, snapshot, _, window) = open_recording_workspace(cx, "tab-front");
    let tab = snapshot.selected_tab().unwrap();
    let order: Vec<Uuid> = tab.sessions.iter().map(|session| session.id).collect();
    // The window's first frame already recorded the selected pane in front.
    let first = tab.selected_session_id.unwrap();
    let second = *order.iter().find(|id| **id != first).unwrap();
    let later = |from: Uuid, to: Uuid| {
        let position = |id| order.iter().position(|other| *other == id).unwrap();
        if position(to) > position(from) {
            1.0
        } else {
            -1.0
        }
    };
    let tab_id = tab.id;
    window
        .update(cx, |view, window, _| {
            let stack = |front: Uuid, back: Uuid| vec![(front, None), (back, None)];
            // Frames that keep the same pane in front only record it.
            assert!(
                view.tab_front_motion(tab_id, first, &stack(first, second), "one", &order)
                    .is_none()
            );
            // Another pane takes focus: the title rolls toward it from the old one.
            let swap = view
                .tab_front_motion(tab_id, second, &stack(second, first), "two", &order)
                .unwrap();
            assert_eq!(swap.direction, later(first, second));
            assert_eq!(swap.from_title, "one");
            assert_eq!(swap.from_stack[0].0, first);
            // It keeps running across frames…
            let running = view
                .tab_front_motion(tab_id, second, &stack(second, first), "two", &order)
                .unwrap();
            assert_eq!(running.id, swap.id);
            // …and going back is a new swap that rolls the other way.
            let back = view
                .tab_front_motion(tab_id, first, &stack(first, second), "one", &order)
                .unwrap();
            assert_eq!(back.direction, -swap.direction);
            assert_ne!(back.id, swap.id);
            assert_eq!(back.from_title, "two");
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
fn panes_enlarge_restore_and_close(cx: &mut gpui::TestAppContext) {
    let (root, snapshot, _, window) = open_recording_workspace(cx, "pane-zoom-close");
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

#[gpui::test]
fn review_tab_closes_from_its_context_menu(cx: &mut gpui::TestAppContext) {
    let (root, _, _, window) = open_recording_workspace(cx, "review-tab-menu");
    window
        .update(cx, |view, _, cx| {
            view.diff_view
                .update(cx, |diff, cx| diff.set_review_expanded(true, cx));
        })
        .unwrap();
    window
        .update(cx, |view, window, cx| {
            assert!(view.review_visible(cx));
            view.open_context_menu(ContextMenuKind::Review, 10.0, 10.0, cx);
            view.run_context_menu_action(ContextMenuAction::CloseReview, window, cx);
            assert!(view.context_menu.is_none());
            assert!(!view.review_visible(cx));
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn new_tabs_and_pane_commands_keep_the_review_and_show_the_terminal(cx: &mut gpui::TestAppContext) {
    let (root, snapshot, _, window) = open_recording_workspace(cx, "tab-hygiene");
    let project = snapshot.selected_project_id.unwrap();
    window
        .update(cx, |view, _, cx| {
            view.set_right_sidebar_visible(false, false, cx);
            view.diff_view
                .update(cx, |diff, cx| diff.set_review_expanded(true, cx));
        })
        .unwrap();
    window
        .update(cx, |view, window, cx| {
            assert!(view.review_covers_terminal(cx));

            // Pane commands act on the visible terminal.
            view.cycle_pane(1, window, cx);
            assert!(!view.review_visible(cx));
            assert!(view.diff_view.read(cx).review_expanded());
            view.activate_review_tab(window, cx);

            // ⌘T (and ⌘N) add a tab to the project; the review stays open.
            let tabs = view.snapshot.selected_workspace().unwrap().tabs.len();
            view.new_terminal_tab(&crate::NewTerminalTab, window, cx);
            let workspace = view.snapshot.selected_workspace().unwrap();
            assert_eq!(workspace.tabs.len(), tabs + 1);
            assert_eq!(
                view.snapshot
                    .selected_project()
                    .unwrap()
                    .workspaces
                    .as_ref()
                    .unwrap()
                    .len(),
                1,
                "new tabs never create hidden sessions"
            );
            assert!(view.diff_view.read(cx).review_expanded());
            assert!(!view.review_visible(cx));

            // Choosing the project keeps the right panel as the user left it.
            view.select_project(project, window, cx);
            assert!(!view.settings.right_sidebar_visible);
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn dragging_tabs_and_panes_preserves_live_terminal_entities(cx: &mut gpui::TestAppContext) {
    use crate::domain::workspace::WorkspaceTabId::Terminal;
    let (root, _, _, window) = open_recording_workspace(cx, "tab-pane-moves");
    window
        .update(cx, |view, window, cx| {
            let original_tab = view.snapshot.selected_tab().unwrap().id;
            let pane = view.snapshot.selected_session().unwrap().id;
            let live_entities: HashMap<_, _> = view
                .terminals
                .iter()
                .map(|(id, terminal)| (*id, terminal.entity_id()))
                .collect();
            view.detach_pane(pane, Some(Terminal(original_tab)), window, cx);
            let detached_tab = view.snapshot.selected_tab().unwrap().id;
            assert_ne!(detached_tab, original_tab);
            assert_eq!(
                view.snapshot.selected_workspace().unwrap().tabs[0].id,
                detached_tab
            );
            assert_eq!(view.snapshot.selected_session().unwrap().id, pane);
            assert_eq!(view.snapshot.painted_session_ids(), HashSet::from([pane]));
            for (id, terminal) in &view.terminals {
                assert_eq!(terminal.entity_id(), live_entities[id]);
                assert_eq!(terminal.read(cx).is_surface_visible(), *id == pane);
            }
            view.dock_tab(Terminal(detached_tab), Terminal(original_tab), window, cx);
            assert_eq!(view.snapshot.selected_workspace().unwrap().tabs.len(), 1);
            assert_eq!(view.snapshot.selected_tab().unwrap().id, original_tab);
            assert_eq!(view.snapshot.selected_session().unwrap().id, pane);
            assert_eq!(view.terminals.len(), live_entities.len());
            for (id, terminal) in &view.terminals {
                assert_eq!(terminal.entity_id(), live_entities[id]);
                assert!(terminal.read(cx).is_surface_visible());
            }
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn review_drag_order_shortcuts_and_detaching_from_a_split_agree(cx: &mut gpui::TestAppContext) {
    use crate::domain::workspace::WorkspaceTabId::{Review, Terminal};
    let (root, _, _, window) = open_recording_workspace(cx, "review-drag");
    window
        .update(cx, |view, _, cx| {
            view.diff_view
                .update(cx, |diff, cx| diff.set_review_expanded(true, cx));
        })
        .unwrap();
    window
        .update(cx, |view, window, cx| {
            let tab = view.snapshot.selected_tab().unwrap().id;
            let mut drag = TabDrag {
                tab_id: Review,
                title: "Working tree".into(),
                from_pane: false,
            };
            view.drop_tab_in_strip(&drag, Some(Terminal(tab)), window, cx);
            assert_eq!(view.visible_tab_order(cx), vec![Review, Terminal(tab)]);
            view.go_to_tab(&crate::GoToTab { index: 2 }, window, cx);
            assert!(!view.review_visible(cx));
            view.go_to_tab(&crate::GoToTab { index: 1 }, window, cx);
            assert!(view.review_covers_terminal(cx));
            view.dock_tab(Review, Terminal(tab), window, cx);
            assert_eq!(
                view.visible_tab_order(cx),
                vec![Terminal(tab)],
                "a docked review is a pane, not a second tab"
            );
            assert_eq!(view.review_dock_owner(), Some(tab));
            assert!(view.review_visible(cx));
            assert!(!view.review_covers_terminal(cx));
            assert_eq!(view.review_split_direction, PaneSplitDirection::Right);
            let pane = view.snapshot.selected_session().unwrap().id;
            view.dock_tab_at_pane(Review, pane, PaneSplitDirection::Up, window, cx);
            assert_eq!(view.review_split_direction, PaneSplitDirection::Up);
            for terminal in view.terminals.values() {
                assert!(terminal.read(cx).is_surface_visible());
            }
            drag.from_pane = true;
            view.drop_tab_in_strip(&drag, None, window, cx);
            assert_eq!(view.review_dock_owner(), None);
            assert_eq!(view.visible_tab_order(cx), vec![Terminal(tab), Review]);
            assert!(view.review_covers_terminal(cx));
            for terminal in view.terminals.values() {
                assert!(!terminal.read(cx).is_surface_visible());
            }
            view.go_to_tab(&crate::GoToTab { index: 1 }, window, cx);
            assert!(!view.review_visible(cx));
            view.go_to_tab(&crate::GoToTab { index: 9 }, window, cx);
            assert!(view.review_covers_terminal(cx));
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn docked_review_follows_its_owner_when_switching_merging_and_detaching_panes(
    cx: &mut gpui::TestAppContext,
) {
    use crate::domain::workspace::WorkspaceTabId::{Review, Terminal};
    let (root, _, _, window) = open_recording_workspace(cx, "review-owner");
    window
        .update(cx, |view, _, cx| {
            view.diff_view
                .update(cx, |diff, cx| diff.set_review_expanded(true, cx));
        })
        .unwrap();
    window
        .update(cx, |view, window, cx| {
            let owner = view.snapshot.selected_tab().unwrap().id;
            let (other, _) = view
                .snapshot
                .open_tab_in_project(view.snapshot.selected_project_id.unwrap(), false)
                .unwrap();
            view.reconcile_terminal_views(cx);
            view.dock_tab(Terminal(owner), Review, window, cx);
            assert_eq!(
                view.visible_tab_order(cx),
                vec![Terminal(owner), Terminal(other)]
            );
            view.go_to_tab(&crate::GoToTab { index: 9 }, window, cx);
            assert_eq!(view.snapshot.selected_tab().unwrap().id, other);
            assert!(!view.review_visible(cx));
            assert!(!view.visible_tab_order(cx).contains(&Review));
            view.go_to_tab(&crate::GoToTab { index: 1 }, window, cx);
            assert!(
                view.review_visible(cx),
                "returning to the owner restores its split"
            );
            view.dock_tab(Terminal(owner), Terminal(other), window, cx);
            assert_eq!(view.review_dock_owner(), Some(other));
            assert!(view.review_visible(cx));
            assert_eq!(view.visible_tab_order(cx), vec![Terminal(other)]);
            let pane = view.snapshot.selected_session().unwrap().id;
            view.detach_pane(pane, None, window, cx);
            assert_eq!(
                view.review_dock_owner(),
                Some(other),
                "detaching one of several terminals keeps review with the original tab"
            );
            assert!(!view.review_visible(cx));
            view.select_tab(other, window, cx);
            assert!(view.review_visible(cx));
            let remaining = view.snapshot.selected_tab().unwrap().layout.terminal_ids();
            for session in remaining {
                view.close_pane(session, window, cx);
            }
            assert_eq!(view.review_dock_owner(), None);
            assert!(
                view.visible_tab_order(cx).contains(&Review),
                "closing the owner does not strand the review"
            );
            assert!(view.diff_view.read(cx).review_focused());
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn explorer_opens_unchanged_documents_without_replacing_terminals(cx: &mut gpui::TestAppContext) {
    let (root, _, _, window) = open_recording_workspace(cx, "explorer-documents");
    let path = root.join("README.md");
    std::fs::write(&path, "# Archivo sin Git\n").unwrap();
    window
        .update(cx, |view, window, cx| {
            let terminals: HashMap<_, _> = view
                .terminals
                .iter()
                .map(|(id, terminal)| (*id, terminal.entity_id()))
                .collect();
            view.open_project_file(path.clone(), window, cx);
            assert_eq!(view.selected_file_path, Some(path.clone()));
            assert_eq!(view.diff_view.read(cx).preview_path(), Some(path.as_path()));
            assert!(view.review_covers_terminal(cx));
            assert!(
                view.terminals
                    .values()
                    .all(|terminal| !terminal.read(cx).is_surface_visible())
            );
            view.close_review(window, cx);
            for (id, terminal) in &view.terminals {
                assert_eq!(terminal.entity_id(), terminals[id]);
                assert!(terminal.read(cx).is_surface_visible());
            }
            view.execute_palette_action(PaletteAction::OpenFile(path.clone()), window, cx);
            assert_eq!(view.diff_view.read(cx).preview_path(), Some(path.as_path()));
            let tab = view.snapshot.selected_tab().unwrap().id;
            view.close_tab(tab, window, cx);
            assert!(view.terminals.is_empty());
            assert!(view.review_covers_terminal(cx));
            assert_eq!(
                view.visible_tab_order(cx),
                vec![crate::domain::workspace::WorkspaceTabId::Review],
                "closing a background terminal leaves the document accessible"
            );
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn changing_project_hides_old_branch_and_ignores_late_results(cx: &mut gpui::TestAppContext) {
    use crate::ports::git::GitBranchSummary;
    let (root, _, _, window) = open_recording_workspace(cx, "status-root");
    let summary = |branch: &str| GitBranchSummary {
        branch: branch.into(),
        ahead: 0,
        behind: 0,
        dirty: false,
    };
    window
        .update(cx, |view, window, cx| {
            view.apply_branch_summary(root.clone(), Some(summary("first")), cx);
            assert_eq!(view.current_branch_summary().unwrap().branch, "first");
            let next_root = root.join("second");
            let next = view.snapshot.add_project(&next_root);
            view.select_project(next, window, cx);
            assert!(view.current_branch_summary().is_none());
            view.apply_branch_summary(next_root.clone(), Some(summary("second")), cx);
            view.apply_branch_summary(root.clone(), None, cx);
            view.apply_branch_summary(root.clone(), Some(summary("late-first")), cx);
            assert_eq!(view.current_branch_summary().unwrap().branch, "second");
            view.apply_branch_summary(next_root, None, cx);
            assert!(view.current_branch_summary().is_none());
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn project_diff_counts_follow_folders_and_ignore_removed_projects(cx: &mut gpui::TestAppContext) {
    use super::projects::ProjectDiffStats;
    let (root, snapshot, _, window) = open_recording_workspace(cx, "project-diff-counts");
    let first = snapshot.selected_project_id.unwrap();
    let first_stats = ProjectDiffStats {
        additions: 12,
        deletions: 3,
    };
    let second_stats = ProjectDiffStats {
        additions: 4,
        deletions: 8,
    };
    window
        .update(cx, |view, window, cx| {
            let second_root = root.join("second");
            let second = view.snapshot.add_project(&second_root);
            view.apply_project_diff_stats(root.clone(), Some(first_stats), cx);
            view.apply_project_diff_stats(second_root.clone(), Some(second_stats), cx);
            assert_eq!(view.project_diff_stats(first), Some(first_stats));
            assert_eq!(view.project_diff_stats(second), Some(second_stats));

            let replacement_root = root.join("replacement");
            view.snapshot
                .set_project_directory(first, &replacement_root);
            view.apply_project_diff_stats(root.clone(), Some(second_stats), cx);
            assert_eq!(view.project_diff_stats(first), None);
            view.apply_project_diff_stats(replacement_root.clone(), Some(first_stats), cx);
            assert_eq!(view.project_diff_stats(first), Some(first_stats));
            // A failed refresh or a folder that is no longer a repo clears its count.
            view.apply_project_diff_stats(replacement_root, None, cx);
            assert_eq!(view.project_diff_stats(first), None);
            assert_eq!(view.project_diff_stats(second), Some(second_stats));

            view.snapshot.remove_project(second);
            view.apply_project_diff_stats(second_root, Some(first_stats), cx);
            assert_eq!(view.project_diff_stats(second), None);
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn repository_opens_as_a_tab_of_its_space_beside_the_terminals(cx: &mut gpui::TestAppContext) {
    let (root, snapshot, _, window) = open_recording_workspace(cx, "repository-tab");
    let space = snapshot.selected_project_id.unwrap();
    let tab = snapshot.selected_tab().unwrap().id;
    window
        .update(cx, |view, window, cx| {
            assert!(!view.tool_tab_open(ToolTab::Repository));
            view.select_section(WorkspaceSection::Inbox, window, cx);
            assert!(
                view.tool_tab_open(ToolTab::Repository),
                "Repository opens as a tab"
            );

            // A terminal tab comes forward; Repository keeps its tab.
            view.select_tab(tab, window, cx);
            assert_eq!(view.workspace_section, WorkspaceSection::Workspace);
            assert!(view.tool_tab_open(ToolTab::Repository));

            // The tab is the space's: another space has none until it opens one.
            let other = view.snapshot.add_project(&root.join("other"));
            view.snapshot.open_tab_in_project(other, true);
            view.reconcile_terminal_views(cx);
            view.select_project(other, window, cx);
            assert!(!view.tool_tab_open(ToolTab::Repository));
            view.select_project(space, window, cx);
            assert!(
                view.tool_tab_open(ToolTab::Repository),
                "it waits in its own space"
            );

            // Closing it from the strip while a terminal is in front stays there.
            view.open_context_menu(ContextMenuKind::Tool(ToolTab::Repository), 10.0, 10.0, cx);
            view.run_context_menu_action(
                ContextMenuAction::CloseTool(ToolTab::Repository),
                window,
                cx,
            );
            assert!(!view.tool_tab_open(ToolTab::Repository));
            assert_eq!(view.workspace_section, WorkspaceSection::Workspace);

            // ⌘W in front of Repository closes its tab and shows the terminal.
            view.select_section(WorkspaceSection::Inbox, window, cx);
            view.close_terminal(&CloseTerminal, window, cx);
            assert!(!view.tool_tab_open(ToolTab::Repository));
            assert_eq!(view.workspace_section, WorkspaceSection::Workspace);
            assert_eq!(view.snapshot.selected_tab().unwrap().id, tab);

            // Numbered shortcuts count Repository's tab after the terminals.
            view.select_section(WorkspaceSection::Inbox, window, cx);
            view.go_to_tab(&crate::GoToTab { index: 1 }, window, cx);
            assert_eq!(view.workspace_section, WorkspaceSection::Workspace);
            view.go_to_tab(&crate::GoToTab { index: 2 }, window, cx);
            assert_eq!(view.workspace_section, WorkspaceSection::Inbox);
            view.go_to_tab(&crate::GoToTab { index: 1 }, window, cx);
            view.go_to_tab(&crate::GoToTab { index: 9 }, window, cx);
            assert_eq!(
                view.workspace_section,
                WorkspaceSection::Inbox,
                "⌘9 is the last tab"
            );

            // A space whose only tab is Repository shows it, not the empty space.
            let bare = view.snapshot.add_project(&root.join("bare"));
            view.select_project(bare, window, cx);
            assert!(view.snapshot.selected_tab().is_none());
            view.select_section(WorkspaceSection::Inbox, window, cx);
            view.select_section(WorkspaceSection::Workspace, window, cx);
            view.settle_space_front(window, cx);
            assert_eq!(view.workspace_section, WorkspaceSection::Inbox);
            view.close_tool_tab(ToolTab::Repository, window, cx);
            view.settle_space_front(window, cx);
            assert_eq!(
                view.workspace_section,
                WorkspaceSection::Workspace,
                "with no tabs at all, the empty space"
            );
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn activity_opens_as_a_tab_after_repository(cx: &mut gpui::TestAppContext) {
    let (root, snapshot, _, window) = open_recording_workspace(cx, "activity-tab");
    let tab = snapshot.selected_tab().unwrap().id;
    window
        .update(cx, |view, window, cx| {
            view.select_section(WorkspaceSection::Activity, window, cx);
            assert!(
                view.tool_tab_open(ToolTab::Activity),
                "Activity opens as a tab"
            );
            assert!(!view.tool_tab_open(ToolTab::Repository));
            view.activity_content(cx);

            // Both tools sit after the terminals, Repository first, whatever
            // order they were opened in.
            view.select_section(WorkspaceSection::Inbox, window, cx);
            assert_eq!(
                view.open_tool_tabs(),
                vec![ToolTab::Repository, ToolTab::Activity]
            );
            view.go_to_tab(&crate::GoToTab { index: 9 }, window, cx);
            assert_eq!(view.workspace_section, WorkspaceSection::Activity);
            view.go_to_tab(&crate::GoToTab { index: 2 }, window, cx);
            assert_eq!(view.workspace_section, WorkspaceSection::Inbox);

            // ⌘W closes the tool in front, never the other one.
            view.go_to_tab(&crate::GoToTab { index: 3 }, window, cx);
            view.close_terminal(&CloseTerminal, window, cx);
            assert!(!view.tool_tab_open(ToolTab::Activity));
            assert!(view.tool_tab_open(ToolTab::Repository));
            assert_eq!(view.workspace_section, WorkspaceSection::Workspace);
            assert_eq!(view.snapshot.selected_tab().unwrap().id, tab);

            // Its menu closes it too.
            view.select_section(WorkspaceSection::Activity, window, cx);
            view.open_context_menu(ContextMenuKind::Tool(ToolTab::Activity), 10.0, 10.0, cx);
            view.run_context_menu_action(
                ContextMenuAction::CloseTool(ToolTab::Activity),
                window,
                cx,
            );
            assert!(!view.tool_tab_open(ToolTab::Activity));
            assert_eq!(view.workspace_section, WorkspaceSection::Workspace);
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn snake_hides_in_the_field_of_an_empty_space_and_pauses_out_of_sight(
    cx: &mut gpui::TestAppContext,
) {
    use crate::ui::retro::snake::SnakePhase;

    let (root, _, _, window) = open_recording_workspace(cx, "snake");
    window
        .update(cx, |view, window, cx| {
            view.window_is_active = true;
            view.arcade_bounds.set(gpui::Bounds::new(
                gpui::point(gpui::px(0.0), gpui::px(0.0)),
                gpui::size(gpui::px(800.0), gpui::px(280.0)),
            ));
            assert!(!view.arcade_shown(cx), "terminals cover the field");

            let tabs: Vec<Uuid> = view
                .snapshot
                .selected_workspace()
                .unwrap()
                .tabs
                .iter()
                .map(|tab| tab.id)
                .collect();
            for tab in tabs {
                view.close_tab(tab, window, cx);
            }
            assert!(view.arcade_shown(cx));
            assert_eq!(view.snake.phase(), SnakePhase::Title);
            view.sync_arcade_clock(cx);
            assert!(
                view._arcade_step_task.is_none(),
                "the resting field takes no steps"
            );
            assert!(view._arcade_rest_task.is_some(), "but it twinkles");

            // The resting field is a picture: keys leave it alone.
            assert!(!view.arcade_key("w", cx));
            assert!(!view.arcade_key("space", cx));

            // The faint snake that plays by itself becomes the player's.
            for _ in 0..12 {
                view.attract.step(57, 20);
            }
            let resting: Vec<_> = view.attract.body().collect();
            assert!(!resting.is_empty());
            view.start_snake();
            assert_eq!(view.snake.phase(), SnakePhase::Playing);
            assert_eq!(view.snake.grid(), (57, 20), "the board fills the field");
            assert_eq!(view.snake.body().collect::<Vec<_>>(), resting);

            // It waits for the field to finish opening before it moves.
            view.sync_arcade_clock(cx);
            assert_eq!(view.arcade_stage_target, 1.0);
            assert!(view._arcade_stage_task.is_some(), "the field opens");
            assert!(view._arcade_step_task.is_none());
            view.arcade_stage = 1.0;
            view.sync_arcade_clock(cx);
            assert!(view._arcade_step_task.is_some(), "a game steps");
            assert!(
                view._arcade_rest_task.is_none(),
                "the board does not twinkle"
            );

            // A background window pauses the game and stops its steps.
            view.window_is_active = false;
            view.sync_arcade_clock(cx);
            assert_eq!(view.snake.phase(), SnakePhase::Paused);
            assert!(view._arcade_step_task.is_none());
            assert!(view._arcade_rest_task.is_none());

            view.window_is_active = true;
            let playing: Vec<_> = view.snake.body().collect();
            assert!(view.arcade_key("escape", cx), "escape leaves a paused game");
            assert_eq!(view.snake.phase(), SnakePhase::Title);
            assert_eq!(
                view.attract.body().collect::<Vec<_>>(),
                playing,
                "the field takes the snake back where it was"
            );
            view.sync_arcade_clock(cx);
            assert_eq!(view.arcade_stage_target, 0.0, "the field closes");
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn snake_keys_never_reach_the_game_while_something_else_holds_the_keyboard(
    cx: &mut gpui::TestAppContext,
) {
    use crate::ui::retro::snake::SnakePhase;

    let (root, _, _, window) = open_recording_workspace(cx, "snake-focus");
    let mut cx = gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(&mut cx, |view, window, cx| {
            let tabs: Vec<Uuid> = view
                .snapshot
                .selected_workspace()
                .unwrap()
                .tabs
                .iter()
                .map(|tab| tab.id)
                .collect();
            for tab in tabs {
                view.close_tab(tab, window, cx);
            }
            view.focus_handle.focus(window);
            view.start_snake();
            view.snake.pause();
        })
        .unwrap();
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    window
        .update(&mut cx, |view, window, _| {
            assert_eq!(
                view.snake.phase(),
                SnakePhase::Title,
                "escape leaves the game"
            );
            view.start_snake();
            view.snake.pause();
            // With something else holding the keyboard, the game hears nothing.
            window.blur();
        })
        .unwrap();
    cx.simulate_keystrokes("escape");
    window
        .update(&mut cx, |view, window, _| {
            assert_eq!(view.snake.phase(), SnakePhase::Paused);
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn space_ripples_follow_their_agents_and_tick_only_while_moving(cx: &mut gpui::TestAppContext) {
    use crate::ui::space_marks::SpaceActivity;

    let (root, snapshot, _, window) = open_recording_workspace(cx, "space-ripples");
    let project = snapshot.selected_project_id.unwrap();
    let pane = snapshot.selected_session().unwrap().id;
    let presence = |state| TerminalViewEvent::AgentPresenceChanged {
        session_id: pane,
        presence: Some(TerminalAgentPresence {
            kind: "Claude".into(),
            kind_source: TerminalAgentKindSource::Process,
            state,
            state_source: TerminalAgentStateSource::Heuristic,
            attention: None,
            process_id: Some(7),
        }),
    };
    window
        .update(cx, |view, window, cx| {
            view.left_sidebar_progress = 1.0;
            assert_eq!(view.space_activity(project), SpaceActivity::Quiet);
            view.sync_ripple_clock(cx);
            assert!(
                view._ripple_clock_task.is_none(),
                "a quiet space never ticks"
            );

            view.handle_terminal_view_event(&presence(AgentRuntimeState::Working), cx);
            assert_eq!(view.space_activity(project), SpaceActivity::Working);
            view.sync_ripple_clock(cx);
            assert!(view._ripple_clock_task.is_some());

            // Nothing to see with the sidebar hidden.
            view.left_sidebar_progress = 0.0;
            view.sync_ripple_clock(cx);
            assert!(view._ripple_clock_task.is_none());
            view.left_sidebar_progress = 1.0;

            view.handle_terminal_view_event(&presence(AgentRuntimeState::Waiting), cx);
            assert_eq!(view.space_activity(project), SpaceActivity::NeedsYou);
            view.handle_terminal_view_event(&presence(AgentRuntimeState::Idle), cx);
            assert_eq!(view.space_activity(project), SpaceActivity::Idle);
            view.sync_ripple_clock(cx);
            assert!(view._ripple_clock_task.is_none(), "idle rings hold still");
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn sidebar_agents_list_every_space_grouped_by_what_they_need(cx: &mut gpui::TestAppContext) {
    use super::spaces::AgentGroup;

    let (root, snapshot, _, window) = open_recording_workspace(cx, "project-agents");
    let first_project = snapshot.selected_project_id.unwrap();
    let first_pane = snapshot.selected_session().unwrap().id;
    let listed = |view: &WorkspaceView, cx: &Context<WorkspaceView>| {
        view.sidebar_agents(cx)
            .into_iter()
            .map(|(group, agents)| {
                let panes = agents.iter().map(|agent| agent.session_id).collect();
                (group, panes)
            })
            .collect::<Vec<(AgentGroup, Vec<Uuid>)>>()
    };
    window
        .update(cx, |view, window, cx| {
            // A saved title alone must never turn a shell into an agent row.
            view.snapshot
                .update_agent_task_title(first_pane, "Review sidebar");
            assert!(view.sidebar_agents(cx).is_empty());
            view.snapshot.open_tab_in_project(first_project, true);
            let second_pane = view.snapshot.selected_session().unwrap().id;
            let second_project = view.snapshot.add_project(&root.join("second"));
            view.snapshot.open_tab_in_project(second_project, true);
            let third_pane = view.snapshot.selected_session().unwrap().id;
            view.reconcile_terminal_views(cx);
            view.show_terminal_tab(window, cx);

            for (pane, kind, state) in [
                (first_pane, "Codex", AgentRuntimeState::Working),
                (second_pane, "Claude", AgentRuntimeState::Waiting),
                (third_pane, "Gemini", AgentRuntimeState::Idle),
            ] {
                view.handle_terminal_view_event(
                    &TerminalViewEvent::AgentPresenceChanged {
                        session_id: pane,
                        presence: Some(TerminalAgentPresence {
                            kind: kind.into(),
                            kind_source: TerminalAgentKindSource::Process,
                            state,
                            state_source: TerminalAgentStateSource::Heuristic,
                            attention: None,
                            process_id: Some(7),
                        }),
                    },
                    cx,
                );
            }
            assert_eq!(
                listed(view, cx),
                vec![
                    (AgentGroup::NeedsYou, vec![second_pane]),
                    (AgentGroup::Working, vec![first_pane]),
                    (AgentGroup::Idle, vec![third_pane]),
                ]
            );
            let groups = view.sidebar_agents(cx);
            let working = &groups[1].1[0];
            assert_eq!(working.identity.title, "Review sidebar");
            assert_eq!(
                working.space,
                view.snapshot.project(first_project).unwrap().name
            );
            assert!(working.since.is_some());
            assert_eq!(
                groups[2].1[0].space,
                view.snapshot.project(second_project).unwrap().name
            );

            // The row uses the existing global navigation, including hidden panes.
            view.open_pane(first_pane, window, cx);
            assert_eq!(view.snapshot.selected_project_id, Some(first_project));
            assert_eq!(view.snapshot.selected_session().unwrap().id, first_pane);
            assert!(view.visible_terminal_ids(cx).contains(&first_pane));
            assert!(
                view.terminals[&first_pane]
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
            );

            view.handle_terminal_view_event(
                &TerminalViewEvent::AgentPresenceChanged {
                    session_id: first_pane,
                    presence: None,
                },
                cx,
            );
            assert!(!view.agent_state_since.contains_key(&first_pane));
            assert_eq!(
                listed(view, cx),
                vec![
                    (AgentGroup::NeedsYou, vec![second_pane]),
                    (AgentGroup::Idle, vec![third_pane]),
                ]
            );
            view.handle_terminal_view_event(
                &TerminalViewEvent::Exited {
                    session_id: second_pane,
                    code: Some(0),
                },
                cx,
            );
            assert_eq!(listed(view, cx), vec![(AgentGroup::Idle, vec![third_pane])]);
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn spaces_follow_their_terminal_into_repositories_and_close_with_it(cx: &mut gpui::TestAppContext) {
    let (root, snapshot, _, window) = open_recording_workspace(cx, "spaces");
    let space = snapshot.selected_project_id.unwrap();
    let pane = snapshot.selected_session().unwrap().id;
    let repository = root.join("Dev/app");
    std::fs::create_dir_all(repository.join("src")).unwrap();
    std::fs::create_dir_all(repository.join(".git")).unwrap();
    let cd = |view: &mut WorkspaceView, path: PathBuf, cx: &mut Context<WorkspaceView>| {
        view.handle_terminal_view_event(
            &TerminalViewEvent::WorkingDirectoryChanged {
                session_id: pane,
                path,
            },
            cx,
        );
    };
    window
        .update(cx, |view, window, cx| {
            // The test folder stands in for the home folder.
            view.home_directory = Some(root.clone());

            // Inside a repository the space takes the repository, not the subfolder.
            cd(view, repository.join("src"), cx);
            let project = view.snapshot.project(space).unwrap();
            assert_eq!(project.name, "app");
            assert_eq!(project.directory(), repository.to_str());
            assert_eq!(view.project_root(), repository);

            cd(view, root.clone(), cx);
            assert_eq!(view.snapshot.project(space).unwrap().name, "~");
            assert_eq!(view.project_root(), root);

            // A name the user chose stays while the space moves.
            assert!(view.snapshot.rename_project(space, "Work"));
            cd(view, repository.clone(), cx);
            assert_eq!(view.snapshot.project(space).unwrap().name, "Work");
            assert_eq!(view.project_root(), repository);

            // ⌘N opens another space in the home folder with its own terminal.
            view.new_space(window, cx);
            let second = view.snapshot.selected_project_id.unwrap();
            assert_ne!(second, space);
            let project = view.snapshot.project(second).unwrap();
            assert_eq!(project.name, "~");
            assert_eq!(project.directory(), root.to_str());
            let second_pane = view.snapshot.selected_session().unwrap();
            assert_eq!(Some(second_pane.working_directory.as_str()), root.to_str());
            let second_pane = second_pane.id;
            assert!(view.terminals.contains_key(&second_pane));

            // Its last terminal leaves it open, selected and pinned.
            view.settings.pinned_project_ids.push(second);
            view.close_pane(second_pane, window, cx);
            assert!(!view.terminals.contains_key(&second_pane));
            let project = view.snapshot.project(second).unwrap();
            assert_eq!(project.terminal_sessions().count(), 0);
            assert_eq!(project.directory(), root.to_str());
            assert_eq!(view.snapshot.selected_project_id, Some(second));
            assert_eq!(view.settings.pinned_project_ids, vec![second]);

            // Closing it without terminals asks nothing and unpins it.
            view.confirm_remove_project(second, window, cx);
            assert!(view.snapshot.project(second).is_none());
            assert!(view.settings.pinned_project_ids.is_empty());
            assert_eq!(view.snapshot.selected_project_id, Some(space));

            // ⌘T in a space whose folder is gone starts in the home folder.
            std::fs::remove_dir_all(&repository).unwrap();
            view.open_terminal_tab_in_project(window, cx);
            let session = view.snapshot.selected_session().unwrap();
            assert_eq!(Some(session.working_directory.as_str()), root.to_str());
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn inbox_reveals_a_pane_hidden_by_another_panes_zoom(cx: &mut gpui::TestAppContext) {
    let (root, snapshot, _, window) = open_recording_workspace(cx, "inbox-zoom");
    let panes: Vec<_> = snapshot
        .selected_tab()
        .unwrap()
        .sessions
        .iter()
        .map(|session| session.id)
        .collect();
    window
        .update(cx, |view, window, cx| {
            view.toggle_pane_zoom_for(panes[0], window, cx);
            view.select_section(WorkspaceSection::Inbox, window, cx);
            view.open_pane(panes[1], window, cx);
            assert_eq!(view.snapshot.selected_session().unwrap().id, panes[1]);
            assert!(view.visible_terminal_ids(cx).contains(&panes[1]));
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn pane_navigation_never_focuses_a_terminal_hidden_by_zoom(cx: &mut gpui::TestAppContext) {
    use crate::domain::workspace::PaneFocusDirection;

    let (root, snapshot, _, window) = open_recording_workspace(cx, "zoom-navigation");
    let panes: Vec<_> = snapshot
        .selected_tab()
        .unwrap()
        .sessions
        .iter()
        .map(|s| s.id)
        .collect();
    window
        .update(cx, |view, window, cx| {
            view.toggle_pane_zoom_for(panes[0], window, cx);
            view.focus_pane(PaneFocusDirection::Right, window, cx);
            assert_eq!(view.snapshot.selected_session().unwrap().id, panes[1]);
            assert!(view.terminals[&panes[1]].read(cx).is_surface_visible());
            assert!(
                view.terminals[&panes[1]]
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
            );

            view.toggle_pane_zoom_for(panes[1], window, cx);
            view.cycle_pane(1, window, cx);
            assert_eq!(view.snapshot.selected_session().unwrap().id, panes[0]);
            assert!(view.terminals[&panes[0]].read(cx).is_surface_visible());
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn project_shortcuts_follow_pins_and_reordering(cx: &mut gpui::TestAppContext) {
    let (root, snapshot, _, window) = open_recording_workspace(cx, "project-shortcuts");
    window
        .update(cx, |view, window, cx| {
            let first = snapshot.selected_project_id.unwrap();
            let mut projects = vec![first];
            for index in 2..=10 {
                projects.push(
                    view.snapshot
                        .add_project(&root.join(format!("project-{index}"))),
                );
            }
            view.settings.pinned_project_ids = vec![projects[8], projects[2]];
            for (number, expected) in [
                (1, projects[2]),
                (2, projects[8]),
                (3, first),
                (9, projects[9]),
            ] {
                view.go_to_project(&crate::GoToProject { index: number }, window, cx);
                assert_eq!(view.snapshot.selected_project_id, Some(expected));
            }

            view.snapshot.move_project(projects[9], Some(first));
            view.go_to_project(&crate::GoToProject { index: 3 }, window, cx);
            assert_eq!(view.snapshot.selected_project_id, Some(projects[9]));
            view.next_project(&NextProject, window, cx);
            assert_eq!(view.snapshot.selected_project_id, Some(first));

            view.open_palette(PaletteMode::Commands, cx);
            view.go_to_project(&crate::GoToProject { index: 1 }, window, cx);
            assert_eq!(view.snapshot.selected_project_id, Some(first));
            view.close_palette(cx);

            view.snapshot
                .projects
                .retain(|project| project.id == first || project.id == projects[2]);
            view.go_to_project(&crate::GoToProject { index: 1 }, window, cx);
            assert_eq!(view.snapshot.selected_project_id, Some(projects[2]));
            view.go_to_project(&crate::GoToProject { index: 8 }, window, cx);
            assert_eq!(view.snapshot.selected_project_id, Some(first));
            view.go_to_project(&crate::GoToProject { index: 0 }, window, cx);
            assert_eq!(view.snapshot.selected_project_id, Some(first));

            view.snapshot.projects.clear();
            view.snapshot.selected_project_id = None;
            view.go_to_project(&crate::GoToProject { index: 1 }, window, cx);
            assert_eq!(view.snapshot.selected_project_id, None);
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn project_shortcuts_restore_the_selected_terminal_from_global_pages(
    cx: &mut gpui::TestAppContext,
) {
    let (root, snapshot, _, window) = open_recording_workspace(cx, "project-shortcut-focus");
    let pane = snapshot.selected_session().unwrap().id;
    let tab = snapshot.selected_tab().unwrap().id;
    window
        .update(cx, |view, window, cx| {
            for section in [WorkspaceSection::Inbox, WorkspaceSection::Settings] {
                view.select_section(section, window, cx);
                view.go_to_project(&crate::GoToProject { index: 1 }, window, cx);
                assert_eq!(view.workspace_section, WorkspaceSection::Workspace);
                assert_eq!(view.snapshot.selected_tab().unwrap().id, tab);
                assert!(view.terminals[&pane].read(cx).is_surface_visible());
                assert!(
                    view.terminals[&pane]
                        .read(cx)
                        .focus_handle(cx)
                        .is_focused(window)
                );
            }
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn tab_shortcuts_return_from_global_pages_even_when_the_tab_is_selected(
    cx: &mut gpui::TestAppContext,
) {
    let (root, snapshot, _, window) = open_recording_workspace(cx, "tab-shortcut");
    let pane = snapshot.selected_session().unwrap().id;
    window
        .update(cx, |view, window, cx| {
            for section in [WorkspaceSection::Inbox, WorkspaceSection::Settings] {
                view.select_section(section, window, cx);
                view.go_to_tab(&crate::GoToTab { index: 1 }, window, cx);
                assert_eq!(view.workspace_section, WorkspaceSection::Workspace);
                assert!(view.terminals[&pane].read(cx).is_surface_visible());
                assert!(
                    view.terminals[&pane]
                        .read(cx)
                        .focus_handle(cx)
                        .is_focused(window)
                );
            }
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn numbered_tabs_include_the_review_when_clamping_to_the_last_tab(cx: &mut gpui::TestAppContext) {
    let (root, snapshot, _, window) = open_recording_workspace(cx, "review-shortcut");
    window
        .update(cx, |view, _, cx| {
            view.diff_view
                .update(cx, |diff, cx| diff.set_review_expanded(true, cx));
        })
        .unwrap();
    window
        .update(cx, |view, window, cx| {
            let tab = snapshot.selected_tab().unwrap().id;
            view.select_tab(tab, window, cx);
            view.go_to_tab(&crate::GoToTab { index: 8 }, window, cx);
            assert!(
                view.review_visible(cx),
                "out-of-range shortcuts select the last visible tab, including review"
            );
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn revealing_a_single_pane_from_review_restores_keyboard_focus(cx: &mut gpui::TestAppContext) {
    let (root, _, _, window) = open_recording_workspace(cx, "review-focus");
    window
        .update(cx, |view, window, cx| {
            view.close_terminal(&CloseTerminal, window, cx);
            view.diff_view
                .update(cx, |diff, cx| diff.set_review_expanded(true, cx));
        })
        .unwrap();
    window
        .update(cx, |view, window, cx| {
            view.activate_review_tab(window, cx);
            view.cycle_pane(1, window, cx);
            let pane = view.snapshot.selected_session().unwrap().id;
            assert!(!view.review_visible(cx));
            assert!(
                view.terminals[&pane]
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
            );
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn explorer_discards_previous_project_rows_before_loading_the_next(cx: &mut gpui::TestAppContext) {
    let (root, _, _, window) = open_recording_workspace(cx, "explorer-root");
    cx.run_until_parked();
    window
        .update(cx, |view, window, cx| {
            assert!(!view.project_files.is_empty());
            view.file_error = Some("old error".into());
            let next_root = root.join("second");
            std::fs::create_dir(&next_root).unwrap();
            let next = view.snapshot.add_project(&next_root);
            view.select_project(next, window, cx);
            assert!(
                view.project_files.is_empty(),
                "the previous project's rows must disappear synchronously"
            );
            assert!(view.file_error.is_none());
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn manual_pane_names_survive_agent_restarts_and_terminal_exit(cx: &mut gpui::TestAppContext) {
    let (root, snapshot, _, window) = open_recording_workspace(cx, "pane-name");
    let pane = snapshot.selected_session().unwrap().id;
    window
        .update(cx, |view, window, cx| {
            view.begin_rename_prompt(RenamePromptKind::Pane { session_id: pane }, cx);
            view.rename_prompt.as_mut().unwrap().value = "Mi revisión".into();
            view.confirm_rename_prompt(cx);
            for process_id in [7, 8] {
                view.handle_terminal_view_event(
                    &TerminalViewEvent::AgentPresenceChanged {
                        session_id: pane,
                        presence: Some(TerminalAgentPresence {
                            kind: "Codex".into(),
                            kind_source: TerminalAgentKindSource::Process,
                            state: AgentRuntimeState::Working,
                            state_source: TerminalAgentStateSource::Heuristic,
                            attention: None,
                            process_id: Some(process_id),
                        }),
                    },
                    cx,
                );
                assert_eq!(
                    view.pane_identity_by_id(pane, cx).unwrap().title,
                    "Mi revisión"
                );
            }
            view.handle_terminal_view_event(
                &TerminalViewEvent::Exited {
                    session_id: pane,
                    code: Some(0),
                },
                cx,
            );
            assert_eq!(
                view.pane_identity_by_id(pane, cx).unwrap().title,
                "Mi revisión"
            );
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn delayed_focus_cannot_return_to_a_hidden_tab_or_cover_a_global_page(
    cx: &mut gpui::TestAppContext,
) {
    let (root, snapshot, _, window) = open_recording_workspace(cx, "stale-focus");
    let previous = snapshot.selected_session().unwrap().id;
    window
        .update(cx, |view, window, cx| {
            view.open_terminal_tab_in_project(window, cx);
            let current = view.snapshot.selected_session().unwrap().id;
            // A deferred callback was queued before the user changed tabs.
            view.focus_terminal(previous, window, cx);
            assert!(
                view.terminals[&current]
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
            );

            view.select_section(WorkspaceSection::Inbox, window, cx);
            view.focus_terminal(current, window, cx);
            assert!(view.focus_handle.is_focused(window));

            view.select_section(WorkspaceSection::Workspace, window, cx);
            view.usage.open = true;
            view.focus_handle.focus(window);
            view.focus_terminal(current, window, cx);
            assert!(view.focus_handle.is_focused(window));
            view.on_workspace_key_down(
                &gpui::KeyDownEvent {
                    keystroke: gpui::Keystroke::parse("escape").unwrap(),
                    is_held: false,
                },
                window,
                cx,
            );
            assert!(!view.usage.open);
            assert!(
                view.terminals[&current]
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
            );

            view.usage.open = true;
            view.open_settings(window, cx);
            assert!(!view.usage.open);
            view.select_section(WorkspaceSection::Workspace, window, cx);
            view.usage.open = true;
            view.open_palette(PaletteMode::Commands, cx);
            assert!(!view.usage.open);
            view.close_palette(cx);
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn settings_navigation_preserves_the_workspace_and_returns_through_history(
    cx: &mut gpui::TestAppContext,
) {
    let (root, snapshot, _, window) = open_recording_workspace(cx, "settings-navigation");
    let pane = snapshot.selected_session().unwrap().id;
    window
        .update(cx, |view, window, cx| {
            view.show_settings(&ShowSettings, window, cx);
            assert_eq!(view.workspace_section, WorkspaceSection::Settings);
            assert_eq!(view.snapshot, snapshot);
            assert!(view.visible_terminal_ids(cx).is_empty());
            assert!(view.focus_handle.is_focused(window));
            view.focus_terminal(pane, window, cx);
            assert!(view.focus_handle.is_focused(window));

            // Reopening Settings keeps the active page instead of toggling it closed.
            view.settings_page = SettingsPage::Appearance;
            view.theme_query = "vibra".into();
            view.show_settings(&ShowSettings, window, cx);
            assert_eq!(view.workspace_section, WorkspaceSection::Settings);
            assert_eq!(view.theme_query, "vibra");

            view.navigate_back(&crate::NavigateBack, window, cx);
            assert_eq!(view.workspace_section, WorkspaceSection::Workspace);
            assert!(view.terminals[&pane].read(cx).is_surface_visible());
            assert!(
                view.terminals[&pane]
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
            );
            view.navigate_forward(&crate::NavigateForward, window, cx);
            assert_eq!(view.workspace_section, WorkspaceSection::Settings);
            assert!(view.settings_page == SettingsPage::Appearance);

            // Cmd-W leaves the page without closing the retained terminal.
            view.close_terminal(&CloseTerminal, window, cx);
            assert_eq!(view.workspace_section, WorkspaceSection::Workspace);
            assert_eq!(view.snapshot, snapshot);
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn settings_search_routes_keys_to_the_frontmost_overlay(cx: &mut gpui::TestAppContext) {
    let (root, _, _, window) = open_recording_workspace(cx, "settings-search");
    window
        .update(cx, |view, window, cx| {
            view.execute_palette_action(PaletteAction::ShowSettings, window, cx);
            view.settings_page = SettingsPage::Appearance;
            for mode in [PaletteMode::Commands, PaletteMode::Files] {
                view.theme_query = "vibra".into();
                view.open_palette(mode, cx);
                view.on_workspace_key_down(
                    &gpui::KeyDownEvent {
                        keystroke: gpui::Keystroke {
                            key_char: Some("x".into()),
                            ..gpui::Keystroke::parse("x").unwrap()
                        },
                        is_held: false,
                    },
                    window,
                    cx,
                );
                assert_eq!(view.palette_query, "x");
                assert_eq!(view.theme_query, "vibra");
                let escape = gpui::KeyDownEvent {
                    keystroke: gpui::Keystroke::parse("escape").unwrap(),
                    is_held: false,
                };
                view.on_workspace_key_down(&escape, window, cx);
                assert!(view.palette_mode.is_none());
                assert_eq!(view.workspace_section, WorkspaceSection::Settings);
                assert_eq!(view.theme_query, "vibra");
                view.on_workspace_key_down(&escape, window, cx);
                assert!(view.theme_query.is_empty());
                view.on_workspace_key_down(&escape, window, cx);
                assert_eq!(view.workspace_section, WorkspaceSection::Settings);
            }
            view.usage.open = true;
            assert!(view.usage_modal(cx).is_some());
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn dismissing_quick_open_cancels_pending_results(cx: &mut gpui::TestAppContext) {
    let (root, _, _, window) = open_recording_workspace(cx, "palette-cancel");
    cx.run_until_parked();
    window
        .update(cx, |view, window, cx| {
            view.open_palette(PaletteMode::Files, cx);
            view.on_workspace_key_down(
                &gpui::KeyDownEvent {
                    keystroke: gpui::Keystroke::parse("escape").unwrap(),
                    is_held: false,
                },
                window,
                cx,
            );
            assert!(view.palette_mode.is_none());
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, window, _| {
            assert!(view.palette_files.is_empty());
            assert!(!view.palette_loading);
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn clicking_a_space_leaves_nothing_to_reorder_when_agents_is_resized(
    cx: &mut gpui::TestAppContext,
) {
    use gpui::{Modifiers, MouseButton, point, px};

    let (root, snapshot, _, window) = open_recording_workspace(cx, "space-click-resize");
    let space = snapshot.selected_project_id.unwrap();
    let row: &'static str = Box::leak(format!("space-row-{space}").into_boxed_str());
    let mut cx = gpui::VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();

    // The row's click handler stops the release, which used to leave the
    // space marked as a reorder source.
    let row = cx.debug_bounds(row).expect("the space row is drawn");
    cx.simulate_click(row.center(), Modifiers::none());
    window
        .update(&mut cx, |view, _, _| assert!(view.reorder_drag.is_none()))
        .unwrap();

    let handle = cx
        .debug_bounds("agents-resize-handle")
        .expect("the Agents resize handle is drawn");
    let start = handle.center();
    let height = window
        .update(&mut cx, |view, _, _| view.settings.agents_panel_height)
        .unwrap();
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    for step in 1..=3 {
        let pointer = point(start.x, start.y - px(12.0 * step as f32));
        cx.simulate_mouse_move(pointer, MouseButton::Left, Modifiers::none());
    }
    window
        .update(&mut cx, |view, _, cx| {
            assert!(cx.has_active_drag(), "Agents is being resized");
            assert!(view.settings.agents_panel_height > height);
            assert!(view.reorder_drag.is_none(), "no space is moving");
        })
        .unwrap();
    cx.simulate_mouse_up(
        point(start.x, start.y - px(36.0)),
        MouseButton::Left,
        Modifiers::none(),
    );
    window
        .update(&mut cx, |_, window, _| window.remove_window())
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn an_agent_card_keeps_a_short_space_name_on_one_line(cx: &mut gpui::TestAppContext) {
    use gpui::px;

    let (root, snapshot, _, window) = open_recording_workspace(cx, "agent-card-space");
    let pane = snapshot.selected_session().unwrap().id;
    let space = snapshot.selected_project_id.unwrap();
    let selector: &'static str = Box::leak(format!("sidebar-agent-space-{pane}").into_boxed_str());
    let mut cx = gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(&mut cx, |view, _, cx| {
            let project = view
                .snapshot
                .projects
                .iter_mut()
                .find(|project| project.id == space)
                .unwrap();
            project.name = "Vibra".into();
            let directory = project.directory().unwrap().to_owned();
            // The branch beside the name made the row lay its text out at
            // min-content first, which used to leave the name 0px wide.
            view.project_branches
                .insert(directory.into(), "main".into());
            view.handle_terminal_view_event(
                &TerminalViewEvent::AgentPresenceChanged {
                    session_id: pane,
                    presence: Some(TerminalAgentPresence {
                        kind: "Claude".into(),
                        kind_source: TerminalAgentKindSource::Process,
                        state: AgentRuntimeState::Idle,
                        state_source: TerminalAgentStateSource::Heuristic,
                        attention: None,
                        process_id: Some(7),
                    }),
                },
                cx,
            );
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();

    let name = cx
        .debug_bounds(selector)
        .expect("the agent card names its space");
    assert!(name.size.width > px(20.0), "the name has room: {name:?}");
    assert!(
        name.size.height < px(24.0),
        "the name is one line: {name:?}"
    );
    window
        .update(&mut cx, |_, window, _| window.remove_window())
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
