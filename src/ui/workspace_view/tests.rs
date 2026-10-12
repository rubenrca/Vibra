use super::*;
use crate::infrastructure::automation::AgentRuntimeState;
use crate::ports::terminal::TerminalAgentKindSource;
use crate::ui::terminal::TerminalViewEvent;
use crate::{CloseTerminal, NextProject, ShowSettings, ToggleRightSidebar};
use gpui::Focusable;
use std::path::Path;

use crate::domain::agent_chat::{
    AgentChatBinding, AgentModelCatalog, CatalogModel, ChatAgent, PaneViewMode,
};
use crate::domain::workspace::WorkspaceSnapshot;
use crate::ui::agent_chat::AgentChatViewEvent;
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
    foreground: Option<&'static str>,
}

/// Records what each terminal was sent, as the shell would read it.
struct RecordingTerminalPort {
    inputs: SentInput,
    foreground: Option<&'static str>,
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
            foreground: self.foreground,
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
            foreground: None,
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
    fn foreground_process_name(&self) -> Option<String> {
        self.foreground.map(str::to_owned)
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

            // The right-sidebar shortcut also returns keyboard focus to the
            // terminal when invoked from a global page, even if tools were open.
            view.select_section(WorkspaceSection::Inbox, window, cx);
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
    open_recording_workspace_with_foreground(cx, name, None)
}

/// Like `open_recording_workspace`, with `foreground` as each shell's foreground program.
pub(super) fn open_recording_workspace_with_foreground(
    cx: &mut gpui::TestAppContext,
    name: &str,
    foreground: Option<&'static str>,
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
        foreground,
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
            assert_eq!(items[0].title, "Codex finished");
            assert!(!items[0].read);
            assert!(
                items[1].read,
                "the pane the user is watching does not count as unread"
            );
            view.handle_terminal_view_event(
                &TerminalViewEvent::Activated { session_id: other },
                cx,
            );
            assert!(view.inbox.items().all(|item| item.read));
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
fn project_agent_sessions_follow_presence_across_tabs_and_projects(cx: &mut gpui::TestAppContext) {
    let (root, snapshot, _, window) = open_recording_workspace(cx, "project-agents");
    let first_project = snapshot.selected_project_id.unwrap();
    let first_pane = snapshot.selected_session().unwrap().id;
    window
        .update(cx, |view, window, cx| {
            // A saved title alone must never turn a shell into an agent row.
            view.snapshot
                .update_agent_task_title(first_pane, "Review sidebar");
            assert!(view.project_agent_sessions(first_project, cx).is_empty());
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
                            process_id: Some(7),
                        }),
                    },
                    cx,
                );
            }
            let sessions = view.project_agent_sessions(first_project, cx);
            assert_eq!(
                sessions.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
                vec![first_pane, second_pane]
            );
            assert_eq!(sessions[0].1.title, "Review sidebar");
            assert_eq!(sessions[0].1.agent_state, Some(AgentRuntimeState::Working));
            assert_eq!(sessions[1].1.agent_state, Some(AgentRuntimeState::Waiting));
            let sessions = view.project_agent_sessions(second_project, cx);
            assert_eq!(sessions.len(), 1);
            assert_eq!(sessions[0].0, third_pane);
            assert_eq!(sessions[0].1.agent_state, Some(AgentRuntimeState::Idle));

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
            let sessions = view.project_agent_sessions(first_project, cx);
            assert_eq!(sessions.len(), 1);
            assert_eq!(sessions[0].0, second_pane);
            view.handle_terminal_view_event(
                &TerminalViewEvent::Exited {
                    session_id: second_pane,
                    code: Some(0),
                },
                cx,
            );
            assert!(view.project_agent_sessions(first_project, cx).is_empty());
            assert_eq!(view.project_agent_sessions(second_project, cx).len(), 1);
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
            assert!(view.usage_popover(window, cx).is_some());
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

/// The bytes typed into one pane's terminal, in order.
fn typed_into(inputs: &SentInput, pane: Uuid) -> Vec<Vec<u8>> {
    inputs
        .lock()
        .unwrap()
        .iter()
        .filter(|(session_id, _)| *session_id == pane)
        .map(|(_, bytes)| bytes.clone())
        .collect()
}

