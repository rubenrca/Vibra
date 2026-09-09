//! Workspace state and coordination. Feature modules share this view's state:
//! settings owns its pages, automation resolves agent activity, and dev_terminal
//! manages utility PTYs. None of them introduces a second workspace model.

mod automation;
mod chrome;
mod dev_terminal;
mod drag;
mod files;
mod input;
mod palette;
mod panes;
mod settings;
mod titlebar;

use automation::HookAgentPresence;
use chrome::*;
use dev_terminal::DevTerminalDrawer;
pub(crate) use drag::*;
use files::*;
use settings::SettingsPage;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Context, DragMoveEvent, Entity, FocusHandle, Focusable, IntoElement, MouseButton,
    MouseDownEvent, ParentElement, Render, SharedString, Styled, Subscription, Task, Timer,
    Transformation, Window, div, prelude::*, px, radians, svg,
};
use uuid::Uuid;

use crate::domain::workspace::{
    PaneSplitDirection, SessionSnapshot, SidebarEntry, WorkspaceSnapshot,
};
use crate::infrastructure::automation::{
    AgentAttention, AgentHookStatus, AgentRuntimeState, AutomationServer, agent_hook_status,
};
use crate::infrastructure::editor::InstalledEditor;
use crate::infrastructure::notifications::AgentActivitySnapshot;
use crate::infrastructure::persistence::WorkspaceRepository;
use crate::infrastructure::settings::{
    AppSettings, MAX_LEFT_SIDEBAR_WIDTH, MAX_RIGHT_SIDEBAR_WIDTH, MIN_LEFT_SIDEBAR_WIDTH,
    MIN_RIGHT_SIDEBAR_WIDTH, SettingsRepository,
};
use crate::ports::files::{FileEntry, FileEntryKind, FileSystemPort};
use crate::ports::git::{GitBranchSummary, GitPort};
use crate::ports::terminal::TerminalPort;
use crate::ports::terminal::{TerminalAgentKindSource, TerminalAgentPresence};
use crate::ui::agent_marks::{
    TERMINAL_GLYPH, agent_compact_badge, agent_sidebar_badge, agent_status_color,
};
use crate::ui::diff_view::{DiffView, DiffViewEvent};
use crate::ui::terminal::{TerminalView, TerminalViewEvent};
use crate::ui::theme::{MONO_FONT, colors};
use crate::{
    CloseTerminal, GoToTab, NewTerminalTab, NewWorkspace, NextWorkspace, PreviousWorkspace,
    ShowSettings, ToggleLeftSidebar, ToggleRightSidebar,
};

/// Titlebar chrome width when the left sidebar is fully collapsed.
const TITLEBAR_CHROME_COLLAPSED: f32 = 148.0;
/// Titlebar chrome width when the right sidebar is fully collapsed (toggle only).
const TITLEBAR_RIGHT_CHROME_COLLAPSED: f32 = 40.0;
const TITLEBAR_HEIGHT: f32 = 38.0;
/// Card padding + badge + gap beside the session text column.
const SIDEBAR_WORKSPACE_CARD_CHROME: f32 = 20.0 + 28.0 + 8.0;
/// The list inset plus the chrome inside a session card.
const LEFT_SIDEBAR_TAB_CHROME: f32 = 16.0 + SIDEBAR_WORKSPACE_CARD_CHROME;
const SIDEBAR_SPACE_HEIGHT: f32 = 32.0;
const SIDEBAR_WORKSPACE_HEIGHT: f32 = 60.0;
const SIDEBAR_GROUP_INSET: f32 = 10.0;
/// Open/close duration — short enough to feel snappy, long enough to read as motion.
const SIDEBAR_ANIM_DURATION: Duration = Duration::from_millis(160);
/// ~60 fps ticks; only runs while a sidebar is mid-animation.
const SIDEBAR_ANIM_FRAME: Duration = Duration::from_millis(16);
/// How often to refresh per-workspace branch/path metadata in the sessions sidebar.
const SIDEBAR_GIT_POLL_INTERVAL: Duration = Duration::from_secs(3);
/// IDE-style utility console height. It is intentionally compact so the main
/// terminal remains the primary surface.
const DEV_TERMINAL_HEIGHT: f32 = 260.0;

/// Cached git metadata for a workspace sidebar tab (cmux-style).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SidebarWorkspaceMeta {
    cwd: String,
    branch: Option<String>,
    ahead: usize,
    behind: usize,
    dirty: bool,
}