fn agent_presence(kind: &str) -> crate::ports::terminal::TerminalAgentPresence {
    crate::ports::terminal::TerminalAgentPresence {
        kind: kind.into(),
        kind_source: TerminalAgentKindSource::Process,
        state: AgentRuntimeState::Working,
        process_id: Some(7),
    }
}

fn chat_binding(agent: ChatAgent, session: Option<&str>, mode: PaneViewMode) -> AgentChatBinding {
    AgentChatBinding {
        agent,
        session_id: session.map(str::to_owned),
        mode,
    }
}

#[test]
fn resume_commands_are_typed_only_at_an_idle_shell() {
    use super::pane_chat::may_type_resume_command;

    assert!(may_type_resume_command(false, true));
    assert!(!may_type_resume_command(true, true));
    assert!(!may_type_resume_command(false, false));
}

#[gpui::test]
fn pane_chat_toggle_keeps_the_terminal_alive_and_hides_it_in_chat_view(
    cx: &mut gpui::TestAppContext,
) {
    let (root, snapshot, _, window) = open_recording_workspace(cx, "chat-toggle");
    let pane = snapshot.selected_session().unwrap().id;
    window
        .update(cx, |view, window, cx| {
            view.snapshot.set_agent_chat_binding(
                pane,
                Some(chat_binding(
                    ChatAgent::Claude,
                    None,
                    PaneViewMode::Terminal,
                )),
            );
            let terminal = view.terminals[&pane].clone();

            view.set_pane_view_mode(pane, PaneViewMode::Chat, window, cx);
            assert_eq!(view.pane_view_mode(pane), PaneViewMode::Chat);
            assert!(view.agent_chats.contains_key(&pane));
            assert_eq!(view.terminals[&pane].entity_id(), terminal.entity_id());
            assert!(!view.terminals[&pane].read(cx).is_surface_visible());
            assert!(!view.visible_terminal_ids(cx).contains(&pane));
            assert!(
                view.pane_focus_handle(pane, cx)
                    .is_some_and(|focus| focus.is_focused(window)),
                "the chat takes focus in chat view"
            );

            // No session yet, so the chat stays alive while the terminal is shown.
            view.set_pane_view_mode(pane, PaneViewMode::Terminal, window, cx);
            assert_eq!(view.pane_view_mode(pane), PaneViewMode::Terminal);
            assert!(view.agent_chats.contains_key(&pane));
            assert_eq!(view.terminals[&pane].entity_id(), terminal.entity_id());
            assert!(view.terminals[&pane].read(cx).is_surface_visible());
            assert!(view.visible_terminal_ids(cx).contains(&pane));
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
fn chat_handoff_resumes_the_session_in_an_idle_shell(cx: &mut gpui::TestAppContext) {
    let (root, snapshot, inputs, window) =
        open_recording_workspace_with_foreground(cx, "chat-resume", Some("zsh"));
    let pane = snapshot.selected_session().unwrap().id;
    window
        .update(cx, |view, window, cx| {
            view.snapshot.set_agent_chat_binding(
                pane,
                Some(chat_binding(
                    ChatAgent::Codex,
                    Some("thread-42"),
                    PaneViewMode::Chat,
                )),
            );
            // A restored chat connects only when the user asks for it.
            view.connect_waiting_pane_chat(pane, window, cx);
            assert!(!view.agent_chats.contains_key(&pane));
            view.set_pane_view_mode(pane, PaneViewMode::Chat, window, cx);
            assert!(view.agent_chats.contains_key(&pane));

            view.set_pane_view_mode(pane, PaneViewMode::Terminal, window, cx);
            assert!(!view.agent_chats.contains_key(&pane), "the chat shut down");
            assert_eq!(
                typed_into(&inputs, pane),
                vec![b"codex resume thread-42\r".to_vec()]
            );
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn chat_handoff_does_not_type_into_a_pane_that_runs_an_agent(cx: &mut gpui::TestAppContext) {
    let (root, snapshot, inputs, window) =
        open_recording_workspace_with_foreground(cx, "chat-busy", Some("zsh"));
    let pane = snapshot.selected_session().unwrap().id;
    window
        .update(cx, |view, window, cx| {
            view.snapshot.set_agent_chat_binding(
                pane,
                Some(chat_binding(
                    ChatAgent::Codex,
                    Some("thread-42"),
                    PaneViewMode::Chat,
                )),
            );
            view.set_pane_view_mode(pane, PaneViewMode::Chat, window, cx);
            view.handle_terminal_view_event(
                &TerminalViewEvent::AgentPresenceChanged {
                    session_id: pane,
                    presence: Some(agent_presence("Codex")),
                },
                cx,
            );

            view.set_pane_view_mode(pane, PaneViewMode::Terminal, window, cx);
            assert_eq!(view.pane_view_mode(pane), PaneViewMode::Terminal);
            assert!(typed_into(&inputs, pane).is_empty());
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn chat_handoff_does_not_type_over_a_running_program(cx: &mut gpui::TestAppContext) {
    let (root, snapshot, inputs, window) =
        open_recording_workspace_with_foreground(cx, "chat-program", Some("node"));
    let pane = snapshot.selected_session().unwrap().id;
    window
        .update(cx, |view, window, cx| {
            view.snapshot.set_agent_chat_binding(
                pane,
                Some(chat_binding(
                    ChatAgent::Claude,
                    Some("sess-1"),
                    PaneViewMode::Chat,
                )),
            );
            view.set_pane_view_mode(pane, PaneViewMode::Chat, window, cx);
            view.set_pane_view_mode(pane, PaneViewMode::Terminal, window, cx);
            assert!(!view.agent_chats.contains_key(&pane));
            assert!(typed_into(&inputs, pane).is_empty());
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn waiting_chat_connects_once_the_terminal_agent_leaves(cx: &mut gpui::TestAppContext) {
    let (root, snapshot, _, window) = open_recording_workspace(cx, "chat-waiting");
    let pane = snapshot.selected_session().unwrap().id;
    window
        .update(cx, |view, window, cx| {
            view.snapshot.set_agent_chat_binding(
                pane,
                Some(chat_binding(
                    ChatAgent::Claude,
                    None,
                    PaneViewMode::Terminal,
                )),
            );
            // Claude still runs in the terminal, so the user's chat waits for it to exit.
            view.handle_terminal_view_event(
                &TerminalViewEvent::AgentPresenceChanged {
                    session_id: pane,
                    presence: Some(agent_presence("Claude")),
                },
                cx,
            );
            view.set_pane_view_mode(pane, PaneViewMode::Chat, window, cx);
            assert_eq!(view.pane_view_mode(pane), PaneViewMode::Chat);
            view.connect_waiting_pane_chat(pane, window, cx);
            assert!(!view.agent_chats.contains_key(&pane));

            view.handle_terminal_view_event(
                &TerminalViewEvent::AgentPresenceChanged {
                    session_id: pane,
                    presence: None,
                },
                cx,
            );
            view.connect_waiting_pane_chat(pane, window, cx);
            assert!(view.agent_chats.contains_key(&pane));
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn closed_chat_reconnects_when_the_pane_is_toggled(cx: &mut gpui::TestAppContext) {
    let (root, snapshot, _, window) = open_recording_workspace(cx, "chat-closed");
    let pane = snapshot.selected_session().unwrap().id;
    window
        .update(cx, |view, window, cx| {
            view.snapshot.set_agent_chat_binding(
                pane,
                Some(chat_binding(ChatAgent::Grok, None, PaneViewMode::Terminal)),
            );
            view.set_pane_view_mode(pane, PaneViewMode::Chat, window, cx);
            let first = view.agent_chats[&pane].view.entity_id();
            // The test connector cannot start an agent, so the chat begins closed.
            assert!(view.agent_chats[&pane].closed);

            // A live chat that closes keeps its view on screen with the error.
            view.agent_chats.get_mut(&pane).unwrap().closed = false;
            view.handle_pane_chat_event(pane, &AgentChatViewEvent::Closed, window, cx);
            assert!(view.agent_chats[&pane].closed);
            assert_eq!(view.agent_chats[&pane].view.entity_id(), first);

            // Toggling away and back replaces the closed chat with a new connection.
            view.set_pane_view_mode(pane, PaneViewMode::Terminal, window, cx);
            view.set_pane_view_mode(pane, PaneViewMode::Chat, window, cx);
            assert_ne!(view.agent_chats[&pane].view.entity_id(), first);
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn session_ids_are_bound_to_the_pane_for_their_own_agent_only(cx: &mut gpui::TestAppContext) {
    let (root, snapshot, _, window) = open_recording_workspace(cx, "chat-bind");
    let pane = snapshot.selected_session().unwrap().id;
    window
        .update(cx, |view, window, cx| {
            view.snapshot.set_agent_chat_binding(
                pane,
                Some(chat_binding(ChatAgent::Codex, None, PaneViewMode::Chat)),
            );
            let generation = view.persist_generation;

            view.handle_pane_chat_event(
                pane,
                &AgentChatViewEvent::SessionBound {
                    agent: ChatAgent::Codex,
                    session_id: "thread-9".into(),
                },
                window,
                cx,
            );
            assert_eq!(
                view.snapshot
                    .agent_chat_binding(pane)
                    .and_then(|binding| binding.session_id.as_deref()),
                Some("thread-9")
            );
            assert_ne!(view.persist_generation, generation, "the binding is saved");

            view.handle_pane_chat_event(
                pane,
                &AgentChatViewEvent::SessionBound {
                    agent: ChatAgent::Claude,
                    session_id: "other".into(),
                },
                window,
                cx,
            );
            assert_eq!(
                view.snapshot
                    .agent_chat_binding(pane)
                    .and_then(|binding| binding.session_id.as_deref()),
                Some("thread-9")
            );
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn open_chat_starts_a_new_tab_in_the_pane_folder(cx: &mut gpui::TestAppContext) {
    let (root, snapshot, _, window) = open_recording_workspace(cx, "chat-tab");
    let project = snapshot.selected_project_id.unwrap();
    let cwd = root.join("nested");
    window
        .update(cx, |view, window, cx| {
            view.open_agent_chat_tab(project, ChatAgent::Grok, cwd.clone(), window, cx);
            let session = view.snapshot.selected_session().unwrap();
            let session_id = session.id;
            assert_eq!(session.working_directory, cwd.to_str().unwrap());
            assert_eq!(
                view.snapshot.agent_chat_binding(session_id),
                Some(&chat_binding(ChatAgent::Grok, None, PaneViewMode::Chat))
            );
            assert!(view.terminals.contains_key(&session_id));
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn restored_chat_pane_does_not_connect_until_the_user_reconnects(cx: &mut gpui::TestAppContext) {
    let (root, snapshot, _, window) = open_recording_workspace(cx, "chat-restored");
    let pane = snapshot.selected_session().unwrap().id;
    window
        .update(cx, |view, window, cx| {
            // A pane restored from the workspace file while in chat view.
            view.snapshot.set_agent_chat_binding(
                pane,
                Some(chat_binding(
                    ChatAgent::Codex,
                    Some("thread-7"),
                    PaneViewMode::Chat,
                )),
            );
            view.connect_waiting_pane_chat(pane, window, cx);
            assert!(!view.agent_chats.contains_key(&pane));
            assert!(!view.pending_chat_panes.contains(&pane));

            // Reconnect is the user's request; it connects the pane's session.
            view.set_pane_view_mode(pane, PaneViewMode::Chat, window, cx);
            assert!(view.agent_chats.contains_key(&pane));
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn showing_the_terminal_of_a_restored_pane_launches_no_agent(cx: &mut gpui::TestAppContext) {
    let (root, snapshot, inputs, window) =
        open_recording_workspace_with_foreground(cx, "chat-show-terminal", Some("zsh"));
    let pane = snapshot.selected_session().unwrap().id;
    window
        .update(cx, |view, window, cx| {
            view.snapshot.set_agent_chat_binding(
                pane,
                Some(chat_binding(
                    ChatAgent::Codex,
                    Some("thread-42"),
                    PaneViewMode::Chat,
                )),
            );
            // The restored placeholder is shown; nothing connected.
            view.connect_waiting_pane_chat(pane, window, cx);
            view.set_pane_view_mode(pane, PaneViewMode::Terminal, window, cx);

            assert_eq!(view.pane_view_mode(pane), PaneViewMode::Terminal);
            assert!(
                typed_into(&inputs, pane).is_empty(),
                "no resume command is typed"
            );
            assert_eq!(
                view.snapshot.agent_chat_binding(pane),
                Some(&chat_binding(
                    ChatAgent::Codex,
                    Some("thread-42"),
                    PaneViewMode::Terminal
                ))
            );
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn toggle_chat_switches_the_selected_pane_without_a_picker(cx: &mut gpui::TestAppContext) {
    let (root, snapshot, _, window) = open_recording_workspace(cx, "chat-toggle-action");
    let pane = snapshot.selected_session().unwrap().id;
    window
        .update(cx, |view, window, cx| {
            let default = view
                .default_chat_agent(Some(pane))
                .unwrap_or(ChatAgent::ALL[0]);

            view.toggle_selected_pane_chat(window, cx);
            assert!(view.context_menu.is_none(), "no agent picker opens");
            assert_eq!(view.pane_view_mode(pane), PaneViewMode::Chat);
            assert_eq!(
                view.snapshot.agent_chat_binding(pane),
                Some(&chat_binding(default, None, PaneViewMode::Chat))
            );
            assert!(view.agent_chats.contains_key(&pane));

            view.toggle_selected_pane_chat(window, cx);
            assert_eq!(view.pane_view_mode(pane), PaneViewMode::Terminal);
            assert!(view.terminals[&pane].read(cx).is_surface_visible());

            view.toggle_selected_pane_chat(window, cx);
            assert_eq!(view.pane_view_mode(pane), PaneViewMode::Chat);
            assert!(view.context_menu.is_none());
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn switching_chat_providers_preserves_the_unsent_prompt_and_references(
    cx: &mut gpui::TestAppContext,
) {
    let (root, snapshot, _, window) = open_recording_workspace(cx, "chat-provider-draft");
    let pane = snapshot.selected_session().unwrap().id;
    let draft = (
        "Review this file\nand explain the change".to_owned(),
        vec![crate::domain::agent_chat::PromptFile {
            path: root.join("src/main.rs").to_string_lossy().into_owned(),
            label: "src/main.rs".to_owned(),
        }],
    );
    window
        .update(cx, |view, window, cx| {
            view.snapshot.set_agent_chat_binding(
                pane,
                Some(chat_binding(ChatAgent::Claude, None, PaneViewMode::Chat)),
            );
            view.connect_pane_chat(pane, window, cx);
            let first = view.agent_chats[&pane].view.clone();
            first.update(cx, |chat, cx| chat.restore_draft(draft.clone(), cx));
            view.handle_pane_chat_event(
                pane,
                &AgentChatViewEvent::SwitchAgent {
                    agent: ChatAgent::Codex,
                    model: Some("gpt-6.1-sol".into()),
                },
                window,
                cx,
            );
            let next = view.agent_chats[&pane].view.clone();
            assert_ne!(first.entity_id(), next.entity_id());
            assert_eq!(next.read(cx).agent(), ChatAgent::Codex);
            assert_eq!(next.update(cx, |chat, _| chat.take_draft()), draft);
            assert!(!view.pending_chat_setups.contains_key(&pane));

            next.update(cx, |chat, cx| chat.restore_draft(draft.clone(), cx));
            view.handle_terminal_view_event(
                &TerminalViewEvent::AgentPresenceChanged {
                    session_id: pane,
                    presence: Some(agent_presence("Claude")),
                },
                cx,
            );
            view.handle_pane_chat_event(
                pane,
                &AgentChatViewEvent::SwitchAgent {
                    agent: ChatAgent::Claude,
                    model: Some("opus".into()),
                },
                window,
                cx,
            );
            assert!(!view.agent_chats.contains_key(&pane));
            assert!(view.pending_chat_setups.contains_key(&pane));
            view.handle_terminal_view_event(
                &TerminalViewEvent::AgentPresenceChanged {
                    session_id: pane,
                    presence: None,
                },
                cx,
            );
            view.connect_waiting_pane_chat(pane, window, cx);
            let resumed = view.agent_chats[&pane].view.clone();
            assert_eq!(resumed.read(cx).agent(), ChatAgent::Claude);
            assert_eq!(resumed.update(cx, |chat, _| chat.take_draft()), draft);
            assert!(!view.pending_chat_setups.contains_key(&pane));
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn new_chats_use_the_remembered_agent_then_the_pane_agent(cx: &mut gpui::TestAppContext) {
    let (root, snapshot, _, window) = open_recording_workspace(cx, "chat-default");
    let pane = snapshot.selected_session().unwrap().id;
    window
        .update(cx, |view, window, cx| {
            let installed = [ChatAgent::Claude, ChatAgent::Codex, ChatAgent::Grok];
            view.settings.chat_last_agent = Some(ChatAgent::Grok);
            assert_eq!(
                view.default_chat_agent_among(&installed, Some(pane)),
                Some(ChatAgent::Grok)
            );

            view.settings.chat_last_agent = None;
            view.handle_terminal_view_event(
                &TerminalViewEvent::AgentPresenceChanged {
                    session_id: pane,
                    presence: Some(agent_presence("Codex")),
                },
                cx,
            );
            assert_eq!(
                view.default_chat_agent_among(&installed, Some(pane)),
                Some(ChatAgent::Codex)
            );
            // A new tab has no pane to detect in, so it takes the first installed agent.
            assert_eq!(
                view.default_chat_agent_among(&installed, None),
                Some(ChatAgent::Claude)
            );
            assert_eq!(view.default_chat_agent_among(&[], Some(pane)), None);

            view.handle_terminal_view_event(
                &TerminalViewEvent::AgentPresenceChanged {
                    session_id: pane,
                    presence: None,
                },
                cx,
            );
            view.settings.chat_last_agent = Some(ChatAgent::Grok);
            let expected = view.default_chat_agent(None).unwrap_or(ChatAgent::ALL[0]);
            view.new_chat_tab(window, cx);
            let new_pane = view.snapshot.selected_session().unwrap().id;
            assert_ne!(new_pane, pane);
            assert_eq!(
                view.snapshot.agent_chat_binding(new_pane),
                Some(&chat_binding(expected, None, PaneViewMode::Chat))
            );
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn switching_agent_before_a_message_rebinds_the_pane_and_is_remembered(
    cx: &mut gpui::TestAppContext,
) {
    let (root, snapshot, _, window) = open_recording_workspace(cx, "chat-switch");
    let pane = snapshot.selected_session().unwrap().id;
    window
        .update(cx, |view, window, cx| {
            view.snapshot.set_agent_chat_binding(
                pane,
                Some(chat_binding(
                    ChatAgent::Claude,
                    None,
                    PaneViewMode::Terminal,
                )),
            );
            view.set_pane_view_mode(pane, PaneViewMode::Chat, window, cx);
            let first = view.agent_chats[&pane].view.entity_id();
            let generation = view.settings_generation;

            view.handle_pane_chat_event(
                pane,
                &AgentChatViewEvent::SwitchAgent {
                    agent: ChatAgent::Grok,
                    model: Some("grok-4".into()),
                },
                window,
                cx,
            );

            assert_eq!(
                view.snapshot.agent_chat_binding(pane),
                Some(&chat_binding(ChatAgent::Grok, None, PaneViewMode::Chat))
            );
            assert_eq!(
                view.agent_chats[&pane].view.read(cx).agent(),
                ChatAgent::Grok
            );
            assert_ne!(view.agent_chats[&pane].view.entity_id(), first);
            assert_eq!(view.settings.chat_last_agent, Some(ChatAgent::Grok));
            assert_ne!(view.settings_generation, generation, "the choice is saved");
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn models_known_upserts_the_catalog_and_saves_only_changes(cx: &mut gpui::TestAppContext) {
    let (root, snapshot, _, window) = open_recording_workspace(cx, "chat-models");
    let pane = snapshot.selected_session().unwrap().id;
    window
        .update(cx, |view, window, cx| {
            let gpt = vec![CatalogModel {
                value: "gpt-5".into(),
                name: "GPT-5".into(),
            }];
            let known = |agent, models: Vec<CatalogModel>| AgentChatViewEvent::ModelsKnown {
                agent,
                models,
            };

            view.handle_pane_chat_event(pane, &known(ChatAgent::Codex, gpt.clone()), window, cx);
            assert_eq!(
                view.settings.chat_model_catalog,
                vec![AgentModelCatalog {
                    agent: ChatAgent::Codex,
                    models: gpt.clone(),
                }]
            );
            let generation = view.settings_generation;

            view.handle_pane_chat_event(pane, &known(ChatAgent::Codex, gpt.clone()), window, cx);
            assert_eq!(
                view.settings_generation, generation,
                "an unchanged list is not saved"
            );

            let renamed = vec![CatalogModel {
                value: "gpt-5".into(),
                name: "GPT-5 (renamed)".into(),
            }];
            view.handle_pane_chat_event(
                pane,
                &known(ChatAgent::Codex, renamed.clone()),
                window,
                cx,
            );
            assert_eq!(view.settings.chat_model_catalog.len(), 1);
            assert_eq!(view.settings.chat_model_catalog[0].models, renamed);
            assert_ne!(view.settings_generation, generation);

            view.handle_pane_chat_event(pane, &known(ChatAgent::Grok, gpt.clone()), window, cx);
            assert_eq!(view.settings.chat_model_catalog.len(), 2);
            // An empty report leaves the remembered list alone.
            view.handle_pane_chat_event(pane, &known(ChatAgent::Grok, Vec::new()), window, cx);
            assert_eq!(view.settings.chat_model_catalog.len(), 2);
            assert_eq!(
                view.settings.chat_model_catalog[1].models, gpt,
                "Grok keeps its models"
            );
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn chat_panes_are_named_by_their_chat_and_marked_with_its_agent(cx: &mut gpui::TestAppContext) {
    let (root, snapshot, _, window) = open_recording_workspace(cx, "chat-title-tab");
    let pane = snapshot.selected_session().unwrap().id;
    window
        .update(cx, |view, window, cx| {
            view.snapshot.set_agent_chat_binding(
                pane,
                Some(chat_binding(ChatAgent::Codex, None, PaneViewMode::Terminal)),
            );
            view.set_pane_view_mode(pane, PaneViewMode::Chat, window, cx);
            let identity = view.pane_identity_by_id(pane, cx).unwrap();
            assert_eq!(identity.title, "New chat");
            assert_eq!(identity.agent_kind.as_deref(), Some("Codex"));

            view.handle_pane_chat_event(
                pane,
                &AgentChatViewEvent::TitleChanged("Fix login".into()),
                window,
                cx,
            );
            assert_eq!(
                view.pane_identity_by_id(pane, cx).unwrap().title,
                "Fix login"
            );

            view.pane_names.insert(pane, "Auth".into());
            assert_eq!(view.pane_identity_by_id(pane, cx).unwrap().title, "Auth");
            view.pane_names.remove(&pane);

            // The terminal view keeps the folder name, not the chat's title.
            view.set_pane_view_mode(pane, PaneViewMode::Terminal, window, cx);
            let identity = view.pane_identity_by_id(pane, cx).unwrap();
            assert_ne!(identity.title, "Fix login");
            assert_eq!(identity.agent_kind, None);
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_lone_pane_has_no_header_but_split_and_zoomed_panes_do() {
    use super::panes::pane_header_visible;

    assert!(!pane_header_visible(false, false));
    assert!(pane_header_visible(true, false));
    assert!(pane_header_visible(false, true));
}

#[test]
fn new_chats_start_with_the_last_agent_then_the_pane_agent_then_the_first_installed() {
    use super::pane_chat::choose_chat_agent;

    let installed = [ChatAgent::Claude, ChatAgent::Codex];
    assert_eq!(
        choose_chat_agent(&installed, Some(ChatAgent::Claude), Some(ChatAgent::Codex)),
        Some(ChatAgent::Claude)
    );
    // An uninstalled last agent falls through to the pane's agent.
    assert_eq!(
        choose_chat_agent(&installed, Some(ChatAgent::Grok), Some(ChatAgent::Codex)),
        Some(ChatAgent::Codex)
    );
    assert_eq!(
        choose_chat_agent(&installed, None, None),
        Some(ChatAgent::Claude)
    );
    assert_eq!(
        choose_chat_agent(&[ChatAgent::Grok], Some(ChatAgent::Claude), None),
        Some(ChatAgent::Grok)
    );
    assert_eq!(choose_chat_agent(&[], Some(ChatAgent::Claude), None), None);
}

#[test]
fn chat_panes_are_named_by_the_manual_name_then_the_chat_title() {
    use super::pane_chat::chat_pane_title;

    assert_eq!(chat_pane_title(Some("Auth"), Some("Fix login")), "Auth");
    assert_eq!(chat_pane_title(Some("  "), Some("Fix login")), "Fix login");
    assert_eq!(chat_pane_title(None, Some(" ")), "New chat");
    assert_eq!(chat_pane_title(None, None), "New chat");
}

#[gpui::test]
fn split_with_chat_creates_a_pane_bound_to_a_new_session(cx: &mut gpui::TestAppContext) {
    let (root, snapshot, _, window) = open_recording_workspace(cx, "chat-split");
    let pane = snapshot.selected_session().unwrap().id;
    window
        .update(cx, |view, window, cx| {
            let before = view.snapshot.terminal_sessions().count();
            view.split_pane_with_chat(pane, PaneSplitDirection::Down, ChatAgent::Codex, window, cx);

            assert_eq!(view.snapshot.terminal_sessions().count(), before + 1);
            let new_pane = view.snapshot.selected_session().unwrap().id;
            assert_ne!(new_pane, pane);
            assert_eq!(
                view.snapshot.agent_chat_binding(new_pane),
                Some(&chat_binding(ChatAgent::Codex, None, PaneViewMode::Chat))
            );
            assert!(view.agent_chats.contains_key(&new_pane));
            assert!(
                view.terminals.contains_key(&new_pane),
                "the shell runs behind the chat"
            );
            assert!(!view.visible_terminal_ids(cx).contains(&new_pane));
            assert_eq!(view.snapshot.agent_chat_binding(pane), None);
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn detection_never_changes_the_mode_or_the_agent(cx: &mut gpui::TestAppContext) {
    let (root, snapshot, _, window) = open_recording_workspace(cx, "chat-detect");
    let panes: Vec<Uuid> = snapshot
        .terminal_sessions()
        .map(|session| session.id)
        .collect();
    let [plain, bound] = panes[..] else {
        panic!("the workspace starts with two panes");
    };
    window
        .update(cx, |view, window, cx| {
            view.snapshot.set_agent_chat_binding(
                bound,
                Some(chat_binding(ChatAgent::Codex, None, PaneViewMode::Terminal)),
            );
            for pane in [plain, bound] {
                view.handle_terminal_view_event(
                    &TerminalViewEvent::AgentPresenceChanged {
                        session_id: pane,
                        presence: Some(agent_presence("Claude")),
                    },
                    cx,
                );
                view.connect_waiting_pane_chat(pane, window, cx);
            }

            assert_eq!(view.snapshot.agent_chat_binding(plain), None);
            assert_eq!(
                view.snapshot.agent_chat_binding(bound),
                Some(&chat_binding(
                    ChatAgent::Codex,
                    None,
                    PaneViewMode::Terminal
                )),
                "a bound pane keeps its agent when another agent runs"
            );
            assert_eq!(view.pane_view_mode(plain), PaneViewMode::Terminal);
            assert_eq!(view.pane_view_mode(bound), PaneViewMode::Terminal);
            assert!(view.agent_chats.is_empty());
            assert!(view.pending_chat_panes.is_empty());
            window.remove_window();
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn linked_worktrees_are_found_from_their_git_file() {
    use super::pane_chat::{is_linked_worktree, is_linked_worktree_pointer};

    assert!(is_linked_worktree_pointer(
        "gitdir: /repo/.git/worktrees/feature\n"
    ));
    assert!(is_linked_worktree_pointer(
        "gitdir: C:\\repo\\.git\\worktrees\\feature"
    ));
    assert!(!is_linked_worktree_pointer(
        "gitdir: /repo/.git/modules/library\n"
    ));
    assert!(!is_linked_worktree_pointer("ref: refs/heads/main"));

    let root = std::env::temp_dir().join(format!("vibra-worktree-{}", Uuid::new_v4()));
    let linked = root.join("feature");
    std::fs::create_dir_all(linked.join("src")).unwrap();
    std::fs::write(
        linked.join(".git"),
        "gitdir: /repo/.git/worktrees/feature\n",
    )
    .unwrap();
    let main = root.join("main");
    std::fs::create_dir_all(main.join(".git")).unwrap();
    assert!(is_linked_worktree(&linked.join("src")));
    assert!(!is_linked_worktree(&main));
    std::fs::remove_dir_all(root).unwrap();
}