#[derive(Clone)]
struct PaneIdentity {
    title: String,
    detail: Option<String>,
    agent_kind: Option<String>,
    agent_state: Option<AgentRuntimeState>,
    agent_attention: Option<AgentAttention>,
    agent_model: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LeftSidebarMode {
    Sessions,
    Info,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RightSidebarMode {
    Files,
    Diff,
}

#[derive(Debug, Clone)]
pub(crate) struct ProjectFileRow {
    entry: FileEntry,
    depth: usize,
    expanded: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaletteMode {
    Commands,
    Files,
}

#[derive(Debug, Clone)]
enum PaletteAction {
    NewTerminalTab,
    OpenIde,
    ToggleDevTerminal,
    NewWorkspace,
    Split(PaneSplitDirection),
    EqualizePanes,
    TogglePaneZoom,
    ToggleGit,
    ShowSessions,
    ShowFiles,
    ShowInfo,
    ShowSettings,
    SelectWorkspace {
        project_id: Uuid,
        workspace_id: Uuid,
    },
    OpenFile(PathBuf),
}

#[derive(Debug, Clone)]
struct PaletteItem {
    label: String,
    detail: String,
    action: PaletteAction,
}

#[derive(Debug, Clone)]
enum ContextMenuKind {
    Workspace {
        project_id: Uuid,
        workspace_id: Uuid,
    },
    Pane {
        session_id: Uuid,
    },
    SidebarSpace {
        space_id: Uuid,
    },
    SidebarBackground,
}

#[derive(Debug, Clone)]
struct ContextMenuState {
    kind: ContextMenuKind,
    x: f32,
    y: f32,
}

#[derive(Debug, Clone)]
enum RenamePromptKind {
    Workspace {
        project_id: Uuid,
        workspace_id: Uuid,
    },
    Pane {
        session_id: Uuid,
    },
    CreateSpace {
        workspace_id: Uuid,
    },
    CreateEmptySpace,
    SidebarSpace {
        space_id: Uuid,
    },
}

#[derive(Debug, Clone)]
struct RenamePrompt {
    kind: RenamePromptKind,
    value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContextMenuAction {
    Rename,
    Delete,
    CreateSpace,
    DeleteSpace,
    ClosePane,
    SplitRight,
    SplitDown,
    ToggleZoom,
}

pub struct WorkspaceView {
    snapshot: WorkspaceSnapshot,
    repository: WorkspaceRepository,
    settings_repository: SettingsRepository,
    settings: AppSettings,
    launch_directory: PathBuf,
    terminal_port: Arc<dyn TerminalPort>,
    file_port: Arc<dyn FileSystemPort>,
    git_port: Arc<dyn GitPort>,
    diff_view: Entity<DiffView>,
    _diff_subscription: Subscription,
    pending_focus_session: Option<Uuid>,
    terminals: HashMap<Uuid, Entity<TerminalView>>,
    /// Per-sidebar-session utility consoles. Showing one never changes the
    /// selected terminal/tab, and switching sessions never reuses another
    /// session's PTYs.
    dev_terminals: HashMap<Uuid, DevTerminalDrawer>,
    dev_terminal_subscriptions: HashMap<Uuid, Subscription>,
    pending_focus_dev_terminal: bool,
    terminal_subscriptions: HashMap<Uuid, Subscription>,
    automation_tokens: HashMap<Uuid, Uuid>,
    automation_socket: Option<PathBuf>,
    _automation_server: Option<AutomationServer>,
    _automation_task: Option<gpui::Task<()>>,
    agent_presence: HashMap<Uuid, TerminalAgentPresence>,
    hook_agent_presence: HashMap<Uuid, HookAgentPresence>,
    /// Optional names assigned to panes from the pane context menu.
    agent_names: HashMap<Uuid, String>,
    agent_activity_seen: HashMap<Uuid, AgentActivitySnapshot>,
    agent_hook_status: Option<AgentHookStatus>,
    agent_hook_error: Option<SharedString>,
    window_is_active: bool,
    focus_handle: FocusHandle,
    left_sidebar_visible: bool,
    /// Visual open amount for the left sidebar (`0.0` closed … `1.0` open).
    left_sidebar_progress: f32,
    left_sidebar_mode: LeftSidebarMode,
    /// Per-workspace path/branch metadata shown in the sessions sidebar.
    sidebar_workspace_meta: HashMap<Uuid, SidebarWorkspaceMeta>,
    sidebar_git_request_id: u64,
    _sidebar_git_task: Option<Task<()>>,
    _sidebar_git_poll_task: Option<Task<()>>,
    expanded_directories: HashSet<PathBuf>,
    project_files: Vec<ProjectFileRow>,
    selected_file_path: Option<PathBuf>,
    file_error: Option<SharedString>,
    palette_mode: Option<PaletteMode>,
    palette_query: String,
    palette_selected: usize,
    palette_files: Vec<PathBuf>,
    settings_open: bool,
    settings_page: SettingsPage,
    context_menu: Option<ContextMenuState>,
    ide_menu_open: bool,
    installed_editors: Vec<InstalledEditor>,
    ide_icons: HashMap<&'static str, Arc<gpui::Image>>,
    rename_prompt: Option<RenamePrompt>,
    right_sidebar_visible: bool,
    /// Visual open amount for the right sidebar (`0.0` closed … `1.0` open).
    right_sidebar_progress: f32,
    right_sidebar_mode: RightSidebarMode,
    sidebar_anim_token: u64,
    _sidebar_anim_task: Option<Task<()>>,
    initial_terminal_focus_pending: bool,
    pane_resize_dirty: bool,
    sidebar_resize_dirty: bool,
    reorder_drag: Option<ReorderDrag>,
    persistence_error: Option<SharedString>,
    persist_generation: u64,
    _persist_task: Option<Task<()>>,
    files_request_id: u64,
    _files_task: Option<Task<()>>,
    files_watch: Option<files::FilesWatch>,
    palette_request_id: u64,
    _palette_task: Option<Task<()>>,
    _open_ide_task: Option<Task<()>>,
    home_directory: Option<PathBuf>,
    /// Subscribed once so system light/dark flips re-resolve the palette.
    _appearance_subscription: Option<Subscription>,
    _activation_subscription: Option<Subscription>,
    _window_bounds_subscription: Option<Subscription>,
    _release_subscription: Subscription,
    window_size_persist_generation: u64,
    _window_size_persist_task: Option<Task<()>>,
}

pub struct WorkspaceDependencies {
    pub repository: WorkspaceRepository,
    pub settings_repository: SettingsRepository,
    pub terminal_port: Arc<dyn TerminalPort>,
    pub file_port: Arc<dyn FileSystemPort>,
    pub git_port: Arc<dyn GitPort>,
}

fn lookup_sidebar_branch_summaries(
    port: Arc<dyn GitPort>,
    targets: Vec<(Uuid, PathBuf)>,
) -> Vec<(Uuid, PathBuf, Option<GitBranchSummary>)> {
    if targets.len() <= 1 {
        return targets
            .into_iter()
            .map(|(workspace_id, cwd)| {
                let summary = port.branch_summary(&cwd).ok().flatten();
                (workspace_id, cwd, summary)
            })
            .collect();
    }
    let handles: Vec<_> = targets
        .into_iter()
        .map(|(workspace_id, cwd)| {
            let port = port.clone();
            std::thread::spawn(move || {
                let summary = port.branch_summary(&cwd).ok().flatten();
                (workspace_id, cwd, summary)
            })
        })
        .collect();
    handles
        .into_iter()
        .filter_map(|handle| handle.join().ok())
        .collect()
}

impl WorkspaceView {
    pub fn new(
        dependencies: WorkspaceDependencies,
        launch_directory: PathBuf,
        focus_handle: FocusHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        let WorkspaceDependencies {
            repository,
            settings_repository,
            terminal_port,
            file_port,
            git_port,
        } = dependencies;
        let (mut snapshot, mut persistence_error) = match repository.load() {
            Ok(Some(snapshot)) => (snapshot, None),
            Ok(None) => (WorkspaceSnapshot::default(), None),
            Err(error) => (
                WorkspaceSnapshot::default(),
                Some(SharedString::from(format!(
                    "No se pudo restaurar el workspace: {error}"
                ))),
            ),
        };
        let settings = match settings_repository.load() {
            Ok(settings) => settings,
            Err(error) => {
                if persistence_error.is_none() {
                    persistence_error =
                        Some(format!("No se pudieron cargar los settings: {error}").into());
                }
                AppSettings::default()
            }
        };
        let mut snapshot_changed = snapshot.relocate_root(Path::new("/"), &launch_directory);
        if snapshot.projects.is_empty() {
            snapshot.create_workspace(&launch_directory);
            snapshot_changed = true;
        }
        if snapshot_changed && let Err(error) = repository.save(&snapshot) {
            persistence_error = Some(format!("No se pudo guardar el workspace: {error}").into());
        }

        let (automation_server, automation_socket, automation_task) =
            match AutomationServer::start() {
                Ok(server) => {
                    let socket = server.path().to_path_buf();
                    let requests = server.receiver();
                    let task = cx.spawn(async move |this, cx| {
                        while let Ok(request) = requests.recv().await {
                            if this
                                .update(cx, |this, cx| {
                                    this.handle_automation_request(request, cx);
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                    });
                    (Some(server), Some(socket), Some(task))
                }
                Err(error) => {
                    if persistence_error.is_none() {
                        persistence_error =
                            Some(format!("Automatización local no disponible: {error}").into());
                    }
                    (None, None, None)
                }
            };

        let diff_root = snapshot
            .selected_project()
            .map(|project| PathBuf::from(&project.root_path))
            .unwrap_or_else(|| launch_directory.clone());
        let diff_view = cx.new(|cx| DiffView::new(diff_root, git_port.clone(), cx));
        let diff_subscription = cx.subscribe(
            &diff_view,
            |_this, _diff_view, _event: &DiffViewEvent, cx| cx.notify(),
        );
        let (agent_hook_status, agent_hook_error) = match agent_hook_status() {
            Ok(status) => (Some(status), None),
            Err(error) => (
                None,
                Some(format!("No se pudo consultar las integraciones: {error}").into()),
            ),
        };
        let sidebar_git_poll_task = cx.spawn(async move |this, cx| {
            loop {
                Timer::after(SIDEBAR_GIT_POLL_INTERVAL).await;
                if this
                    .update(cx, |this, cx| {
                        if crate::ui::idle::should_poll_sidebar_git(
                            this.left_sidebar_visible,
                            this.left_sidebar_mode == LeftSidebarMode::Sessions,
                        ) {
                            this.refresh_sidebar_workspace_meta(cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let release_subscription = cx.on_release(|this, _| {
            if this.window_size_persist_generation > 0
                && let Err(error) = this.settings_repository.save(&this.settings)
            {
                eprintln!("No se pudieron guardar settings al cerrar: {error}");
            }
        });
        let mut view = Self {
            snapshot,
            repository,
            settings_repository,
            settings: settings.clone(),
            launch_directory,
            terminal_port,
            file_port,
            git_port,
            diff_view,
            _diff_subscription: diff_subscription,
            pending_focus_session: None,
            terminals: HashMap::new(),
            dev_terminals: HashMap::new(),
            dev_terminal_subscriptions: HashMap::new(),
            pending_focus_dev_terminal: false,
            terminal_subscriptions: HashMap::new(),
            automation_tokens: HashMap::new(),
            automation_socket,
            _automation_server: automation_server,
            _automation_task: automation_task,
            agent_presence: HashMap::new(),
            hook_agent_presence: HashMap::new(),
            agent_names: HashMap::new(),
            agent_activity_seen: HashMap::new(),
            agent_hook_status,
            agent_hook_error,
            window_is_active: true,
            focus_handle,
            left_sidebar_visible: settings.left_sidebar_visible,
            left_sidebar_progress: if settings.left_sidebar_visible {
                1.0
            } else {
                0.0
            },
            left_sidebar_mode: LeftSidebarMode::Sessions,
            sidebar_workspace_meta: HashMap::new(),
            sidebar_git_request_id: 0,
            _sidebar_git_task: None,
            _sidebar_git_poll_task: Some(sidebar_git_poll_task),
            expanded_directories: HashSet::new(),
            project_files: Vec::new(),
            selected_file_path: None,
            file_error: None,
            palette_mode: None,
            palette_query: String::new(),
            palette_selected: 0,
            palette_files: Vec::new(),
            settings_open: false,
            settings_page: SettingsPage::General,
            context_menu: None,
            ide_menu_open: false,
            installed_editors: Vec::new(),
            ide_icons: HashMap::new(),
            rename_prompt: None,
            right_sidebar_visible: settings.right_sidebar_visible,
            right_sidebar_progress: if settings.right_sidebar_visible {
                1.0
            } else {
                0.0
            },
            right_sidebar_mode: RightSidebarMode::Diff,
            sidebar_anim_token: 0,
            _sidebar_anim_task: None,
            initial_terminal_focus_pending: true,
            pane_resize_dirty: false,
            sidebar_resize_dirty: false,
            reorder_drag: None,
            persistence_error,
            persist_generation: 0,
            _persist_task: None,
            files_request_id: 0,
            _files_task: None,
            files_watch: None,
            palette_request_id: 0,
            _palette_task: None,
            _open_ide_task: None,
            home_directory: directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf()),
            _appearance_subscription: None,
            _activation_subscription: None,
            _window_bounds_subscription: None,
            _release_subscription: release_subscription,
            window_size_persist_generation: 0,
            _window_size_persist_task: None,
        };
        if settings.agent_notifications {
            crate::infrastructure::notifications::request_authorization();
        }
        // System appearance is refined on first paint via observe_window_appearance.
        view.apply_theme_preference(true, cx);
        view.reconcile_terminal_views(cx);
        view.sync_diff_root(cx);
        view.refresh_project_files(cx);
        if crate::ui::idle::should_poll_sidebar_git(
            view.left_sidebar_visible,
            view.left_sidebar_mode == LeftSidebarMode::Sessions,
        ) {
            view.refresh_sidebar_workspace_meta(cx);
        }
        view.sync_git_panel_visibility(cx);
        view
    }

    fn sync_git_panel_visibility(&self, cx: &mut Context<Self>) {
        let visible = self.right_sidebar_visible || self.right_sidebar_progress > 0.001;
        self.diff_view
            .update(cx, |diff_view, cx| diff_view.set_panel_visible(visible, cx));
    }

    fn pane_identity_with_cwd(
        &self,
        session: &SessionSnapshot,
        index: usize,
        working_directory: &str,
    ) -> PaneIdentity {
        let alias = self.agent_names.get(&session.id).map(String::as_str);
        let presence = self.resolved_agent_presence(session.id);
        PaneIdentity {
            title: tab_display_title(
                alias.or(session.agent_task_title.as_deref()),
                Some(session.title.as_str()),
                Some(working_directory),
                index,
            ),
            detail: pane_detail_title(
                alias,
                Some(session.title.as_str()),
                Some(working_directory),
                self.home_directory.as_deref(),
            ),
            agent_kind: presence.as_ref().map(|presence| presence.kind.clone()),
            agent_state: presence.as_ref().map(|presence| presence.state),
            agent_attention: presence.as_ref().and_then(|presence| presence.attention),
            agent_model: presence.and_then(|presence| presence.model),
        }
    }

    fn pane_identity(
        &self,
        session: &SessionSnapshot,
        index: usize,
        cx: &Context<Self>,
    ) -> PaneIdentity {
        let live_cwd = self
            .terminals
            .get(&session.id)
            .map(|terminal| {
                terminal
                    .read(cx)
                    .current_working_directory()
                    .to_string_lossy()
                    .into_owned()
            })
            .unwrap_or_else(|| session.working_directory.clone());
        self.pane_identity_with_cwd(session, index, &live_cwd)
    }

    fn pane_identity_by_id(&self, session_id: Uuid, cx: &Context<Self>) -> Option<PaneIdentity> {
        let session = self
            .snapshot
            .projects
            .iter()
            .flat_map(|project| project.workspaces.as_deref().unwrap_or_default())
            .flat_map(|workspace| &workspace.tabs)
            .flat_map(|tab| &tab.sessions)
            .find(|session| session.id == session_id)?;
        Some(self.pane_identity(session, 0, cx))
    }

    fn sync_diff_root(&self, cx: &mut Context<Self>) {
        let root = self.selected_live_cwd(cx);
        self.diff_view
            .update(cx, |diff_view, cx| diff_view.set_root(root, cx));
    }

    /// Active console directory: live PTY cwd when available, else snapshot / launch dir.
    fn selected_live_cwd(&self, cx: &Context<Self>) -> PathBuf {
        if let Some(session) = self.snapshot.selected_session() {
            if let Some(terminal) = self.terminals.get(&session.id) {
                return terminal.read(cx).current_working_directory();
            }
            return PathBuf::from(&session.working_directory);
        }
        self.snapshot
            .selected_project()
            .map(|project| PathBuf::from(&project.root_path))
            .unwrap_or_else(|| self.launch_directory.clone())
    }

    /// Files root follows the selected terminal cwd (not the frozen project root).
    fn project_root(&self) -> PathBuf {
        if let Some(session) = self.snapshot.selected_session() {
            return PathBuf::from(&session.working_directory);
        }
        self.snapshot
            .selected_project()
            .map(|project| PathBuf::from(&project.root_path))
            .unwrap_or_else(|| self.launch_directory.clone())
    }

    /// Live cwd for a workspace tab: prefers the terminal process, falls back to snapshot.
    fn workspace_live_cwd(&self, workspace_id: Uuid, cx: &Context<Self>) -> Option<PathBuf> {
        let workspace = self
            .snapshot
            .projects
            .iter()
            .flat_map(|project| project.workspaces.as_deref().unwrap_or_default())
            .find(|workspace| workspace.id == workspace_id)?;
        let session = workspace.primary_session()?;
        if let Some(terminal) = self.terminals.get(&session.id) {
            return Some(terminal.read(cx).current_working_directory());
        }
        Some(PathBuf::from(&session.working_directory))
    }

    /// Collects (workspace_id, cwd) for every open workspace, using live terminal cwd when available.
    fn sidebar_workspace_targets(&self, cx: &Context<Self>) -> Vec<(Uuid, PathBuf)> {
        self.snapshot
            .projects
            .iter()
            .flat_map(|project| {
                project
                    .workspaces
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .map(|workspace| {
                        let cwd = self
                            .workspace_live_cwd(workspace.id, cx)
                            .or_else(|| workspace.primary_working_directory().map(PathBuf::from))
                            .unwrap_or_else(|| PathBuf::from(&project.root_path));
                        (workspace.id, cwd)
                    })
            })
            .collect()
    }

    /// Refreshes path + branch metadata for sidebar tabs (async, non-blocking).
    fn refresh_sidebar_workspace_meta(&mut self, cx: &mut Context<Self>) {
        let targets = self.sidebar_workspace_targets(cx);
        let live_ids: HashSet<Uuid> = targets.iter().map(|(id, _)| *id).collect();
        self.sidebar_workspace_meta
            .retain(|id, _| live_ids.contains(id));

        // Apply paths immediately so `cd` updates the label without waiting for git.
        // Keep the previous branch label until the async lookup finishes (avoids flicker).
        let mut path_changed = false;
        for (workspace_id, cwd) in &targets {
            let cwd_str = cwd.to_string_lossy().into_owned();
            match self.sidebar_workspace_meta.get_mut(workspace_id) {
                Some(meta) if meta.cwd != cwd_str => {
                    meta.cwd = cwd_str;
                    path_changed = true;
                }
                Some(_) => {}
                None => {
                    self.sidebar_workspace_meta.insert(
                        *workspace_id,
                        SidebarWorkspaceMeta {
                            cwd: cwd_str,
                            branch: None,
                            ahead: 0,
                            behind: 0,
                            dirty: false,
                        },
                    );
                    path_changed = true;
                }
            }
        }
        if path_changed {
            cx.notify();
        }

        self.sidebar_git_request_id = self.sidebar_git_request_id.wrapping_add(1);
        let request_id = self.sidebar_git_request_id;
        let port = self.git_port.clone();
        let task =
            cx.background_spawn(async move { lookup_sidebar_branch_summaries(port, targets) });
        self._sidebar_git_task = Some(cx.spawn(async move |this, cx| {
            let results = task.await;
            let _ = this.update(cx, |this, cx| {
                if request_id != this.sidebar_git_request_id {
                    return;
                }
                let mut changed = false;
                for (workspace_id, cwd, summary) in results {
                    let cwd_str = cwd.to_string_lossy().into_owned();
                    // Drop stale results if the workspace already moved again.
                    if this
                        .sidebar_workspace_meta
                        .get(&workspace_id)
                        .is_some_and(|meta| meta.cwd != cwd_str)
                    {
                        continue;
                    }
                    let next = match summary {
                        Some(GitBranchSummary {
                            branch,
                            ahead,
                            behind,
                            dirty,
                        }) => SidebarWorkspaceMeta {
                            cwd: cwd_str,
                            branch: Some(branch),
                            ahead,
                            behind,
                            dirty,
                        },
                        None => SidebarWorkspaceMeta {
                            cwd: cwd_str,
                            branch: None,
                            ahead: 0,
                            behind: 0,
                            dirty: false,
                        },
                    };
                    if this.sidebar_workspace_meta.get(&workspace_id) != Some(&next) {
                        this.sidebar_workspace_meta.insert(workspace_id, next);
                        changed = true;
                    }
                }
                if changed {
                    cx.notify();
                }
            });
        }));
    }

    fn refresh_project_files(&mut self, cx: &mut Context<Self>) {
        // Snapshot cwd is updated on WorkingDirectoryChanged; enough for tree rebuilds.
        let root = self.project_root();
        self.expanded_directories.insert(root.clone());
        self.files_request_id = self.files_request_id.wrapping_add(1);
        let request_id = self.files_request_id;
        let expanded = self.expanded_directories.clone();
        let show_hidden = self.settings.show_hidden_files;
        let port = self.file_port.clone();
        let selected = self.selected_file_path.clone();
        self.sync_files_watcher(cx);
        let task = cx.background_spawn(async move {
            let mut rows = Vec::new();
            let result = collect_project_files(
                port.as_ref(),
                &root,
                &root,
                0,
                &expanded,
                show_hidden,
                &mut rows,
            );
            (result, rows, selected)
        });
        self._files_task = Some(cx.spawn(async move |this, cx| {
            let (result, rows, selected) = task.await;
            let _ = this.update(cx, |this, cx| {
                if request_id != this.files_request_id {
                    return;
                }
                match result {
                    Ok(()) => {
                        this.project_files = rows;
                        this.file_error = None;
                        if selected.as_ref().is_some_and(|path| !path.exists()) {
                            this.selected_file_path = None;
                        }
                    }
                    Err(error) => {
                        this.project_files = rows;
                        this.file_error = Some(error.to_string().into());
                    }
                }
                cx.notify();
            });
        }));
    }

    fn toggle_directory(&mut self, path: &Path, cx: &mut Context<Self>) {
        if !self.expanded_directories.remove(path) {
            self.expanded_directories.insert(path.to_path_buf());
        }
        self.selected_file_path = Some(path.to_path_buf());
        self.refresh_project_files(cx);
        cx.notify();
    }

    fn select_file_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.selected_file_path = Some(path);
        cx.notify();
    }

    fn open_context_menu(&mut self, kind: ContextMenuKind, x: f32, y: f32, cx: &mut Context<Self>) {
        self.context_menu = Some(ContextMenuState { kind, x, y });
        self.rename_prompt = None;
        cx.notify();
    }

    fn close_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.context_menu.take().is_some() {
            cx.notify();
        }
    }

    fn begin_rename_prompt(&mut self, kind: RenamePromptKind, cx: &mut Context<Self>) {
        let value = match kind {
            RenamePromptKind::Workspace {
                project_id,
                workspace_id,
            } => self
                .snapshot
                .projects
                .iter()
                .find(|project| project.id == project_id)
                .and_then(|project| {
                    project
                        .workspaces
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .find(|workspace| workspace.id == workspace_id)
                        .map(|workspace| workspace.name.clone())
                })
                .unwrap_or_default(),
            RenamePromptKind::Pane { session_id } => self
                .agent_names
                .get(&session_id)
                .cloned()
                .or_else(|| {
                    self.snapshot
                        .terminal_sessions()
                        .into_iter()
                        .find(|session| session.id == session_id)
                        .map(|session| session.title)
                })
                .unwrap_or_default(),
            RenamePromptKind::CreateSpace { .. } => String::new(),
            RenamePromptKind::CreateEmptySpace => String::new(),
            RenamePromptKind::SidebarSpace { space_id } => {
                self.snapshot
                    .sidebar_items
                    .iter()
                    .find_map(|item| match item {
                        crate::domain::workspace::SidebarItemSnapshot::Space {
                            id, name, ..
                        } if *id == space_id => Some(name.clone()),
                        _ => None,
                    })
                    .unwrap_or_default()
            }
        };
        self.context_menu = None;
        self.rename_prompt = Some(RenamePrompt { kind, value });
        self.palette_mode = None;
        self.settings_open = false;
        cx.notify();
    }

    fn confirm_rename_prompt(&mut self, cx: &mut Context<Self>) {
        let Some(prompt) = self.rename_prompt.clone() else {
            return;
        };
        let name = prompt.value.trim().to_owned();
        if name.is_empty() {
            self.persistence_error = Some("El nombre no puede estar vacío".into());
            cx.notify();
            return;
        }
        match prompt.kind {
            RenamePromptKind::Workspace {
                project_id,
                workspace_id,
            } => {
                if self
                    .snapshot
                    .rename_workspace(project_id, workspace_id, &name)
                {
                    self.rename_prompt = None;
                    self.persistence_error = None;
                    self.persist(cx);
                } else {
                    self.persistence_error = Some("No se pudo renombrar la sesión".into());
                }
            }
            RenamePromptKind::Pane { session_id } => {
                // Pane labels are intentionally independent from agent hook identity.
                let project_id = self.project_id_for_session(session_id);
                if name.len() > 48 {
                    self.persistence_error =
                        Some("El nombre del pane es demasiado largo (máx. 48)".into());
                    cx.notify();
                    return;
                }
                if let Some(project_id) = project_id
                    && let Some((existing, _)) =
                        self.agent_names.iter().find(|(other_id, other_name)| {
                            *other_name == &name
                                && **other_id != session_id
                                && self
                                    .project_id_for_session(**other_id)
                                    .is_some_and(|id| id == project_id)
                        })
                {
                    self.persistence_error = Some(
                        format!("El nombre '{name}' ya está en uso por el pane {existing}").into(),
                    );
                    cx.notify();
                    return;
                }
                self.agent_names.insert(session_id, name);
                self.rename_prompt = None;
                self.persistence_error = None;
            }
            RenamePromptKind::CreateSpace { workspace_id } => {
                if self
                    .snapshot
                    .create_sidebar_space(workspace_id, &name)
                    .is_some()
                {
                    self.rename_prompt = None;
                    self.persistence_error = None;
                    self.persist(cx);
                }
            }
            RenamePromptKind::CreateEmptySpace => {
                self.snapshot.create_empty_sidebar_space(&name);
                self.rename_prompt = None;
                self.persistence_error = None;
                self.persist(cx);
            }
            RenamePromptKind::SidebarSpace { space_id } => {
                if self.snapshot.rename_sidebar_space(space_id, &name) {
                    self.rename_prompt = None;
                    self.persistence_error = None;
                    self.persist(cx);
                }
            }
        }
        cx.notify();
    }

    fn run_context_menu_action(
        &mut self,
        action: ContextMenuAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.context_menu.clone() else {
            return;
        };
        self.context_menu = None;
        match (menu.kind, action) {
            (
                ContextMenuKind::Workspace {
                    project_id,
                    workspace_id,
                },
                ContextMenuAction::Rename,
            ) => {
                self.begin_rename_prompt(
                    RenamePromptKind::Workspace {
                        project_id,
                        workspace_id,
                    },
                    cx,
                );
            }
            (
                ContextMenuKind::Workspace {
                    project_id,
                    workspace_id,
                },
                ContextMenuAction::Delete,
            ) => {
                if self.snapshot.close_workspace(project_id, workspace_id) {
                    self.reconcile_terminal_views(cx);
                    self.sync_diff_root(cx);
                    self.refresh_project_files(cx);
                    self.persist(cx);
                    self.focus_selected_terminal(window, cx);
                }
            }
            (ContextMenuKind::Workspace { workspace_id, .. }, ContextMenuAction::CreateSpace) => {
                self.begin_rename_prompt(RenamePromptKind::CreateSpace { workspace_id }, cx);
            }
            (ContextMenuKind::SidebarBackground, ContextMenuAction::CreateSpace) => {
                self.begin_rename_prompt(RenamePromptKind::CreateEmptySpace, cx);
            }
            (ContextMenuKind::SidebarSpace { space_id }, ContextMenuAction::Rename) => {
                self.begin_rename_prompt(RenamePromptKind::SidebarSpace { space_id }, cx);
            }
            (ContextMenuKind::SidebarSpace { space_id }, ContextMenuAction::DeleteSpace) => {
                if self.snapshot.remove_sidebar_space(space_id) {
                    self.persist(cx);
                }
            }
            (ContextMenuKind::Pane { session_id }, ContextMenuAction::Rename) => {
                self.begin_rename_prompt(RenamePromptKind::Pane { session_id }, cx);
            }
            (ContextMenuKind::Pane { session_id }, ContextMenuAction::ClosePane) => {
                if self.snapshot.close_terminal(session_id) {
                    self.agent_names.remove(&session_id);
                    self.reconcile_terminal_views(cx);
                    self.sync_diff_root(cx);
                    self.refresh_project_files(cx);
                    self.persist(cx);
                    self.focus_selected_terminal(window, cx);
                }
            }
            (ContextMenuKind::Pane { session_id }, ContextMenuAction::SplitRight) => {
                if self.snapshot.select_terminal_global(session_id) {
                    self.split_pane(PaneSplitDirection::Right, window, cx);
                }
            }
            (ContextMenuKind::Pane { session_id }, ContextMenuAction::SplitDown) => {
                if self.snapshot.select_terminal_global(session_id) {
                    self.split_pane(PaneSplitDirection::Down, window, cx);
                }
            }
            (ContextMenuKind::Pane { session_id }, ContextMenuAction::ToggleZoom) => {
                if self.snapshot.select_terminal_global(session_id)
                    && self.snapshot.toggle_selected_pane_zoom()
                {
                    self.sync_terminal_surface_visibility(cx);
                    self.persist(cx);
                    self.focus_selected_terminal(window, cx);
                    cx.notify();
                }
            }
            _ => cx.notify(),
        }
    }

    fn toggle_diff_panel(&mut self, cx: &mut Context<Self>) {
        self.set_right_sidebar_visible(!self.right_sidebar_visible, true, cx);
        if self.right_sidebar_visible {
            self.sync_diff_root(cx);
        }
    }

    /// Desired open/closed state for the left sidebar, with a light width animation.
    fn set_left_sidebar_visible(&mut self, visible: bool, persist: bool, cx: &mut Context<Self>) {
        if self.left_sidebar_visible == visible {
            // Caller may have changed mode/content; repaint without restarting motion.
            cx.notify();
            return;
        }
        self.left_sidebar_visible = visible;
        self.settings.left_sidebar_visible = visible;
        if persist {
            self.persist_settings(cx);
        }
        self.start_sidebar_animation(cx);
    }

    /// Desired open/closed state for the right sidebar, with a light width animation.
    fn set_right_sidebar_visible(&mut self, visible: bool, persist: bool, cx: &mut Context<Self>) {
        if !visible {
            self.diff_view
                .update(cx, |diff, cx| diff.set_review_expanded(false, cx));
        }
        if self.right_sidebar_visible == visible {
            self.sync_files_watcher(cx);
            cx.notify();
            return;
        }
        self.right_sidebar_visible = visible;
        self.settings.right_sidebar_visible = visible;
        self.sync_git_panel_visibility(cx);
        if persist {
            self.persist_settings(cx);
        }
        self.start_sidebar_animation(cx);
        self.sync_files_watcher(cx);
    }

    /// Interpolates left/right sidebar progress toward their targets (~160ms ease-out).
    /// Cheap: only schedules frames while mid-animation; drops previous task on restart.
    fn start_sidebar_animation(&mut self, cx: &mut Context<Self>) {
        let left_to = if self.left_sidebar_visible { 1.0 } else { 0.0 };
        let right_to = if self.right_sidebar_visible { 1.0 } else { 0.0 };
        let left_from = self.left_sidebar_progress;
        let right_from = self.right_sidebar_progress;

        if (left_from - left_to).abs() < 0.001 && (right_from - right_to).abs() < 0.001 {
            self.left_sidebar_progress = left_to;
            self.right_sidebar_progress = right_to;
            self._sidebar_anim_task = None;
            cx.notify();
            return;
        }

        let token = self.sidebar_anim_token.wrapping_add(1);
        self.sidebar_anim_token = token;
        let started = Instant::now();

        self._sidebar_anim_task = Some(cx.spawn(async move |this, cx| {
            loop {
                Timer::after(SIDEBAR_ANIM_FRAME).await;
                let cont = this
                    .update(cx, |this, cx| {
                        if this.sidebar_anim_token != token {
                            return false;
                        }
                        let t = (started.elapsed().as_secs_f32()
                            / SIDEBAR_ANIM_DURATION.as_secs_f32())
                        .min(1.0);
                        let eased = ease_out_cubic(t);
                        this.left_sidebar_progress = left_from + (left_to - left_from) * eased;
                        this.right_sidebar_progress = right_from + (right_to - right_from) * eased;
                        if t >= 1.0 {
                            this.left_sidebar_progress = left_to;
                            this.right_sidebar_progress = right_to;
                            this._sidebar_anim_task = None;
                            cx.notify();
                            return false;
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !cont {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn reconcile_terminal_views(&mut self, cx: &mut Context<Self>) {
        let sessions = self.snapshot.terminal_sessions();
        let live_ids: HashSet<_> = sessions.iter().map(|session| session.id).collect();

        let stale_ids: Vec<_> = self
            .terminals
            .keys()
            .filter(|session_id| !live_ids.contains(session_id))
            .copied()
            .collect();
        for session_id in stale_ids {
            if let Some(terminal) = self.terminals.remove(&session_id) {
                terminal.read(cx).shutdown();
            }
            self.terminal_subscriptions.remove(&session_id);
            self.automation_tokens.remove(&session_id);
            self.agent_presence.remove(&session_id);
            self.hook_agent_presence.remove(&session_id);
            self.agent_names.remove(&session_id);
            self.agent_activity_seen.remove(&session_id);
        }

        self.prune_dev_terminals(cx);

        for session in sessions {
            if self.terminals.contains_key(&session.id) {
                continue;
            }
            let session_id = session.id;
            let terminal_port = self.terminal_port.clone();
            let working_directory = PathBuf::from(session.working_directory);
            let title = session.title;
            let token = *self
                .automation_tokens
                .entry(session_id)
                .or_insert_with(Uuid::new_v4);
            let mut environment = HashMap::new();
            environment.insert("VIBRA_PANE_ID".into(), session_id.to_string());
            environment.insert("VIBRA_AUTOMATION_TOKEN".into(), token.to_string());
            if let Ok(executable) = std::env::current_exe() {
                environment.insert(
                    "VIBRA_CLI".into(),
                    executable.to_string_lossy().into_owned(),
                );
            }
            if let Some(socket) = &self.automation_socket {
                environment.insert(
                    "VIBRA_AUTOMATION_SOCKET".into(),
                    socket.to_string_lossy().into_owned(),
                );
            }
            let terminal = cx.new(|cx| {
                TerminalView::new_with_environment(
                    session_id,
                    title,
                    Path::new(&working_directory),
                    terminal_port,
                    environment,
                    cx,
                )
            });
            terminal.update(cx, |terminal, cx| {
                terminal.apply_font_size(self.settings.terminal_font_size, cx);
            });
            let subscription = cx.subscribe(
                &terminal,
                |this, _terminal, event: &TerminalViewEvent, cx| {
                    this.handle_terminal_view_event(event, cx);
                },
            );
            self.terminals.insert(session_id, terminal);
            self.terminal_subscriptions.insert(session_id, subscription);
        }
        self.sync_terminal_surface_visibility(cx);
    }

    fn visible_terminal_ids(&self) -> HashSet<Uuid> {
        self.snapshot.painted_session_ids()
    }

    fn sync_terminal_surface_visibility(&self, cx: &mut Context<Self>) {
        let visible = self.visible_terminal_ids();
        for (session_id, terminal) in &self.terminals {
            let shown = visible.contains(session_id);
            terminal.update(cx, |terminal, _| terminal.set_surface_visible(shown));
        }
        let current_workspace = self.current_workspace_id();
        for (workspace_id, drawer) in &self.dev_terminals {
            let drawer_shown = drawer.visible && current_workspace == Some(*workspace_id);
            for terminal in &drawer.terminals {
                let session_id = terminal.read(cx).session_id();
                let shown = drawer_shown && session_id == drawer.selected_id;
                terminal.update(cx, |terminal, _| terminal.set_surface_visible(shown));
            }
        }
    }

    fn handle_terminal_view_event(&mut self, event: &TerminalViewEvent, cx: &mut Context<Self>) {
        match event {
            TerminalViewEvent::TitleChanged { session_id, title } => {
                if self.snapshot.update_session_title(*session_id, title) {
                    self.persist(cx);
                } else if self.is_dev_terminal(*session_id, cx) {
                    cx.notify();
                }
            }
            TerminalViewEvent::WorkingDirectoryChanged { session_id, path } => {
                let is_selected = self
                    .snapshot
                    .selected_session()
                    .is_some_and(|session| session.id == *session_id);
                let previous_files_root = is_selected.then(|| self.project_root());
                let changed = self
                    .snapshot
                    .update_session_working_directory(*session_id, path);
                if is_selected {
                    self.diff_view.update(cx, |diff_view, cx| {
                        diff_view.set_root(path.clone(), cx);
                    });
                    // Keep Files rooted on the live console directory.
                    let new_root = self.project_root();
                    if previous_files_root.as_ref() != Some(&new_root) {
                        self.expanded_directories.retain(|entry| {
                            entry.starts_with(&new_root) || new_root.starts_with(entry)
                        });
                        self.expanded_directories.insert(new_root);
                        self.refresh_project_files(cx);
                    }
                }
                // Path/branch/title in the sessions sidebar follow the live cwd.
                self.refresh_sidebar_workspace_meta(cx);
                if changed {
                    self.persist(cx);
                } else {
                    cx.notify();
                }
            }
            TerminalViewEvent::Exited {
                session_id,
                code: _code,
            } => {
                self.agent_presence.remove(session_id);
                self.hook_agent_presence.remove(session_id);
                self.agent_names.remove(session_id);
                self.publish_agent_activity(*session_id);
                cx.notify();
            }
            TerminalViewEvent::ContextMenuRequested { session_id, x, y } => {
                let session_id = *session_id;
                let _ = self.snapshot.select_terminal_global(session_id);
                self.open_context_menu(ContextMenuKind::Pane { session_id }, *x, *y, cx);
            }
            TerminalViewEvent::AgentPresenceChanged {
                session_id,
                presence,
            } => {
                let previous = self.agent_presence.get(session_id);
                let occupant_changed = match (previous, presence) {
                    (Some(previous), Some(current)) => {
                        !previous.kind.eq_ignore_ascii_case(&current.kind)
                            || (previous.kind_source == TerminalAgentKindSource::Process
                                && current.kind_source != TerminalAgentKindSource::Process)
                            || matches!(
                                (previous.process_id, current.process_id),
                                (Some(left), Some(right)) if left != right
                            )
                    }
                    (Some(_), None) => true,
                    _ => false,
                };
                let definitive_exit = previous.is_some_and(|previous| {
                    previous.kind_source == TerminalAgentKindSource::Process
                }) || !self.hook_agent_presence.contains_key(session_id);
                if occupant_changed && (presence.is_some() || definitive_exit) {
                    self.agent_names.remove(session_id);
                    self.hook_agent_presence.remove(session_id);
                }
                if let Some(presence) = presence {
                    self.agent_presence.insert(*session_id, presence.clone());
                } else {
                    self.agent_presence.remove(session_id);
                }
                self.publish_agent_activity(*session_id);
                cx.notify();
            }
            TerminalViewEvent::FontSizeChanged { size } => {
                self.set_terminal_font_size(*size, cx);
            }
        }
    }

    fn project_id_for_session(&self, session_id: Uuid) -> Option<Uuid> {
        self.snapshot.projects.iter().find_map(|project| {
            project
                .workspaces
                .as_deref()
                .unwrap_or_default()
                .iter()
                .flat_map(|workspace| &workspace.tabs)
                .flat_map(|tab| &tab.sessions)
                .any(|session| session.id == session_id)
                .then_some(project.id)
        })
    }

    fn focus_selected_terminal(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(terminal) = self
            .snapshot
            .selected_session()
            .and_then(|session| self.terminals.get(&session.id))
        {
            terminal.read(cx).focus_handle(cx).focus(window);
        }
    }

    fn focus_terminal(&self, session_id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(terminal) = self.terminals.get(&session_id) {
            terminal.read(cx).focus_handle(cx).focus(window);
        }
    }

    fn select_terminal(&mut self, session_id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        if self.snapshot.select_terminal(session_id) {
            self.sync_terminal_surface_visibility(cx);
            self.sync_diff_root(cx);
            self.refresh_project_files(cx);
            self.refresh_sidebar_workspace_meta(cx);
            self.persist(cx);
        }
        self.focus_terminal(session_id, window, cx);
    }

    fn capture_selected_working_directory(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.snapshot.selected_session().map(|session| session.id) else {
            return;
        };
        let Some(terminal) = self.terminals.get(&session_id) else {
            return;
        };
        let path = terminal.read(cx).current_working_directory();
        self.snapshot
            .update_session_working_directory(session_id, &path);
    }

    fn go_to_tab(&mut self, action: &GoToTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette_mode.is_some() || self.settings_open || self.rename_prompt.is_some() {
            return;
        }
        if self.snapshot.select_tab_number(action.index) {
            self.sync_terminal_surface_visibility(cx);
            self.sync_diff_root(cx);
            self.refresh_project_files(cx);
            self.refresh_sidebar_workspace_meta(cx);
            self.persist(cx);
        }
        self.focus_selected_terminal(window, cx);
    }

    fn reorder_tab(
        &mut self,
        tab_id: Uuid,
        before_tab_id: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reorder_drag = None;
        if self.snapshot.move_tab(tab_id, before_tab_id) {
            self.sync_terminal_surface_visibility(cx);
            self.sync_diff_root(cx);
            self.refresh_project_files(cx);
            self.refresh_sidebar_workspace_meta(cx);
            self.persist(cx);
            self.focus_selected_terminal(window, cx);
        }
        cx.notify();
    }

    fn reorder_sidebar_workspace(
        &mut self,
        workspace_id: Uuid,
        before_workspace_id: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reorder_drag = None;
        if self
            .snapshot
            .move_workspace(workspace_id, before_workspace_id)
        {
            self.persist(cx);
            self.focus_selected_terminal(window, cx);
        }
        cx.notify();
    }

    fn reorder_sidebar_workspace_relative(
        &mut self,
        workspace_id: Uuid,
        target_id: Uuid,
        place_after: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reorder_drag = None;
        if self
            .snapshot
            .move_workspace_relative(workspace_id, target_id, place_after)
        {
            self.persist(cx);
            self.focus_selected_terminal(window, cx);
        }
        cx.notify();
    }

    fn left_sidebar_width(&self) -> f32 {
        self.settings.left_sidebar_width
    }

    fn right_sidebar_width(&self) -> f32 {
        self.settings.right_sidebar_width
    }

    fn left_sidebar_tab_text_width(&self) -> f32 {
        (self.left_sidebar_width() - LEFT_SIDEBAR_TAB_CHROME).max(80.0)
    }

    fn set_sidebar_width(&mut self, edge: SidebarResizeEdge, width: f32, cx: &mut Context<Self>) {
        let width = match edge {
            SidebarResizeEdge::Left => width.clamp(MIN_LEFT_SIDEBAR_WIDTH, MAX_LEFT_SIDEBAR_WIDTH),
            SidebarResizeEdge::Right => {
                width.clamp(MIN_RIGHT_SIDEBAR_WIDTH, MAX_RIGHT_SIDEBAR_WIDTH)
            }
        };
        let current = match edge {
            SidebarResizeEdge::Left => self.settings.left_sidebar_width,
            SidebarResizeEdge::Right => self.settings.right_sidebar_width,
        };
        if (current - width).abs() < 0.5 {
            return;
        }
        match edge {
            SidebarResizeEdge::Left => self.settings.left_sidebar_width = width,
            SidebarResizeEdge::Right => self.settings.right_sidebar_width = width,
        }
        self.sidebar_resize_dirty = true;
        cx.notify();
    }

    fn on_sidebar_resize_move(
        &mut self,
        event: &DragMoveEvent<SidebarResizeEdge>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let edge = *event.drag(cx);
        let x: f32 = event.event.position.x.into();
        let left: f32 = event.bounds.left().into();
        let right: f32 = event.bounds.right().into();
        let width = match edge {
            SidebarResizeEdge::Left => x - left,
            SidebarResizeEdge::Right => right - x,
        };
        self.set_sidebar_width(edge, width, cx);
    }

    fn sidebar_resize_handle(
        &self,
        id: &'static str,
        edge: SidebarResizeEdge,
        _cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let docked_end = matches!(edge, SidebarResizeEdge::Right);
        div()
            .id(id)
            .absolute()
            .top_0()
            .bottom_0()
            .when(docked_end, |handle| handle.left_0())
            .when(!docked_end, |handle| handle.right_0())
            .w(px(8.0))
            .cursor_ew_resize()
            .on_drag(edge, |edge, _, _, cx| {
                let _ = edge;
                cx.new(|_| SidebarResizeDragView)
            })
    }

    fn persist(&mut self, cx: &mut Context<Self>) {
        self.persist_generation = self.persist_generation.wrapping_add(1);
        let generation = self.persist_generation;
        self._persist_task = Some(cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(400)).await;
            let _ = this.update(cx, |this, cx| {
                if this.persist_generation != generation {
                    return;
                }
                this.flush_persist(cx);
            });
        }));
    }

    fn flush_persist(&mut self, cx: &mut Context<Self>) {
        self.persistence_error = self
            .repository
            .save(&self.snapshot)
            .err()
            .map(|error| SharedString::from(format!("No se pudo guardar: {error}")));
        cx.notify();
    }

    fn persist_settings(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = self.settings_repository.save(&self.settings) {
            self.persistence_error =
                Some(format!("No se pudieron guardar settings: {error}").into());
        }
        cx.notify();
    }

    fn ensure_activation_subscription(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self._activation_subscription.is_some() {
            return;
        }
        self.window_is_active = window.is_window_active();
        self._activation_subscription =
            Some(cx.observe_window_activation(window, |this, window, _cx| {
                this.window_is_active = window.is_window_active();
            }));
    }

    fn ensure_window_bounds_subscription(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self._window_bounds_subscription.is_some() {
            return;
        }
        self._window_bounds_subscription =
            Some(cx.observe_window_bounds(window, |this, window, cx| {
                let size = window.window_bounds().get_bounds().size;
                let width: f32 = size.width.into();
                let height: f32 = size.height.into();
                if !this.settings.set_window_size(width, height) {
                    return;
                }
                this.window_size_persist_generation =
                    this.window_size_persist_generation.wrapping_add(1);
                let generation = this.window_size_persist_generation;
                this._window_size_persist_task = Some(cx.spawn(async move |this, cx| {
                    Timer::after(Duration::from_millis(400)).await;
                    let _ = this.update(cx, |this, cx| {
                        if this.window_size_persist_generation == generation {
                            this.persist_settings(cx);
                        }
                    });
                }));
            }));
    }

    fn new_workspace(&mut self, _: &NewWorkspace, window: &mut Window, cx: &mut Context<Self>) {
        self.open_workspace_in_current_directory(window, cx);
    }

    fn new_terminal_tab(
        &mut self,
        _: &NewTerminalTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_terminal_tab_in_current_directory(window, cx);
    }

    fn current_workspace_id(&self) -> Option<Uuid> {
        self.snapshot
            .selected_workspace()
            .map(|workspace| workspace.id)
    }

    fn open_workspace_in_current_directory(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = self.selected_live_cwd(cx);
        self.snapshot.create_workspace(&cwd);
        self.reconcile_terminal_views(cx);
        self.sync_diff_root(cx);
        self.refresh_project_files(cx);
        self.refresh_sidebar_workspace_meta(cx);
        self.persist(cx);
        self.focus_selected_terminal(window, cx);
    }

    fn open_terminal_tab_in_current_directory(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cwd = self.selected_live_cwd(cx).to_string_lossy().into_owned();
        if self
            .snapshot
            .create_terminal_tab_with_options(true, Some(cwd))
            .is_some()
        {
            self.reconcile_terminal_views(cx);
            self.sync_diff_root(cx);
            self.refresh_project_files(cx);
            self.refresh_sidebar_workspace_meta(cx);
            self.persist(cx);
            self.focus_selected_terminal(window, cx);
        }
    }

    fn close_terminal(&mut self, _: &CloseTerminal, window: &mut Window, cx: &mut Context<Self>) {
        if self.snapshot.close_selected_terminal() {
            self.reconcile_terminal_views(cx);
            self.sync_diff_root(cx);
            self.refresh_project_files(cx);
            self.refresh_sidebar_workspace_meta(cx);
            self.persist(cx);
            self.focus_selected_terminal(window, cx);
        }
    }

    fn toggle_left_sidebar(
        &mut self,
        _: &ToggleLeftSidebar,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.left_sidebar_visible {
            self.left_sidebar_mode = LeftSidebarMode::Sessions;
        }
        self.set_left_sidebar_visible(!self.left_sidebar_visible, true, cx);
    }

    fn show_settings(&mut self, _: &ShowSettings, _: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open {
            self.close_settings(cx);
        } else {
            self.open_settings(cx);
        }
    }

    fn toggle_right_sidebar(
        &mut self,
        _: &ToggleRightSidebar,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_diff_panel(cx);
    }

    fn previous_workspace(
        &mut self,
        _: &PreviousWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.snapshot.cycle_workspace(-1) {
            self.apply_workspace_selection_change(window, cx);
        }
    }

    fn next_workspace(&mut self, _: &NextWorkspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.snapshot.cycle_workspace(1) {
            self.apply_workspace_selection_change(window, cx);
        }
    }

    fn select_workspace(
        &mut self,
        project_id: Uuid,
        workspace_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.snapshot.select_workspace(project_id, workspace_id) {
            self.apply_workspace_selection_change(window, cx);
        }
    }

    fn select_tab(&mut self, tab_id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        if self.snapshot.select_tab(tab_id) {
            self.apply_workspace_selection_change(window, cx);
        }
    }

    fn apply_workspace_selection_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_terminal_surface_visibility(cx);
        self.sync_diff_root(cx);
        self.refresh_project_files(cx);
        self.refresh_sidebar_workspace_meta(cx);
        self.persist(cx);
        self.focus_selected_terminal(window, cx);
    }

    fn sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.left_sidebar_mode {
            LeftSidebarMode::Sessions => self.sessions_sidebar_content(cx),
            LeftSidebarMode::Info => self.info_sidebar_content(cx),
        };
        let full_width = self.left_sidebar_width();
        let width = full_width * self.left_sidebar_progress;
        let show_handle = self.left_sidebar_progress > 0.99;
        clipped_width_panel(width, full_width, colors().sidebar, content).when(
            show_handle,
            |sidebar| {
                sidebar.child(self.sidebar_resize_handle(
                    "resize-left-sidebar",
                    SidebarResizeEdge::Left,
                    cx,
                ))
            },
        )
    }

    fn sessions_sidebar_content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let sidebar_entries = self.snapshot.sidebar_entries();
        let workspace_count = self.snapshot.workspace_entries().len();
        let has_spaces = sidebar_entries
            .iter()
            .any(|entry| matches!(entry, SidebarEntry::Space { .. }));
        // A lone workspace still needs to be draggable when it can be moved into
        // (or out of) a sidebar space.
        let can_reorder = workspace_count > 1 || has_spaces;
        let dragging_workspace = match self.reorder_drag {
            Some(ReorderDrag::SidebarWorkspace(id)) if cx.has_active_drag() => Some(id),
            _ => None,
        };
        let sidebar_identities: HashMap<Uuid, (Option<PaneIdentity>, Option<PaneIdentity>)> = self
            .snapshot
            .projects
            .iter()
            .flat_map(|project| project.workspaces.as_deref().unwrap_or_default())
            .map(|workspace| {
                let primary = workspace
                    .primary_session()
                    .map(|session| self.pane_identity(session, 0, cx));
                let active_agent = workspace
                    .tabs
                    .iter()
                    .flat_map(|tab| tab.sessions.iter().enumerate())
                    .filter_map(|(index, session)| {
                        let identity = self.pane_identity(session, index, cx);
                        identity.agent_kind.is_some().then(|| {
                            (
                                sidebar_agent_priority(
                                    identity.agent_state,
                                    identity.agent_attention,
                                ),
                                identity,
                            )
                        })
                    })
                    .max_by_key(|(priority, _)| *priority)
                    .map(|(_, identity)| identity);
                (workspace.id, (primary, active_agent))
            })
            .collect();
        let meta_by_workspace = self.sidebar_workspace_meta.clone();
        let mut panel = div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(colors().sidebar)
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    let x: f32 = event.position.x.into();
                    let y: f32 = event.position.y.into();
                    this.open_context_menu(ContextMenuKind::SidebarBackground, x, y, cx);
                    cx.stop_propagation();
                }),
            );

        if workspace_count == 0 {
            panel = panel.child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .px_5()
                    .gap_3()
                    .child(
                        div()
                            .size(px(42.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(10.0))
                            .bg(colors().elevated)
                            .font_family(MONO_FONT)
                            .text_size(px(11.0))
                            .text_color(colors().muted)
                            .child(TERMINAL_GLYPH),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(colors().muted)
                            .child("Sin sesiones"),
                    )
                    .child(
                        div()
                            .text_center()
                            .text_size(px(10.5))
                            .text_color(colors().subtle)
                            .child("Usa ⌘N para crear una"),
                    ),
            );
        } else {
            panel = panel.child(
                div()
                    .id("workspace-list")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .p_2()
                    .children(sidebar_entries.into_iter().map(|sidebar_entry| {
                        let (entry, space_id) = match sidebar_entry {
                            SidebarEntry::Workspace { entry, space_id } => (entry, space_id),
                            SidebarEntry::Space {
                                id,
                                name,
                                collapsed,
                                workspace_count,
                            } => {
                                let section_name = name.to_uppercase();
                                return div()
                                    .id(SharedString::from(format!("sidebar-space-{id}")))
                                    .h(px(SIDEBAR_SPACE_HEIGHT))
                                    .w_full()
                                    .flex_none()
                                    .relative()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .px(px(6.0))
                                    .mt(px(8.0))
                                    .cursor_pointer()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            if this.snapshot.toggle_sidebar_space(id) {
                                                this.persist(cx);
                                            }
                                        }),
                                    )
                                    .on_mouse_down(
                                        MouseButton::Right,
                                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                            let x: f32 = event.position.x.into();
                                            let y: f32 = event.position.y.into();
                                            this.open_context_menu(
                                                ContextMenuKind::SidebarSpace { space_id: id },
                                                x,
                                                y,
                                                cx,
                                            );
                                            cx.stop_propagation();
                                        }),
                                    )
                                    .can_drop(move |value, _, _| {
                                        value
                                            .downcast_ref::<SidebarWorkspaceDrag>()
                                            .is_some_and(|drag| drag.source_space_id != Some(id))
                                    })
                                    .drag_over::<SidebarWorkspaceDrag>(|style, _, _, _| {
                                        style
                                            .border_1()
                                            .border_color(colors().accent)
                                            .bg(colors().selection)
                                    })
                                    .on_drop(cx.listener(
                                        move |this, drag: &SidebarWorkspaceDrag, _, cx| {
                                            this.reorder_drag = None;
                                            if this
                                                .snapshot
                                                .move_workspace_to_space(drag.workspace_id, id)
                                            {
                                                this.persist(cx);
                                            }
                                        },
                                    ))
                                    .child(
                                        div()
                                            .size(px(16.0))
                                            .flex_none()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(
                                                svg()
                                                    .path("chrome-icons/chevron-right.svg")
                                                    .size(px(9.0))
                                                    .text_color(colors().subtle)
                                                    .when(!collapsed, |icon| {
                                                        icon.with_transformation(
                                                            Transformation::rotate(radians(
                                                                std::f32::consts::FRAC_PI_2,
                                                            )),
                                                        )
                                                    }),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .min_w(px(0.0))
                                            .max_w(px(130.0))
                                            .truncate()
                                            .text_size(px(9.5))
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .text_color(colors().muted)
                                            .child(section_name),
                                    )
                                    .child(div().h(px(1.0)).flex_1().bg(colors().border_subtle))
                                    .child(
                                        div()
                                            .flex_none()
                                            .font_family(MONO_FONT)
                                            .text_size(px(8.5))
                                            .text_color(colors().subtle)
                                            .child(workspace_count.to_string()),
                                    )
                                    .into_any_element();
                            }
                        };
                        let grouped = space_id.is_some();
                        let group_inset = if grouped { SIDEBAR_GROUP_INSET } else { 0.0 };
                        let item_width = self.left_sidebar_width() - 16.0 - group_inset;
                        let project_id = entry.project_id;
                        let workspace_id = entry.workspace_id;
                        let selected = entry.is_selected;
                        let (primary_identity, active_agent_identity) = sidebar_identities
                            .get(&workspace_id)
                            .cloned()
                            .unwrap_or((None, None));
                        let agent_identity =
                            active_agent_identity.or_else(|| primary_identity.clone());
                        let meta = meta_by_workspace.get(&workspace_id);
                        let cwd = meta
                            .map(|m| m.cwd.as_str())
                            .unwrap_or(entry.working_directory.as_str());
                        let path_label = format_sidebar_path(cwd, self.home_directory.as_deref());
                        // The automatic workspace title follows the selected pane identity.
                        // A manually renamed workspace remains authoritative.
                        let title_label = if entry.title_is_manual {
                            entry.workspace_name.clone()
                        } else {
                            primary_identity
                                .as_ref()
                                .map(|identity| identity.title.clone())
                                .unwrap_or_else(|| entry.workspace_name.clone())
                        };
                        // Keep the workspace name first; the following rows summarize the
                        // active agent and repository location without hiding branch state.
                        let branch_label = meta.and_then(format_sidebar_branch);
                        let appearance = sidebar_workspace_appearance(
                            selected,
                            meta.map(|m| m.dirty).unwrap_or(false),
                            meta.map(|m| m.behind).unwrap_or_default(),
                        );
                        let agent_color = agent_status_color(
                            agent_identity
                                .as_ref()
                                .and_then(|identity| identity.agent_state),
                            agent_identity
                                .as_ref()
                                .and_then(|identity| identity.agent_attention),
                        )
                        .unwrap_or(appearance.agent_fallback);
                        let agent_label = sidebar_agent_line(
                            agent_identity
                                .as_ref()
                                .and_then(|identity| identity.agent_kind.as_deref()),
                            agent_identity
                                .as_ref()
                                .and_then(|identity| identity.agent_model.as_deref()),
                            agent_identity
                                .as_ref()
                                .and_then(|identity| identity.agent_state),
                            agent_identity
                                .as_ref()
                                .and_then(|identity| identity.agent_attention),
                        );
                        let location_label =
                            sidebar_location_line(branch_label.as_deref(), &path_label);
                        let location_color = if branch_label.is_some() {
                            appearance.branch
                        } else {
                            appearance.path
                        };
                        let drag = SidebarWorkspaceDrag {
                            workspace_id,
                            source_space_id: space_id,
                            title: title_label.clone(),
                            branch: branch_label.clone(),
                            path: path_label.clone(),
                            selected,
                            dirty: meta.map(|m| m.dirty).unwrap_or(false),
                            behind: meta.map(|m| m.behind).unwrap_or_default(),
                            agent_kind: agent_identity
                                .as_ref()
                                .and_then(|identity| identity.agent_kind.clone()),
                            agent_state: agent_identity
                                .as_ref()
                                .and_then(|identity| identity.agent_state),
                            agent_attention: agent_identity
                                .as_ref()
                                .and_then(|identity| identity.agent_attention),
                            agent_model: agent_identity
                                .as_ref()
                                .and_then(|identity| identity.agent_model.clone()),
                            width: item_width,
                        };
                        let is_source = dragging_workspace == Some(workspace_id);
                        // Explicit text width avoids flex+truncate collapsing labels to "…".
                        div()
                            .id(SharedString::from(format!("workspace-{workspace_id}")))
                            .h(px(SIDEBAR_WORKSPACE_HEIGHT))
                            .w(px(item_width))
                            .ml(px(group_inset))
                            .relative()
                            .mb(px(2.0))
                            .px(px(10.0))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .rounded(px(7.0))
                            .when(can_reorder, |item| item.cursor_move())
                            .when(!can_reorder, |item| item.cursor_pointer())
                            .bg(appearance.background)
                            .border_1()
                            .border_color(appearance.border)
                            .hover(|item| item.bg(colors().hover))
                            .active(|item| item.opacity(0.82))
                            .when(is_source, |item| item.opacity(0.65))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    if can_reorder {
                                        this.reorder_drag =
                                            Some(ReorderDrag::SidebarWorkspace(workspace_id));
                                    }
                                    this.close_context_menu(cx);
                                    this.select_workspace(project_id, workspace_id, window, cx);
                                }),
                            )
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                    let x: f32 = event.position.x.into();
                                    let y: f32 = event.position.y.into();
                                    this.open_context_menu(
                                        ContextMenuKind::Workspace {
                                            project_id,
                                            workspace_id,
                                        },
                                        x,
                                        y,
                                        cx,
                                    );
                                    cx.stop_propagation();
                                }),
                            )
                            .when(can_reorder, |item| {
                                item.on_drag(drag, |drag, _, _, cx| {
                                    cx.new(|_| SidebarWorkspaceDragView {
                                        title: drag.title.clone(),
                                        branch: drag.branch.clone(),
                                        path: drag.path.clone(),
                                        selected: drag.selected,
                                        dirty: drag.dirty,
                                        behind: drag.behind,
                                        agent_kind: drag.agent_kind.clone(),
                                        agent_state: drag.agent_state,
                                        agent_attention: drag.agent_attention,
                                        agent_model: drag.agent_model.clone(),
                                        width: drag.width,
                                    })
                                })
                            })
                            .child(agent_sidebar_badge(
                                agent_identity
                                    .as_ref()
                                    .and_then(|identity| identity.agent_kind.as_deref()),
                                agent_identity
                                    .as_ref()
                                    .and_then(|identity| identity.agent_state),
                                agent_identity
                                    .as_ref()
                                    .and_then(|identity| identity.agent_attention),
                                selected,
                            ))
                            .child(sidebar_workspace_text_column(
                                (self.left_sidebar_tab_text_width() - group_inset).max(80.0),
                                &title_label,
                                appearance.title,
                                &agent_label,
                                agent_color,
                                &location_label,
                                location_color,
                            ))
                            .when(can_reorder, |item| {
                                item.child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "workspace-drop-before-{workspace_id}"
                                        )))
                                        .absolute()
                                        .top_0()
                                        .left_0()
                                        .right_0()
                                        .h(px(SIDEBAR_WORKSPACE_HEIGHT / 2.0))
                                        .can_drop(move |value, _, _| {
                                            value
                                                .downcast_ref::<SidebarWorkspaceDrag>()
                                                .is_some_and(|drag| {
                                                    drag.workspace_id != workspace_id
                                                })
                                        })
                                        .drag_over::<SidebarWorkspaceDrag>(|style, _, _, _| {
                                            style.border_t_2().border_color(colors().accent)
                                        })
                                        .on_drop(cx.listener(
                                            move |this, drag: &SidebarWorkspaceDrag, window, cx| {
                                                this.reorder_sidebar_workspace_relative(
                                                    drag.workspace_id,
                                                    workspace_id,
                                                    false,
                                                    window,
                                                    cx,
                                                );
                                            },
                                        )),
                                )
                                .child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "workspace-drop-after-{workspace_id}"
                                        )))
                                        .absolute()
                                        .bottom_0()
                                        .left_0()
                                        .right_0()
                                        .h(px(SIDEBAR_WORKSPACE_HEIGHT / 2.0))
                                        .can_drop(move |value, _, _| {
                                            value
                                                .downcast_ref::<SidebarWorkspaceDrag>()
                                                .is_some_and(|drag| {
                                                    drag.workspace_id != workspace_id
                                                })
                                        })
                                        .drag_over::<SidebarWorkspaceDrag>(|style, _, _, _| {
                                            style.border_b_2().border_color(colors().accent)
                                        })
                                        .on_drop(cx.listener(
                                            move |this, drag: &SidebarWorkspaceDrag, window, cx| {
                                                this.reorder_sidebar_workspace_relative(
                                                    drag.workspace_id,
                                                    workspace_id,
                                                    true,
                                                    window,
                                                    cx,
                                                );
                                            },
                                        )),
                                )
                            })
                            .into_any_element()
                    }))
                    .when(can_reorder, |list| {
                        list.child(
                            div()
                                .id("workspace-drop-end")
                                .flex_1()
                                .min_h(px(36.0))
                                .w_full()
                                .can_drop(|value, _, _| {
                                    value.downcast_ref::<SidebarWorkspaceDrag>().is_some()
                                })
                                .drag_over::<SidebarWorkspaceDrag>(|style, _, _, _| {
                                    style.border_t_2().border_color(colors().accent)
                                })
                                .on_drop(cx.listener(
                                    |this, drag: &SidebarWorkspaceDrag, window, cx| {
                                        this.reorder_sidebar_workspace(
                                            drag.workspace_id,
                                            None,
                                            window,
                                            cx,
                                        );
                                    },
                                )),
                        )
                    }),
            );
        }

        panel.into_any_element()
    }

    fn files_sidebar_content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let rows = self.project_files.clone();
        let selected_path = self.selected_file_path.clone();
        let file_error = self.file_error.clone();
        let project_root = self.project_root();
        let (git_root, git_statuses) = self.diff_view.read(cx).status_index();
        let status_root = git_root.unwrap_or_else(|| project_root.clone());

        div()
            .id("project-files-content")
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(colors().panel)
            // File tree
            .child(
                div()
                    .id("project-file-tree")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .px_1()
                    .pt_1()
                    .pb_2()
                    .children(rows.into_iter().map(|row| {
                        let path = row.entry.path.clone();
                        let selected = selected_path.as_ref() == Some(&path);
                        let is_directory = row.entry.kind == FileEntryKind::Directory;
                        let rel = relative_repo_path(&path, &status_root);
                        let status = if is_directory {
                            rel.as_deref()
                                .and_then(|rel| aggregate_dir_status(rel, &git_statuses))
                        } else {
                            rel.as_ref().and_then(|rel| git_statuses.get(rel).copied())
                        };
                        let icon_color = if is_directory {
                            status.map(git_status_color).unwrap_or(colors().folder)
                        } else {
                            file_tree_icon_color(row.entry.kind, &row.entry.name)
                        };
                        let name_color = status.map(git_status_color).unwrap_or(if is_directory {
                            colors().foreground
                        } else {
                            colors().muted
                        });
                        let depth = row.depth;
                        let expanded = row.expanded;
                        let rel_for_click = rel.clone();
                        div()
                            .id(SharedString::from(format!(
                                "file-row-{}",
                                path.to_string_lossy()
                            )))
                            .h(px(26.0))
                            .w_full()
                            .flex()
                            .items_center()
                            .pr_2()
                            .mb(px(1.0))
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .bg(if selected {
                                colors().elevated
                            } else {
                                gpui::rgba(0x00000000)
                            })
                            .hover(|item| item.bg(colors().hover))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                    if is_directory {
                                        if event.click_count == 1 {
                                            this.toggle_directory(&path, cx);
                                        }
                                        return;
                                    }
                                    this.select_file_path(path.clone(), cx);
                                    if let Some(rel) = rel_for_click.as_ref() {
                                        // Selecting any changed file peeks it in Git; documents
                                        // never replace the terminal surface.
                                        let selected = this.diff_view.update(cx, |diff, cx| {
                                            diff.select_path_if_changed(rel, cx)
                                        });
                                        if selected {
                                            this.right_sidebar_mode = RightSidebarMode::Diff;
                                            this.set_right_sidebar_visible(true, true, cx);
                                        }
                                    }
                                }),
                            )
                            // Indent + soft guide for nested rows.
                            .child(
                                div()
                                    .w(px(6.0 + depth as f32 * 12.0))
                                    .h_full()
                                    .flex_none()
                                    .relative()
                                    .when(depth > 0, |indent| {
                                        indent.child(
                                            div()
                                                .absolute()
                                                .left(px(6.0 + (depth as f32 - 1.0) * 12.0 + 5.0))
                                                .top_0()
                                                .bottom_0()
                                                .w(px(1.0))
                                                .bg(colors().indent_guide),
                                        )
                                    }),
                            )
                            // Expand chevron (folders) or spacer (files).
                            .child(
                                div()
                                    .w(px(12.0))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(9.0))
                                    .text_color(colors().subtle)
                                    .child(if is_directory {
                                        if expanded { "▾" } else { "▸" }
                                    } else {
                                        ""
                                    }),
                            )
                            // Folder / file icon.
                            .child(
                                div()
                                    .w(px(16.0))
                                    .h(px(16.0))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_color(icon_color)
                                    .child(file_tree_icon(
                                        row.entry.kind,
                                        expanded,
                                        &row.entry.name,
                                        icon_color,
                                    )),
                            )
                            .child(
                                div()
                                    .min_w(px(0.0))
                                    .flex_1()
                                    .truncate()
                                    .pl_1()
                                    .text_size(px(12.0))
                                    .font_weight(if selected || is_directory {
                                        gpui::FontWeight::MEDIUM
                                    } else {
                                        gpui::FontWeight::NORMAL
                                    })
                                    .text_color(if selected && status.is_none() {
                                        colors().foreground
                                    } else {
                                        name_color
                                    })
                                    .child(row.entry.name),
                            )
                            .when_some(status.map(git_status_trailing), |row, trailing| {
                                row.child(trailing)
                            })
                    })),
            )
            .when_some(file_error, |panel, error| {
                panel.child(
                    div()
                        .mx_2()
                        .mb_1()
                        .p_2()
                        .rounded(px(5.0))
                        .bg(colors().diff_deleted_bg)
                        .text_size(px(9.0))
                        .text_color(colors().danger)
                        .child(error),
                )
            })
            .into_any_element()
    }

    fn info_sidebar_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let root = self.project_root();
        let workspace_count = self.snapshot.workspace_entries().len();
        let (tab_count, pane_count) = self
            .snapshot
            .selected_workspace()
            .map(|workspace| {
                (
                    workspace.tabs.len(),
                    workspace
                        .tabs
                        .iter()
                        .map(|tab| tab.sessions.len())
                        .sum::<usize>(),
                )
            })
            .unwrap_or_default();
        let selected = self.selected_file_path.as_ref().map(|path| {
            (
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string()),
                path.display().to_string(),
            )
        });
        let facts = [
            ("Workspaces", workspace_count.to_string()),
            ("Tabs", tab_count.to_string()),
            ("Panes", pane_count.to_string()),
            ("Terminal", self.terminal_port.backend_name().to_owned()),
            ("Files visibles", self.project_files.len().to_string()),
        ];
        div()
            .id("project-info-panel")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .p_3()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .id("sidebar-back-sessions")
                    .px_1()
                    .py_1()
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .text_size(px(11.0))
                    .text_color(colors().subtle)
                    .hover(|back| back.text_color(colors().foreground).bg(colors().hover))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.left_sidebar_mode = LeftSidebarMode::Sessions;
                        cx.notify();
                    }))
                    .child("← Sessions"),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(colors().muted)
                    .child("PROJECT ROOT"),
            )
            .child(
                div()
                    .p_2()
                    .rounded(px(6.0))
                    .bg(colors().elevated)
                    .font_family(MONO_FONT)
                    .text_size(px(9.0))
                    .text_color(colors().foreground)
                    .child(root.display().to_string()),
            )
            .children(facts.into_iter().map(|(label, value)| {
                div()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(10.0))
                            .text_color(colors().subtle)
                            .child(label),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(colors().foreground)
                            .child(value),
                    )
            }))
            .when_some(selected, |panel, (name, path)| {
                panel
                    .child(
                        div()
                            .mt_2()
                            .text_size(px(10.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(colors().muted)
                            .child("SELECCIÓN"),
                    )
                    .child(
                        div()
                            .p_2()
                            .rounded(px(6.0))
                            .bg(colors().elevated)
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(colors().foreground)
                                    .child(name),
                            )
                            .child(
                                div()
                                    .font_family(MONO_FONT)
                                    .text_size(px(8.5))
                                    .text_color(colors().subtle)
                                    .child(path),
                            ),
                    )
            })
            .into_any_element()
    }

    fn right_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = self.right_sidebar_mode;
        let full_width = self.right_sidebar_width();
        let width = full_width * self.right_sidebar_progress;
        let show_handle = self.right_sidebar_progress > 0.99;
        let content = match mode {
            RightSidebarMode::Files => self.files_sidebar_content(cx),
            RightSidebarMode::Diff => self.diff_view.clone().into_any_element(),
        };

        clipped_width_panel(width, full_width, colors().panel, content).when(
            show_handle,
            |sidebar| {
                sidebar.child(self.sidebar_resize_handle(
                    "resize-right-sidebar",
                    SidebarResizeEdge::Right,
                    cx,
                ))
            },
        )
    }

    fn context_menu_overlay(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.context_menu.clone()?;
        let pane_identity = match &menu.kind {
            ContextMenuKind::Pane { session_id } => self.pane_identity_by_id(*session_id, cx),
            ContextMenuKind::Workspace { .. }
            | ContextMenuKind::SidebarSpace { .. }
            | ContextMenuKind::SidebarBackground => None,
        };
        let items: Vec<(&str, ContextMenuAction, bool)> = match &menu.kind {
            ContextMenuKind::Workspace { .. } => vec![
                ("Renombrar", ContextMenuAction::Rename, false),
                ("Crear espacio", ContextMenuAction::CreateSpace, false),
                ("Eliminar", ContextMenuAction::Delete, true),
            ],
            ContextMenuKind::SidebarSpace { .. } => vec![
                ("Renombrar", ContextMenuAction::Rename, false),
                ("Eliminar espacio", ContextMenuAction::DeleteSpace, true),
            ],
            ContextMenuKind::SidebarBackground => {
                vec![("Crear espacio", ContextMenuAction::CreateSpace, false)]
            }
            ContextMenuKind::Pane { .. } => vec![
                ("Renombrar", ContextMenuAction::Rename, false),
                ("Cerrar pane", ContextMenuAction::ClosePane, true),
                ("Dividir a la derecha", ContextMenuAction::SplitRight, false),
                ("Dividir abajo", ContextMenuAction::SplitDown, false),
                ("Zoom", ContextMenuAction::ToggleZoom, false),
            ],
        };
        Some(
            div()
                .absolute()
                .inset_0()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.close_context_menu(cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, _, cx| {
                        this.close_context_menu(cx);
                    }),
                )
                .child(
                    div()
                        .id("context-menu")
                        .absolute()
                        .left(px(menu.x))
                        .top(px(menu.y))
                        .min_w(px(180.0))
                        .py_1()
                        .rounded(px(8.0))
                        .border_1()
                        .border_color(colors().border_subtle)
                        .bg(colors().elevated)
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .on_mouse_down(MouseButton::Right, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .when_some(pane_identity, |menu, identity| {
                            menu.child(
                                div()
                                    .min_w(px(220.0))
                                    .max_w(px(320.0))
                                    .px_3()
                                    .py_2()
                                    .mb_1()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .border_b_1()
                                    .border_color(colors().border_subtle)
                                    .child(agent_compact_badge(
                                        identity.agent_kind.as_deref(),
                                        identity.agent_state,
                                        identity.agent_attention,
                                        true,
                                    ))
                                    .child(
                                        div()
                                            .min_w(px(0.0))
                                            .flex_1()
                                            .flex()
                                            .flex_col()
                                            .child(
                                                div()
                                                    .truncate()
                                                    .font_family(MONO_FONT)
                                                    .text_size(px(10.5))
                                                    .font_weight(gpui::FontWeight::MEDIUM)
                                                    .text_color(colors().foreground)
                                                    .child(identity.title),
                                            )
                                            .when_some(identity.detail, |column, detail| {
                                                column.child(
                                                    div()
                                                        .truncate()
                                                        .font_family(MONO_FONT)
                                                        .text_size(px(9.0))
                                                        .text_color(colors().subtle)
                                                        .child(detail),
                                                )
                                            }),
                                    ),
                            )
                        })
                        .children(items.into_iter().enumerate().map(
                            |(index, (label, action, danger))| {
                                div()
                                    .id(SharedString::from(format!("context-menu-item-{index}")))
                                    .h(px(30.0))
                                    .mx_1()
                                    .px_3()
                                    .rounded(px(5.0))
                                    .flex()
                                    .items_center()
                                    .cursor_pointer()
                                    .text_size(px(11.0))
                                    .text_color(if danger {
                                        colors().danger
                                    } else {
                                        colors().foreground
                                    })
                                    .hover(|item| item.bg(colors().hover))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.run_context_menu_action(action, window, cx);
                                    }))
                                    .child(label)
                            },
                        )),
                )
                .into_any_element(),
        )
    }

    fn rename_modal(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let prompt = self.rename_prompt.clone()?;
        let title = match prompt.kind {
            RenamePromptKind::Workspace { .. } => "Renombrar sesión",
            RenamePromptKind::Pane { .. } => "Renombrar pane",
            RenamePromptKind::CreateSpace { .. } => "Crear espacio",
            RenamePromptKind::CreateEmptySpace => "Crear espacio",
            RenamePromptKind::SidebarSpace { .. } => "Renombrar espacio",
        };
        let value = if prompt.value.is_empty() {
            "Escribe un nombre…".to_owned()
        } else {
            prompt.value
        };
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(colors().overlay())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.rename_prompt = None;
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .w(px(420.0))
                        .max_w_full()
                        .mx_4()
                        .p_4()
                        .rounded_lg()
                        .border_1()
                        .border_color(colors().border_subtle)
                        .bg(colors().elevated)
                        .shadow_lg()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(colors().foreground)
                                .child(title),
                        )
                        .child(
                            div()
                                .h(px(34.0))
                                .px_3()
                                .rounded(px(5.0))
                                .border_1()
                                .border_color(colors().border_subtle)
                                .bg(colors().terminal)
                                .flex()
                                .items_center()
                                .font_family(MONO_FONT)
                                .text_size(px(11.0))
                                .text_color(if value == "Escribe un nombre…" {
                                    colors().subtle
                                } else {
                                    colors().foreground
                                })
                                .child(value),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(colors().subtle)
                                        .child("↵ confirmar · esc cancelar"),
                                )
                                .child(
                                    div()
                                        .id("confirm-rename-prompt")
                                        .px_3()
                                        .py_1()
                                        .rounded(px(5.0))
                                        .border_1()
                                        .border_color(colors().border_subtle)
                                        .cursor_pointer()
                                        .bg(colors().selection)
                                        .text_xs()
                                        .text_color(colors().foreground)
                                        .hover(|button| button.bg(colors().hover))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.confirm_rename_prompt(cx);
                                        }))
                                        .child("Confirmar"),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    fn error_banner(&self) -> Option<impl IntoElement> {
        self.persistence_error.as_ref().map(|error| {
            div()
                .h(px(30.0))
                .flex_none()
                .flex()
                .items_center()
                .px_3()
                .gap_2()
                .bg(colors().elevated)
                .border_b_1()
                .border_color(colors().danger)
                .text_size(px(10.5))
                .text_color(colors().danger)
                .child(div().size(px(5.0)).rounded_full().bg(colors().danger))
                .child(error.clone())
        })
    }
}

impl Render for WorkspaceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_terminal_surface_visibility(cx);
        self.ensure_appearance_subscription(window, cx);
        self.ensure_activation_subscription(window, cx);
        self.ensure_window_bounds_subscription(window, cx);
        if self.initial_terminal_focus_pending {
            self.initial_terminal_focus_pending = false;
            cx.defer_in(window, |this, window, cx| {
                this.focus_selected_terminal(window, cx);
            });
        }
        if let Some(session_id) = self.pending_focus_session.take() {
            cx.defer_in(window, move |this, window, cx| {
                this.focus_terminal(session_id, window, cx);
            });
        }
        if self.pending_focus_dev_terminal {
            self.pending_focus_dev_terminal = false;
            if let Some(workspace_id) = self.current_workspace_id()
                && let Some(terminal) = self.selected_dev_terminal(workspace_id, cx)
            {
                cx.defer_in(window, move |_, window, cx| {
                    terminal.read(cx).focus_handle(cx).focus(window);
                });
            }
        }
        let mut body = div()
            .id("vibra-root")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::new_workspace))
            .on_action(cx.listener(Self::new_terminal_tab))
            .on_action(cx.listener(Self::close_terminal))
            .on_action(cx.listener(Self::toggle_dev_terminal))
            .on_action(cx.listener(Self::toggle_left_sidebar))
            .on_action(cx.listener(Self::toggle_right_sidebar))
            .on_action(cx.listener(Self::previous_workspace))
            .on_action(cx.listener(Self::next_workspace))
            .on_action(cx.listener(Self::go_to_tab))
            .on_action(cx.listener(Self::split_pane_left))
            .on_action(cx.listener(Self::split_pane_right))
            .on_action(cx.listener(Self::split_pane_up))
            .on_action(cx.listener(Self::split_pane_down))
            .on_action(cx.listener(Self::focus_pane_left))
            .on_action(cx.listener(Self::focus_pane_right))
            .on_action(cx.listener(Self::focus_pane_up))
            .on_action(cx.listener(Self::focus_pane_down))
            .on_action(cx.listener(Self::previous_pane))
            .on_action(cx.listener(Self::next_pane))
            .on_action(cx.listener(Self::resize_pane_left))
            .on_action(cx.listener(Self::resize_pane_right))
            .on_action(cx.listener(Self::resize_pane_up))
            .on_action(cx.listener(Self::resize_pane_down))
            .on_action(cx.listener(Self::equalize_panes))
            .on_action(cx.listener(Self::toggle_pane_zoom))
            .on_action(cx.listener(Self::toggle_command_palette))
            .on_action(cx.listener(Self::quick_open))
            .on_action(cx.listener(Self::open_ide))
            .on_action(cx.listener(Self::show_settings))
            .capture_key_down(cx.listener(Self::on_workspace_key_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::finish_pane_resize))
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .font_family(".SystemUIFont")
            .text_size(px(12.0))
            .text_color(colors().foreground)
            .bg(colors().background);

        body = body.child(self.titlebar(cx));

        if let Some(banner) = self.error_banner() {
            body = body.child(banner);
        }

        let mut layout = div()
            .id("workspace-columns")
            .relative()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .gap(px(PANEL_GAP))
            .px(px(PANEL_GAP))
            .pb(px(PANEL_GAP))
            .on_drag_move(cx.listener(Self::on_sidebar_resize_move));
        let expanded_review = self.right_sidebar_visible
            && self.right_sidebar_mode == RightSidebarMode::Diff
            && self.diff_view.read(cx).review_expanded();
        if expanded_review {
            layout = layout.child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .h_full()
                    .overflow_hidden()
                    .rounded(px(PANEL_RADIUS))
                    .border_1()
                    .border_color(colors().border_subtle)
                    .child(self.diff_view.clone()),
            );
        } else {
            // Keep sidebars mounted while progress > 0 so close animations can finish.
            if self.left_sidebar_progress > 0.001 {
                layout = layout.child(self.sidebar(cx));
            }
            layout = layout.child(self.center_panel(cx));
            if self.right_sidebar_progress > 0.001 {
                layout = layout.child(self.right_sidebar(cx));
            }
        }

        body = body.child(layout);
        if let Some(modal) = self.palette_modal(cx) {
            body = body.child(modal);
        } else if let Some(modal) = self.rename_modal(cx) {
            body = body.child(modal);
        } else if let Some(modal) = self.settings_modal(window, cx) {
            body = body.child(modal);
        }
        if let Some(menu) = self.context_menu_overlay(cx) {
            body = body.child(menu);
        }
        if let Some(menu) = self.ide_menu_overlay(cx) {
            body = body.child(menu);
        }
        body
    }
}

#[cfg(test)]
mod tests {
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
            })
            .unwrap();

        std::fs::remove_dir_all(root).unwrap();
    }

    #[gpui::test]
    fn toggling_dev_terminal_keeps_a_hidden_pty_without_changing_panes(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::infrastructure::files::LocalFileSystemPort;
        use crate::infrastructure::git::GitCliPort;
        use crate::infrastructure::persistence::WorkspaceRepository;
        use crate::infrastructure::settings::SettingsRepository;
        use std::sync::Arc;

        let root = std::env::temp_dir().join(format!("vibra-dev-terminal-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let repository = WorkspaceRepository::at(root.join("workspace.json"));
        let mut snapshot = WorkspaceSnapshot::default();
        snapshot.create_workspace(&root);
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
                let selected = view.snapshot.selected_session().unwrap().id;
                let workspace_id = view.snapshot.selected_workspace().unwrap().id;
                assert!(view.dev_terminals.is_empty());
                assert!(!view.is_dev_terminal_visible());

                view.toggle_dev_terminal(&crate::ToggleDevTerminal, window, cx);
                let drawer = view
                    .selected_dev_terminal(workspace_id, cx)
                    .expect("dev terminal PTY");
                let drawer_id = drawer.read(cx).session_id();
                assert!(view.is_dev_terminal_visible());
                assert!(drawer.read(cx).is_surface_visible());
                assert_eq!(view.snapshot.selected_session().unwrap().id, selected);
                assert!(
                    !view.terminals.contains_key(&drawer_id),
                    "the utility console must stay outside workspace panes"
                );

                view.toggle_dev_terminal(&crate::ToggleDevTerminal, window, cx);
                assert!(!view.is_dev_terminal_visible());
                assert!(
                    view.dev_terminals.contains_key(&workspace_id),
                    "hiding must keep the PTY"
                );
                assert!(!drawer.read(cx).is_surface_visible());
                assert_eq!(view.snapshot.selected_session().unwrap().id, selected);
                assert_eq!(
                    view.selected_dev_terminal(workspace_id, cx)
                        .unwrap()
                        .read(cx)
                        .session_id(),
                    drawer_id
                );

                view.toggle_dev_terminal(&crate::ToggleDevTerminal, window, cx);
                assert!(view.is_dev_terminal_visible());
                assert!(drawer.read(cx).is_surface_visible());
                assert_eq!(
                    view.selected_dev_terminal(workspace_id, cx)
                        .unwrap()
                        .read(cx)
                        .session_id(),
                    drawer_id
                );

                view.add_dev_terminal(window, cx);
                assert_eq!(view.dev_terminals[&workspace_id].terminals.len(), 2);
                let second_id = view
                    .selected_dev_terminal(workspace_id, cx)
                    .unwrap()
                    .read(cx)
                    .session_id();
                assert_ne!(second_id, drawer_id);
            })
            .unwrap();

        std::fs::remove_dir_all(root).unwrap();
    }

    #[gpui::test]
    fn dev_terminals_stay_scoped_to_the_sidebar_session(cx: &mut gpui::TestAppContext) {
        use crate::infrastructure::files::LocalFileSystemPort;
        use crate::infrastructure::git::GitCliPort;
        use crate::infrastructure::persistence::WorkspaceRepository;
        use crate::infrastructure::settings::SettingsRepository;
        use std::sync::Arc;

        let root = std::env::temp_dir().join(format!("vibra-dev-scope-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let repository = WorkspaceRepository::at(root.join("workspace.json"));
        let mut snapshot = WorkspaceSnapshot::default();
        snapshot.create_workspace(&root);
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
                assert_eq!(
                    view.snapshot.selected_workspace().unwrap().id,
                    second_workspace
                );
                view.toggle_dev_terminal(&crate::ToggleDevTerminal, window, cx);
                let second_drawer = view
                    .selected_dev_terminal(second_workspace, cx)
                    .expect("second session drawer")
                    .read(cx)
                    .session_id();
                assert!(view.is_dev_terminal_visible());

                view.select_workspace(first_project, first_workspace, window, cx);
                assert!(
                    !view.is_dev_terminal_visible(),
                    "a different sidebar session must not inherit the drawer"
                );
                assert!(
                    !view
                        .selected_dev_terminal(second_workspace, cx)
                        .unwrap()
                        .read(cx)
                        .is_surface_visible()
                );

                view.toggle_dev_terminal(&crate::ToggleDevTerminal, window, cx);
                let first_drawer = view
                    .selected_dev_terminal(first_workspace, cx)
                    .expect("first session drawer")
                    .read(cx)
                    .session_id();
                assert_ne!(first_drawer, second_drawer);

                view.select_workspace(first_project, second_workspace, window, cx);
                assert!(view.is_dev_terminal_visible());
                assert_eq!(
                    view.selected_dev_terminal(second_workspace, cx)
                        .unwrap()
                        .read(cx)
                        .session_id(),
                    second_drawer
                );
            })
            .unwrap();

        std::fs::remove_dir_all(root).unwrap();
    }
}
