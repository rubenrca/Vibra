#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsPage {
    General,
    Appearance,
    Iphone,
    Agents,
    Security,
}
impl SettingsPage {
    fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Appearance => "Apariencia",
            Self::Iphone => "iPhone",
            Self::Agents => "Agentes",
            Self::Security => "Privacidad",
        }
    }
    fn id(self) -> &'static str {
        match self {
            Self::General => "settings-general",
            Self::Appearance => "settings-appearance",
            Self::Iphone => "settings-iphone",
            Self::Agents => "settings-agents",
            Self::Security => "settings-privacy",
        }
    }
}

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Context, Div, DragMoveEvent, Entity, FocusHandle, Focusable, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseUpEvent, ParentElement, Render, SharedString,
    Stateful, Styled, Subscription, Task, Timer, Transformation, Window, WindowControlArea, div,
    prelude::*, px, radians, relative, svg,
};
use uuid::Uuid;

use crate::domain::workspace::{
    PaneBranch, PaneFocusDirection, PaneLayoutSnapshot, PaneResizeDirection, PaneSplitDirection,
    SessionSnapshot, SidebarEntry, TabSnapshot, WorkspaceSnapshot, WorkspaceSplitAxis,
};
use crate::infrastructure::automation::{
    AgentAttention, AgentHookStatus, AgentKind, AgentRuntimeState, AutomationCommand,
    AutomationIncoming, AutomationResponse, AutomationServer, agent_hook_status,
    install_agent_hooks, uninstall_agent_hooks,
};
use crate::infrastructure::editor::InstalledEditor;
use crate::infrastructure::notifications::{
    AgentActivitySnapshot, AgentNotificationDelivery, agent_notification_copy, should_notify_agent,
};
use crate::infrastructure::persistence::WorkspaceRepository;
use crate::infrastructure::settings::{
    AppSettings, MAX_LEFT_SIDEBAR_WIDTH, MAX_RIGHT_SIDEBAR_WIDTH, MIN_LEFT_SIDEBAR_WIDTH,
    MIN_RIGHT_SIDEBAR_WIDTH, SettingsRepository,
};
use crate::ports::files::{FileEntry, FileEntryKind, FileSystemPort};
use crate::ports::git::{GitBranchSummary, GitPort};
use crate::ports::terminal::TerminalPort;
use crate::ports::terminal::{TerminalAgentKindSource, TerminalAgentPresence};
use crate::ui::agent_marks::{agent_compact_badge, agent_sidebar_badge, agent_status_color};
use crate::ui::diff_view::{DiffView, DiffViewEvent};
use crate::ui::terminal::{TerminalDragPreview, TerminalView, TerminalViewEvent};
use crate::ui::theme::{self, AppearanceMode, ThemeTone, colors};
use crate::{
    CloseTerminal, EqualizePanes, FocusPaneDown, FocusPaneLeft, FocusPaneRight, FocusPaneUp,
    GoToTab, NewTerminalTab, NewWorkspace, NextPane, NextWorkspace, OpenIde, PreviousPane,
    PreviousWorkspace, QuickOpen, ResizePaneDown, ResizePaneLeft, ResizePaneRight, ResizePaneUp,
    ShowSettings, SplitPaneDown, SplitPaneLeft, SplitPaneRight, SplitPaneUp, ToggleCommandPalette,
    ToggleDevTerminal, ToggleLeftSidebar, TogglePaneZoom, ToggleRightSidebar,
};

/// Titlebar chrome width when the left sidebar is fully collapsed.
const TITLEBAR_CHROME_COLLAPSED: f32 = 148.0;
/// Titlebar chrome width when the right sidebar is fully collapsed (toggle only).
const TITLEBAR_RIGHT_CHROME_COLLAPSED: f32 = 40.0;
const TITLEBAR_HEIGHT: f32 = 38.0;
/// Quiet separation between the app shell and its primary work surfaces.
const PANEL_GAP: f32 = 4.0;
/// Shared radius keeps sidebars and the terminal reading as one panel system.
const PANEL_RADIUS: f32 = 10.0;
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
const HOOK_OBSERVATION_TTL: Duration = Duration::from_secs(15 * 60);
/// IDE-style utility console height. It is intentionally compact so the main
/// terminal remains the primary surface.
const DEV_TERMINAL_HEIGHT: f32 = 260.0;

/// Bottom-console PTYs for one sidebar session. They stay alive while that
/// session exists, but they are never shared across sessions.
struct DevTerminalDrawer {
    terminals: Vec<Entity<TerminalView>>,
    selected_id: Uuid,
    visible: bool,
}

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
struct TerminalAgentObservation {
    presence: TerminalAgentPresence,
    observed_at: Instant,
}

#[derive(Clone)]
struct HookAgentPresence {
    kind: Option<AgentKind>,
    state: AgentRuntimeState,
    attention: Option<AgentAttention>,
    model: Option<String>,
    session_id: Option<String>,
    observed_at: Instant,
}

#[derive(Clone)]
struct ResolvedAgentPresence {
    kind: String,
    state: Option<AgentRuntimeState>,
    attention: Option<AgentAttention>,
    model: Option<String>,
    kind_source: &'static str,
    state_source: Option<&'static str>,
    process_id: Option<u32>,
    session_id: Option<String>,
}

struct SettingsToggleRow {
    label: &'static str,
    description: &'static str,
    enabled: bool,
    divider: bool,
    id: &'static str,
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

/// A workspace can have several split panes and tabs. Surface the agent that
/// needs the user most, rather than whichever pane happened to be selected
/// when the workspace was last saved.
fn sidebar_agent_priority(identity: &PaneIdentity) -> u8 {
    match (identity.agent_state, identity.agent_attention) {
        (Some(AgentRuntimeState::Waiting), Some(AgentAttention::Permission)) => 50,
        (Some(AgentRuntimeState::Waiting), Some(AgentAttention::Question)) => 45,
        (Some(AgentRuntimeState::Waiting), Some(AgentAttention::Plan)) => 40,
        (Some(AgentRuntimeState::Waiting), Some(AgentAttention::Notification)) => 35,
        (Some(AgentRuntimeState::Working), _) => 30,
        (Some(AgentRuntimeState::Waiting), _) => 20,
        (Some(AgentRuntimeState::Idle), _) => 10,
        (None, _) => 0,
    }
}

fn sidebar_agent_line(
    kind: Option<&str>,
    model: Option<&str>,
    state: Option<AgentRuntimeState>,
    attention: Option<AgentAttention>,
) -> String {
    let kind = kind.unwrap_or("Terminal");
    let activity = match (state, attention) {
        (_, Some(AgentAttention::Permission)) => "needs permission",
        (_, Some(AgentAttention::Question)) => "has a question",
        (_, Some(AgentAttention::Plan)) => "has a plan",
        (_, Some(AgentAttention::Notification)) => "needs attention",
        (Some(AgentRuntimeState::Working), _) => "working",
        (Some(AgentRuntimeState::Waiting), _) => "waiting",
        (Some(AgentRuntimeState::Idle), _) => "ready",
        (None, _) => "shell",
    };
    match model.map(str::trim).filter(|model| !model.is_empty()) {
        Some(model) => format!("{kind} · {} · {activity}", compact_chrome_label(model, 20)),
        None => format!("{kind} · {activity}"),
    }
}

fn sidebar_location_line(branch: Option<&str>, path: &str) -> String {
    match branch {
        Some(branch) => format!("{branch}  ·  {path}"),
        None => path.to_owned(),
    }
}

#[derive(Clone)]
struct PaneDividerDrag {
    path: Vec<PaneBranch>,
    axis: WorkspaceSplitAxis,
}

struct PaneDividerDragView {
    axis: WorkspaceSplitAxis,
}

#[derive(Clone)]
struct TabDrag {
    tab_id: Uuid,
    title: String,
    selected: bool,
    shortcut: Option<String>,
    tab_count: usize,
}

struct TabDragView {
    title: String,
    selected: bool,
    shortcut: Option<String>,
    width: f32,
}

#[derive(Clone)]
struct PaneDrag {
    session_id: Uuid,
    preview: TerminalDragPreview,
}

#[derive(Clone)]
struct SidebarWorkspaceDrag {
    workspace_id: Uuid,
    source_space_id: Option<Uuid>,
    title: String,
    branch: Option<String>,
    path: String,
    selected: bool,
    dirty: bool,
    behind: usize,
    agent_kind: Option<String>,
    agent_state: Option<AgentRuntimeState>,
    agent_attention: Option<AgentAttention>,
    agent_model: Option<String>,
    width: f32,
}

struct SidebarWorkspaceDragView {
    title: String,
    branch: Option<String>,
    path: String,
    selected: bool,
    dirty: bool,
    behind: usize,
    agent_kind: Option<String>,
    agent_state: Option<AgentRuntimeState>,
    agent_attention: Option<AgentAttention>,
    agent_model: Option<String>,
    width: f32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReorderDrag {
    Tab(Uuid),
    Pane(Uuid),
    SidebarWorkspace(Uuid),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SidebarResizeEdge {
    Left,
    Right,
}

struct SidebarResizeDragView;

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
    ShareRemote,
    ReclaimRemote,
    SplitRight,
    SplitDown,
    ToggleZoom,
}

impl Render for PaneDividerDragView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .when(self.axis == WorkspaceSplitAxis::Horizontal, |line| {
                line.w(px(2.0)).h(px(40.0))
            })
            .when(self.axis == WorkspaceSplitAxis::Vertical, |line| {
                line.w(px(40.0)).h(px(2.0))
            })
            .rounded_full()
            .bg(colors().border_subtle)
    }
}

impl Render for SidebarResizeDragView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(2.0))
            .h(px(48.0))
            .rounded_full()
            .bg(colors().muted)
    }
}

impl Render for TabDragView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected;
        div()
            .h(px(26.0))
            .w(px(self.width))
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .px(px(10.0))
            .rounded(px(7.0))
            .bg(if selected {
                colors().selection
            } else {
                gpui::rgba(0x00000000)
            })
            .border_1()
            .border_color(if selected {
                colors().muted
            } else {
                colors().border_subtle
            })
            .text_color(colors().foreground)
            .shadow_sm()
            .opacity(0.96)
            .child(
                div()
                    .min_w(px(0.0))
                    .truncate()
                    .text_center()
                    .text_size(px(12.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(self.title.clone()),
            )
            .when_some(self.shortcut.clone(), |tab, shortcut| {
                tab.child(
                    div()
                        .absolute()
                        .right(px(10.0))
                        .flex_none()
                        .font_family("JetBrains Mono")
                        .text_size(px(9.5))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(if selected {
                            colors().muted
                        } else {
                            colors().subtle
                        })
                        .child(shortcut),
                )
            })
    }
}

impl Render for SidebarWorkspaceDragView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let branch_color = match (self.dirty, self.behind > 0) {
            (true, _) => colors().warning,
            (_, true) => colors().accent,
            _ if self.selected => colors().muted,
            _ => colors().subtle,
        };
        let path_color = if self.selected {
            colors().muted
        } else {
            colors().subtle
        };
        let title_color = if self.selected {
            colors().foreground
        } else {
            colors().muted
        };
        let agent_line = sidebar_agent_line(
            self.agent_kind.as_deref(),
            self.agent_model.as_deref(),
            self.agent_state,
            self.agent_attention,
        );
        let agent_color = agent_status_color(self.agent_state, self.agent_attention).unwrap_or(
            if self.selected {
                colors().muted
            } else {
                colors().subtle
            },
        );
        let location_line = sidebar_location_line(self.branch.as_deref(), &self.path);
        let location_color = if self.branch.is_some() {
            branch_color
        } else {
            path_color
        };

        div()
            .h(px(SIDEBAR_WORKSPACE_HEIGHT))
            .w(px(self.width))
            .px(px(10.0))
            .rounded(px(7.0))
            .flex()
            .items_center()
            .gap_2()
            .bg(if self.selected {
                colors().elevated
            } else {
                colors().sidebar
            })
            .border_1()
            .border_color(if self.selected {
                colors().border_subtle
            } else {
                gpui::rgba(0x00000000)
            })
            .child(agent_sidebar_badge(
                self.agent_kind.as_deref(),
                self.agent_state,
                self.agent_attention,
                self.selected,
            ))
            .child(
                div()
                    .w(px(self.width - SIDEBAR_WORKSPACE_CARD_CHROME))
                    .flex_none()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .gap(px(1.0))
                    .child(sidebar_tab_line(
                        &self.title,
                        title_color,
                        11.5,
                        true,
                        false,
                    ))
                    .child(sidebar_tab_line(&agent_line, agent_color, 9.5, true, false))
                    .child(sidebar_tab_line(
                        &location_line,
                        location_color,
                        8.5,
                        false,
                        true,
                    )),
            )
    }
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
    agent_presence: HashMap<Uuid, TerminalAgentObservation>,
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
    show_hidden_files: bool,
    file_error: Option<SharedString>,
    palette_mode: Option<PaletteMode>,
    palette_query: String,
    palette_selected: usize,
    palette_files: Vec<PathBuf>,
    settings_open: bool,
    settings_page: SettingsPage,
    remote_feedback: Option<String>,
    remote_copied: bool,
    remote_forget_confirm: bool,
    context_menu: Option<ContextMenuState>,
    ide_menu_open: bool,
    installed_editors: Vec<InstalledEditor>,
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
                        if crate::infrastructure::remote::hub().status().enabled {
                            cx.notify();
                        }
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
            if this.window_size_persist_generation > 0 {
                let _ = this.settings_repository.save(&this.settings);
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
            show_hidden_files: settings.show_hidden_files,
            file_error: None,
            palette_mode: None,
            palette_query: String::new(),
            palette_selected: 0,
            palette_files: Vec::new(),
            settings_open: false,
            settings_page: SettingsPage::General,
            remote_feedback: None,
            remote_copied: false,
            remote_forget_confirm: false,
            context_menu: None,
            ide_menu_open: false,
            installed_editors: Vec::new(),
            rename_prompt: None,
            right_sidebar_visible: settings.git_panel_visible,
            right_sidebar_progress: if settings.git_panel_visible { 1.0 } else { 0.0 },
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
        view.refresh_sidebar_workspace_meta(cx);
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
                alias,
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
            agent_state: presence.as_ref().and_then(|presence| presence.state),
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
        let task = cx.background_spawn(async move {
            let mut results = Vec::with_capacity(targets.len());
            for (workspace_id, cwd) in targets {
                let summary = port.branch_summary(&cwd).ok().flatten();
                results.push((workspace_id, cwd, summary));
            }
            results
        });
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
        let show_hidden = self.show_hidden_files;
        let port = self.file_port.clone();
        let selected = self.selected_file_path.clone();
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

    fn open_palette(&mut self, mode: PaletteMode, cx: &mut Context<Self>) {
        self.palette_mode = Some(mode);
        self.palette_query.clear();
        self.palette_selected = 0;
        self.settings_open = false;
        self.context_menu = None;
        self.rename_prompt = None;
        self.palette_files.clear();
        if mode == PaletteMode::Files {
            self.palette_request_id = self.palette_request_id.wrapping_add(1);
            let request_id = self.palette_request_id;
            let root = self.project_root();
            let port = self.file_port.clone();
            let task = cx.background_spawn(async move {
                let mut files = Vec::new();
                let _ = collect_search_files(port.as_ref(), &root, &root, &mut files);
                files
            });
            self._palette_task = Some(cx.spawn(async move |this, cx| {
                let files = task.await;
                let _ = this.update(cx, |this, cx| {
                    if request_id != this.palette_request_id {
                        return;
                    }
                    this.palette_files = files;
                    cx.notify();
                });
            }));
        }
        cx.notify();
    }

    fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.settings_open = true;
        self.palette_mode = None;
        self.palette_files.clear();
        self.context_menu = None;
        self.ide_menu_open = false;
        self.rename_prompt = None;
        cx.notify();
    }

    fn close_settings(&mut self, cx: &mut Context<Self>) {
        if self.settings_open {
            self.settings_open = false;
            cx.notify();
        }
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
            (ContextMenuKind::Pane { session_id }, ContextMenuAction::ShareRemote) => {
                if let Some(identity) = self.pane_identity_by_id(session_id, cx) {
                    crate::infrastructure::remote::hub().title(session_id, &identity.title);
                }
                crate::infrastructure::remote::hub().toggle_share(session_id);
                cx.notify();
            }
            (ContextMenuKind::Pane { session_id }, ContextMenuAction::ReclaimRemote) => {
                crate::infrastructure::remote::hub().reclaim(session_id);
                cx.notify();
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

    fn toggle_command_palette(
        &mut self,
        _: &ToggleCommandPalette,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.palette_mode.is_some() {
            self.palette_mode = None;
            self.palette_files.clear();
            cx.notify();
        } else {
            self.open_palette(PaletteMode::Commands, cx);
        }
    }

    fn quick_open(&mut self, _: &QuickOpen, _: &mut Window, cx: &mut Context<Self>) {
        self.open_palette(PaletteMode::Files, cx);
    }

    fn open_ide(&mut self, _: &OpenIde, _: &mut Window, cx: &mut Context<Self>) {
        if self.ide_menu_open {
            self.ide_menu_open = false;
            cx.notify();
            return;
        }
        self.installed_editors = crate::infrastructure::editor::installed_editors();
        if self.installed_editors.is_empty() {
            self.persistence_error = Some(
                "No se encontró un IDE compatible. Instala Cursor, VS Code, Windsurf, Zed, Xcode, Sublime Text o VSCodium"
                    .into(),
            );
        } else {
            self.ide_menu_open = true;
            self.context_menu = None;
        }
        cx.notify();
    }

    fn open_with_editor(&mut self, editor: InstalledEditor, cx: &mut Context<Self>) {
        self.ide_menu_open = false;
        let path = self.selected_live_cwd(cx);
        let launch = cx.background_spawn(async move {
            crate::infrastructure::editor::open_in_editor(&path, &editor)
        });
        self._open_ide_task = Some(cx.spawn(async move |this, cx| {
            let result = launch.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        if this.persistence_error.as_ref().is_some_and(|error| {
                            error.to_string().starts_with("No se pudo abrir el IDE:")
                        }) {
                            this.persistence_error = None;
                        }
                    }
                    Err(error) => {
                        this.persistence_error =
                            Some(format!("No se pudo abrir el IDE: {error}").into());
                    }
                }
                cx.notify();
            });
        }));
    }

    fn palette_items(&self) -> Vec<PaletteItem> {
        let Some(mode) = self.palette_mode else {
            return Vec::new();
        };
        let mut items = match mode {
            PaletteMode::Commands => vec![
                PaletteItem {
                    label: "Terminal: New tab".into(),
                    detail: "⌘T".into(),
                    action: PaletteAction::NewTerminalTab,
                },
                PaletteItem {
                    label: "Terminal: Toggle Dev Terminal".into(),
                    detail: "⌘J".into(),
                    action: PaletteAction::ToggleDevTerminal,
                },
                PaletteItem {
                    label: "Workspace: Open current folder in IDE".into(),
                    detail: "⇧⌘E".into(),
                    action: PaletteAction::OpenIde,
                },
                PaletteItem {
                    label: "Workspace: New".into(),
                    detail: "⌘N".into(),
                    action: PaletteAction::NewWorkspace,
                },
                PaletteItem {
                    label: "Pane: Split right".into(),
                    detail: "⌘D".into(),
                    action: PaletteAction::Split(PaneSplitDirection::Right),
                },
                PaletteItem {
                    label: "Pane: Split down".into(),
                    detail: "⇧⌘D".into(),
                    action: PaletteAction::Split(PaneSplitDirection::Down),
                },
                PaletteItem {
                    label: "Pane: Split left".into(),
                    detail: String::new(),
                    action: PaletteAction::Split(PaneSplitDirection::Left),
                },
                PaletteItem {
                    label: "Pane: Split up".into(),
                    detail: String::new(),
                    action: PaletteAction::Split(PaneSplitDirection::Up),
                },
                PaletteItem {
                    label: "Pane: Equalize".into(),
                    detail: "⌃⌥E".into(),
                    action: PaletteAction::EqualizePanes,
                },
                PaletteItem {
                    label: "Pane: Toggle zoom".into(),
                    detail: "⇧⌘↵".into(),
                    action: PaletteAction::TogglePaneZoom,
                },
                PaletteItem {
                    label: "Sidebar: Toggle Sessions".into(),
                    detail: "⌘B".into(),
                    action: PaletteAction::ShowSessions,
                },
                PaletteItem {
                    label: "Sidebar: Toggle Files / Git".into(),
                    detail: "⌥⌘B".into(),
                    action: PaletteAction::ToggleGit,
                },
                PaletteItem {
                    label: "Sidebar: Files".into(),
                    detail: String::new(),
                    action: PaletteAction::ShowFiles,
                },
                PaletteItem {
                    label: "Sidebar: Info".into(),
                    detail: String::new(),
                    action: PaletteAction::ShowInfo,
                },
                PaletteItem {
                    label: "Settings: Open".into(),
                    detail: "⌘,".into(),
                    action: PaletteAction::ShowSettings,
                },
            ],
            PaletteMode::Files => {
                let root = self.project_root();
                let query = self.palette_query.to_lowercase();
                let tokens: Vec<_> = query.split_whitespace().filter(|t| !t.is_empty()).collect();
                self.palette_files
                    .iter()
                    .filter_map(|path| {
                        let label = path
                            .strip_prefix(&root)
                            .unwrap_or(path)
                            .display()
                            .to_string();
                        if !tokens.is_empty() {
                            let haystack = label.to_lowercase();
                            if !tokens.iter().all(|token| haystack.contains(token)) {
                                return None;
                            }
                        }
                        Some(PaletteItem {
                            label,
                            detail: path
                                .extension()
                                .map(|extension| extension.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                            action: PaletteAction::OpenFile(path.clone()),
                        })
                    })
                    .take(100)
                    .collect()
            }
        };
        if mode == PaletteMode::Commands {
            items.extend(
                self.snapshot
                    .workspace_entries()
                    .into_iter()
                    .map(|entry| PaletteItem {
                        label: format!("Workspace: {}", entry.workspace_name),
                        detail: entry.project_name,
                        action: PaletteAction::SelectWorkspace {
                            project_id: entry.project_id,
                            workspace_id: entry.workspace_id,
                        },
                    }),
            );
        }
        if mode == PaletteMode::Commands {
            let query = self.palette_query.to_lowercase();
            if !query.is_empty() {
                let tokens: Vec<_> = query.split_whitespace().collect();
                items.retain(|item| {
                    let haystack = item.label.to_lowercase();
                    tokens.iter().all(|token| haystack.contains(token))
                });
            }
            items.truncate(100);
        }
        items
    }

    fn execute_palette_action(
        &mut self,
        action: PaletteAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.palette_mode = None;
        match action {
            PaletteAction::NewTerminalTab => {
                self.open_terminal_tab_in_current_directory(window, cx);
            }
            PaletteAction::ToggleDevTerminal => {
                self.toggle_dev_terminal(&ToggleDevTerminal, window, cx);
            }
            PaletteAction::OpenIde => self.open_ide(&OpenIde, window, cx),
            PaletteAction::NewWorkspace => {
                self.open_workspace_in_current_directory(window, cx);
            }
            PaletteAction::Split(direction) => self.split_pane(direction, window, cx),
            PaletteAction::EqualizePanes => {
                if self.snapshot.equalize_selected_panes() {
                    self.persist(cx);
                }
            }
            PaletteAction::TogglePaneZoom => {
                if self.snapshot.toggle_selected_pane_zoom() {
                    self.sync_terminal_surface_visibility(cx);
                    self.persist(cx);
                }
            }
            PaletteAction::ToggleGit => self.toggle_diff_panel(cx),
            PaletteAction::ShowSessions => {
                if self.left_sidebar_visible && self.left_sidebar_mode == LeftSidebarMode::Sessions
                {
                    self.set_left_sidebar_visible(false, true, cx);
                } else {
                    self.left_sidebar_mode = LeftSidebarMode::Sessions;
                    self.set_left_sidebar_visible(true, true, cx);
                }
            }
            PaletteAction::ShowFiles => {
                self.right_sidebar_mode = RightSidebarMode::Files;
                self.refresh_project_files(cx);
                self.set_right_sidebar_visible(true, true, cx);
            }
            PaletteAction::ShowInfo => {
                self.left_sidebar_mode = LeftSidebarMode::Info;
                self.set_left_sidebar_visible(true, true, cx);
            }
            PaletteAction::ShowSettings => {
                self.open_settings(cx);
            }
            PaletteAction::SelectWorkspace {
                project_id,
                workspace_id,
            } => self.select_workspace(project_id, workspace_id, window, cx),
            PaletteAction::OpenFile(path) => {
                self.select_file_path(path, cx);
                self.right_sidebar_mode = RightSidebarMode::Files;
                self.set_right_sidebar_visible(true, true, cx);
            }
        }
    }

    fn on_workspace_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.to_ascii_lowercase();
        if self.settings_open {
            if matches!(key.as_str(), "escape" | "esc") {
                self.close_settings(cx);
            }
            cx.stop_propagation();
            return;
        }
        if self.ide_menu_open {
            if matches!(key.as_str(), "escape" | "esc") {
                self.ide_menu_open = false;
                cx.notify();
            }
            cx.stop_propagation();
            return;
        }
        if self.context_menu.is_some() {
            if matches!(key.as_str(), "escape" | "esc") {
                self.close_context_menu(cx);
            }
            cx.stop_propagation();
            return;
        }
        if self.rename_prompt.is_some() {
            match key.as_str() {
                "escape" | "esc" => {
                    self.rename_prompt = None;
                    cx.notify();
                }
                "enter" | "return" => self.confirm_rename_prompt(cx),
                "backspace" => {
                    if let Some(prompt) = self.rename_prompt.as_mut() {
                        prompt.value.pop();
                        cx.notify();
                    }
                }
                _ if !event.keystroke.modifiers.platform
                    && !event.keystroke.modifiers.control
                    && !event.keystroke.modifiers.alt =>
                {
                    if let Some(text) = event.keystroke.key_char.as_ref()
                        && let Some(prompt) = self.rename_prompt.as_mut()
                    {
                        prompt.value.push_str(text);
                        cx.notify();
                    }
                }
                _ => {}
            }
            cx.stop_propagation();
            return;
        }
        if self.palette_mode.is_some() {
            match key.as_str() {
                "escape" | "esc" => {
                    self.palette_mode = None;
                    cx.notify();
                }
                "up" => {
                    self.palette_selected = self.palette_selected.saturating_sub(1);
                    cx.notify();
                }
                "down" => {
                    let count = self.palette_items().len();
                    self.palette_selected =
                        (self.palette_selected + 1).min(count.saturating_sub(1));
                    cx.notify();
                }
                "enter" | "return" => {
                    let items = self.palette_items();
                    if let Some(item) = items.get(self.palette_selected) {
                        self.execute_palette_action(item.action.clone(), window, cx);
                    }
                }
                "backspace" => {
                    self.palette_query.pop();
                    self.palette_selected = 0;
                    cx.notify();
                }
                _ if !event.keystroke.modifiers.platform
                    && !event.keystroke.modifiers.control
                    && !event.keystroke.modifiers.alt =>
                {
                    if let Some(text) = event.keystroke.key_char.as_ref() {
                        self.palette_query.push_str(text);
                        self.palette_selected = 0;
                        cx.notify();
                    }
                }
                _ => {}
            }
            cx.stop_propagation();
        }
    }

    fn toggle_diff_panel(&mut self, cx: &mut Context<Self>) {
        self.set_right_sidebar_visible(!self.right_sidebar_visible, true, cx);
        if self.right_sidebar_visible {
            self.sync_diff_root(cx);
            self.diff_view
                .update(cx, |diff_view, cx| diff_view.refresh_now(cx));
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
        if self.right_sidebar_visible == visible {
            cx.notify();
            return;
        }
        self.right_sidebar_visible = visible;
        self.settings.git_panel_visible = visible;
        self.sync_git_panel_visibility(cx);
        if persist {
            self.persist_settings(cx);
        }
        self.start_sidebar_animation(cx);
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
                crate::infrastructure::remote::hub().title(*session_id, title);
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
            TerminalViewEvent::Exited { session_id, code } => {
                let _exited_session = (*session_id, *code);
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
                        !previous.presence.kind.eq_ignore_ascii_case(&current.kind)
                            || (previous.presence.kind_source == TerminalAgentKindSource::Process
                                && current.kind_source != TerminalAgentKindSource::Process)
                            || matches!(
                                (previous.presence.process_id, current.process_id),
                                (Some(left), Some(right)) if left != right
                            )
                    }
                    (Some(_), None) => true,
                    _ => false,
                };
                let definitive_exit = previous.is_some_and(|previous| {
                    previous.presence.kind_source == TerminalAgentKindSource::Process
                }) || !self.hook_agent_presence.contains_key(session_id);
                if occupant_changed && (presence.is_some() || definitive_exit) {
                    self.agent_names.remove(session_id);
                    self.hook_agent_presence.remove(session_id);
                }
                if let Some(presence) = presence {
                    self.agent_presence.insert(
                        *session_id,
                        TerminalAgentObservation {
                            presence: presence.clone(),
                            observed_at: Instant::now(),
                        },
                    );
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

    fn handle_automation_request(&mut self, request: AutomationIncoming, cx: &mut Context<Self>) {
        let pane_id = request.envelope.pane_id;
        let authorized = self
            .automation_tokens
            .get(&pane_id)
            .is_some_and(|token| *token == request.envelope.token);
        if !authorized {
            let _ = request
                .response
                .send(AutomationResponse::failure("capacidad inválida o expirada"));
            return;
        }

        let result: Result<serde_json::Value, String> = match request.envelope.command {
            AutomationCommand::SetAgentState { state } => {
                self.set_hook_agent_presence(pane_id, None, state, None, None, None);
                self.publish_agent_activity(pane_id);
                cx.notify();
                Ok(self.agent_status_value(pane_id))
            }
            AutomationCommand::SetAgentPresence {
                kind,
                state,
                attention,
                model,
                session_id,
            } => {
                self.set_hook_agent_presence(
                    pane_id,
                    Some(kind),
                    state,
                    attention,
                    model,
                    session_id,
                );
                self.publish_agent_activity(pane_id);
                cx.notify();
                Ok(self.agent_status_value(pane_id))
            }
            AutomationCommand::ClearAgentPresence { session_id } => {
                let clear = self
                    .hook_agent_presence
                    .get(&pane_id)
                    .is_none_or(|presence| {
                        session_id.is_none() || presence.session_id.as_ref() == session_id.as_ref()
                    });
                if clear {
                    self.hook_agent_presence.remove(&pane_id);
                    self.publish_agent_activity(pane_id);
                    cx.notify();
                }
                Ok(self.agent_status_value(pane_id))
            }
        };
        let response = match result {
            Ok(data) => AutomationResponse::success(data),
            Err(error) => AutomationResponse::failure(error),
        };
        let _ = request.response.send(response);
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

    fn set_hook_agent_presence(
        &mut self,
        pane_id: Uuid,
        kind: Option<AgentKind>,
        state: AgentRuntimeState,
        attention: Option<AgentAttention>,
        model: Option<String>,
        session_id: Option<String>,
    ) {
        let now = Instant::now();
        let new_session = session_id.as_deref();
        let session_changed = self
            .hook_agent_presence
            .get(&pane_id)
            .and_then(|presence| presence.session_id.as_deref())
            .zip(new_session)
            .is_some_and(|(previous, current)| previous != current);
        if session_changed {
            self.agent_names.remove(&pane_id);
        }
        let entry = self
            .hook_agent_presence
            .entry(pane_id)
            .or_insert_with(|| HookAgentPresence {
                kind: None,
                state,
                attention: None,
                model: None,
                session_id: None,
                observed_at: now,
            });
        if kind.is_some() {
            entry.kind = kind;
        }
        entry.state = state;
        entry.attention = attention;
        if model.is_some() || session_changed {
            entry.model = model;
        }
        if session_id.is_some() {
            entry.session_id = session_id;
        }
        entry.observed_at = now;
    }

    fn resolved_agent_presence(&self, pane_id: Uuid) -> Option<ResolvedAgentPresence> {
        let now = Instant::now();
        let detected = self.agent_presence.get(&pane_id);
        let hook = self
            .hook_agent_presence
            .get(&pane_id)
            .filter(|presence| now.duration_since(presence.observed_at) <= HOOK_OBSERVATION_TTL);
        // A process identity is stronger than a stale hook from a previous
        // occupant. Ignore mismatched hooks instead of merging two agents.
        let hook = hook.filter(|hook| {
            detected.is_none_or(|detected| {
                detected.presence.kind_source != TerminalAgentKindSource::Process
                    || hook.kind.is_none_or(|kind| {
                        detected
                            .presence
                            .kind
                            .eq_ignore_ascii_case(kind.display_name())
                    })
            })
        });
        let terminal_kind = detected.map(|presence| {
            (
                presence.presence.kind.as_str(),
                presence.presence.kind_source,
            )
        });
        let (kind, kind_source) = match (terminal_kind, hook.and_then(|presence| presence.kind)) {
            (Some((kind, TerminalAgentKindSource::Process)), _) => (kind.to_owned(), "process"),
            (_, Some(kind)) => (kind.display_name().to_owned(), "hook"),
            (Some((kind, TerminalAgentKindSource::Title)), _) => (kind.to_owned(), "title"),
            (Some((kind, TerminalAgentKindSource::Screen)), _) => (kind.to_owned(), "screen"),
            (None, None) => return None,
        };
        let terminal_state = detected.map(|presence| {
            (
                terminal_agent_state_to_runtime_state(presence.presence.state),
                presence.observed_at,
            )
        });
        let hook_state = hook.map(|presence| (presence.state, presence.observed_at));
        let (state, state_source, attention) = match (hook_state, terminal_state) {
            (Some((state, _)), _) => (
                Some(state),
                Some("hook"),
                hook.and_then(|presence| presence.attention),
            ),
            (None, Some((state, _))) => (Some(state), Some("heuristic"), None),
            (None, None) => (None, None, None),
        };
        Some(ResolvedAgentPresence {
            kind,
            state,
            attention,
            model: hook.and_then(|presence| presence.model.clone()),
            kind_source,
            state_source,
            process_id: detected.and_then(|presence| presence.presence.process_id),
            session_id: hook.and_then(|presence| presence.session_id.clone()),
        })
    }

    fn agent_status_value(&self, pane_id: Uuid) -> serde_json::Value {
        let presence = self.resolved_agent_presence(pane_id);
        serde_json::json!({
            "kind": presence.as_ref().map(|presence| presence.kind.as_str()),
            "state": presence.as_ref().and_then(|presence| presence.state).map(agent_runtime_state_label),
            "attention": presence.as_ref().and_then(|presence| presence.attention).map(AgentAttention::label),
            "model": presence.as_ref().and_then(|presence| presence.model.as_deref()),
            "kindSource": presence.as_ref().map(|presence| presence.kind_source),
            "stateSource": presence.as_ref().and_then(|presence| presence.state_source),
            "source": presence.as_ref().and_then(|presence| presence.state_source),
            "processId": presence.as_ref().and_then(|presence| presence.process_id),
            "sessionId": presence.as_ref().and_then(|presence| presence.session_id.as_deref()),
        })
    }

    fn session_is_selected(&self, pane_id: Uuid) -> bool {
        self.snapshot
            .selected_session()
            .is_some_and(|session| session.id == pane_id)
    }

    fn publish_agent_activity(&mut self, pane_id: Uuid) {
        let current = self.resolved_agent_presence(pane_id).and_then(|presence| {
            presence.state.map(|state| AgentActivitySnapshot {
                kind: presence.kind,
                state,
                attention: presence.attention,
            })
        });
        let previous = self.agent_activity_seen.get(&pane_id);
        if let Some(notification) = should_notify_agent(
            previous,
            current.as_ref(),
            self.session_is_selected(pane_id),
            self.window_is_active,
            self.settings.agent_notifications,
        ) {
            match notification.delivery {
                AgentNotificationDelivery::Banner => {
                    let agent = current
                        .as_ref()
                        .or(previous)
                        .map(|snapshot| snapshot.kind.as_str())
                        .unwrap_or("Agente");
                    let (title, body) = agent_notification_copy(notification.kind, agent);
                    crate::infrastructure::notifications::deliver(
                        &title,
                        &body,
                        &format!("vibra.agent.{pane_id}"),
                    );
                }
                AgentNotificationDelivery::Sound => {
                    crate::infrastructure::notifications::play_completion_sound();
                }
            }
        }
        match current {
            Some(snapshot) => {
                self.agent_activity_seen.insert(pane_id, snapshot);
            }
            None => {
                self.agent_activity_seen.remove(&pane_id);
            }
        }
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

    fn split_pane(
        &mut self,
        direction: PaneSplitDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.capture_selected_working_directory(cx);
        if let Some(session_id) = self.snapshot.split_selected_terminal(direction) {
            self.reconcile_terminal_views(cx);
            self.sync_diff_root(cx);
            self.persist(cx);
            self.focus_terminal(session_id, window, cx);
        }
    }

    fn focus_pane(
        &mut self,
        direction: PaneFocusDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.snapshot.focus_terminal(direction) {
            self.sync_terminal_surface_visibility(cx);
            self.sync_diff_root(cx);
            self.refresh_project_files(cx);
            self.persist(cx);
            self.focus_selected_terminal(window, cx);
        }
    }

    fn cycle_pane(&mut self, offset: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.snapshot.cycle_terminal(offset) {
            self.sync_terminal_surface_visibility(cx);
            self.sync_diff_root(cx);
            self.refresh_project_files(cx);
            self.persist(cx);
            self.focus_selected_terminal(window, cx);
        }
    }

    fn resize_pane(&mut self, direction: PaneResizeDirection, cx: &mut Context<Self>) {
        if self.snapshot.resize_selected_pane(direction) {
            self.persist(cx);
        }
    }

    fn finish_pane_resize(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.pane_resize_dirty {
            self.pane_resize_dirty = false;
            self.persist(cx);
        }
        if self.sidebar_resize_dirty {
            self.sidebar_resize_dirty = false;
            self.persist_settings(cx);
        }
        if self.reorder_drag.take().is_some() {
            cx.notify();
        }
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

    fn swap_panes(&mut self, from: Uuid, onto: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        self.reorder_drag = None;
        if self.snapshot.swap_tab_terminals(from, onto) {
            self.sync_terminal_surface_visibility(cx);
            self.sync_diff_root(cx);
            self.refresh_project_files(cx);
            self.refresh_sidebar_workspace_meta(cx);
            self.persist(cx);
            self.focus_terminal(from, window, cx);
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

    fn apply_agent_hook_result(
        &mut self,
        result: anyhow::Result<AgentHookStatus>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(status) => {
                self.agent_hook_status = Some(status);
                self.agent_hook_error = None;
            }
            Err(error) => {
                self.agent_hook_status = None;
                self.agent_hook_error =
                    Some(format!("No se pudo actualizar las integraciones: {error}").into());
            }
        }
        cx.notify();
    }

    fn install_agent_hooks_from_settings(&mut self, cx: &mut Context<Self>) {
        self.apply_agent_hook_result(install_agent_hooks(), cx);
    }

    fn uninstall_agent_hooks_from_settings(&mut self, cx: &mut Context<Self>) {
        self.apply_agent_hook_result(uninstall_agent_hooks(), cx);
    }

    fn set_terminal_font_size(&mut self, size: f32, cx: &mut Context<Self>) {
        let size = size.clamp(8.0, 32.0);
        if self.settings.terminal_font_size == size {
            return;
        }
        self.settings.terminal_font_size = size;
        for terminal in self.terminals.values() {
            terminal.update(cx, |terminal, cx| terminal.apply_font_size(size, cx));
        }
        for drawer in self.dev_terminals.values() {
            for terminal in &drawer.terminals {
                terminal.update(cx, |terminal, cx| terminal.apply_font_size(size, cx));
            }
        }
        self.persist_settings(cx);
    }

    fn appearance_mode(&self) -> AppearanceMode {
        AppearanceMode::parse(&self.settings.appearance_mode)
    }

    fn apply_theme_preference(&mut self, system_dark: bool, cx: &mut Context<Self>) {
        theme::apply_preference(&self.settings.theme_id, self.appearance_mode(), system_dark);
        for terminal in self.terminals.values() {
            terminal.update(cx, |_, cx| cx.notify());
        }
        for drawer in self.dev_terminals.values() {
            for terminal in &drawer.terminals {
                terminal.update(cx, |_, cx| cx.notify());
            }
        }
        cx.notify();
    }

    fn ensure_appearance_subscription(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self._appearance_subscription.is_some() {
            return;
        }
        let system_dark = ThemeTone::from_window_appearance(window.appearance()) == ThemeTone::Dark;
        self.apply_theme_preference(system_dark, cx);
        self._appearance_subscription =
            Some(cx.observe_window_appearance(window, |this, window, cx| {
                let system_dark =
                    ThemeTone::from_window_appearance(window.appearance()) == ThemeTone::Dark;
                this.apply_theme_preference(system_dark, cx);
            }));
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

    fn set_theme_id(&mut self, theme_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let theme_id = theme::canonicalize_theme_id(theme_id);
        if self.settings.theme_id == theme_id {
            return;
        }
        self.settings.theme_id = theme_id.to_string();
        let system_dark = ThemeTone::from_window_appearance(window.appearance()) == ThemeTone::Dark;
        self.apply_theme_preference(system_dark, cx);
        self.persist_settings(cx);
    }

    fn set_appearance_mode(
        &mut self,
        mode: AppearanceMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.appearance_mode() == mode {
            return;
        }
        self.settings.appearance_mode = mode.as_str().to_string();
        let system_dark = ThemeTone::from_window_appearance(window.appearance()) == ThemeTone::Dark;
        self.apply_theme_preference(system_dark, cx);
        self.persist_settings(cx);
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

    fn is_dev_terminal_visible(&self) -> bool {
        self.current_workspace_id()
            .and_then(|workspace_id| self.dev_terminals.get(&workspace_id))
            .is_some_and(|drawer| drawer.visible)
    }

    fn is_dev_terminal(&self, session_id: Uuid, cx: &Context<Self>) -> bool {
        self.dev_workspace_for_session(session_id, cx).is_some()
    }

    fn dev_workspace_for_session(&self, session_id: Uuid, cx: &Context<Self>) -> Option<Uuid> {
        self.dev_terminals
            .iter()
            .find_map(|(workspace_id, drawer)| {
                drawer
                    .terminals
                    .iter()
                    .any(|terminal| terminal.read(cx).session_id() == session_id)
                    .then_some(*workspace_id)
            })
    }

    fn selected_dev_terminal(
        &self,
        workspace_id: Uuid,
        cx: &Context<Self>,
    ) -> Option<Entity<TerminalView>> {
        let drawer = self.dev_terminals.get(&workspace_id)?;
        drawer
            .terminals
            .iter()
            .find(|terminal| terminal.read(cx).session_id() == drawer.selected_id)
            .cloned()
            .or_else(|| drawer.terminals.first().cloned())
    }

    fn spawn_dev_terminal(
        &mut self,
        workspace_id: Uuid,
        cx: &mut Context<Self>,
    ) -> Entity<TerminalView> {
        let working_directory = self.selected_live_cwd(cx);
        let terminal_port = self.terminal_port.clone();
        let font_size = self.settings.terminal_font_size;
        let visible = self
            .dev_terminals
            .get(&workspace_id)
            .is_some_and(|drawer| drawer.visible);
        let mut environment = HashMap::new();
        if let Ok(executable) = std::env::current_exe() {
            environment.insert(
                "VIBRA_CLI".into(),
                executable.to_string_lossy().into_owned(),
            );
        }
        let session_id = Uuid::new_v4();
        let terminal = cx.new(|cx| {
            TerminalView::new_with_environment(
                session_id,
                "Dev terminal".to_owned(),
                &working_directory,
                terminal_port,
                environment,
                cx,
            )
        });
        terminal.update(cx, |terminal, cx| {
            terminal.apply_font_size(font_size, cx);
            terminal.set_surface_visible(visible);
        });
        let subscription = cx.subscribe(
            &terminal,
            |this, _terminal, event: &TerminalViewEvent, cx| {
                this.handle_terminal_view_event(event, cx);
            },
        );
        self.dev_terminal_subscriptions
            .insert(session_id, subscription);
        let drawer = self
            .dev_terminals
            .entry(workspace_id)
            .or_insert_with(|| DevTerminalDrawer {
                terminals: Vec::new(),
                selected_id: session_id,
                visible: false,
            });
        drawer.terminals.push(terminal.clone());
        drawer.selected_id = session_id;
        terminal
    }

    fn ensure_dev_terminal(&mut self, cx: &mut Context<Self>) -> Option<Entity<TerminalView>> {
        let workspace_id = self.current_workspace_id()?;
        if let Some(terminal) = self.selected_dev_terminal(workspace_id, cx) {
            return Some(terminal);
        }
        Some(self.spawn_dev_terminal(workspace_id, cx))
    }

    fn prune_dev_terminals(&mut self, cx: &mut Context<Self>) {
        let live: HashSet<Uuid> = self
            .snapshot
            .workspace_entries()
            .into_iter()
            .map(|entry| entry.workspace_id)
            .collect();
        let stale: Vec<Uuid> = self
            .dev_terminals
            .keys()
            .copied()
            .filter(|workspace_id| !live.contains(workspace_id))
            .collect();
        for workspace_id in stale {
            self.shutdown_dev_drawer(workspace_id, cx);
        }
    }

    fn shutdown_dev_drawer(&mut self, workspace_id: Uuid, cx: &mut Context<Self>) {
        let Some(drawer) = self.dev_terminals.remove(&workspace_id) else {
            return;
        };
        for terminal in drawer.terminals {
            let session_id = terminal.read(cx).session_id();
            terminal.read(cx).shutdown();
            self.dev_terminal_subscriptions.remove(&session_id);
        }
    }

    fn set_dev_terminal_visible(
        &mut self,
        visible: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace_id) = self.current_workspace_id() else {
            return;
        };
        if visible {
            let Some(terminal) = self.ensure_dev_terminal(cx) else {
                return;
            };
            if let Some(drawer) = self.dev_terminals.get_mut(&workspace_id) {
                drawer.visible = true;
            }
            self.sync_terminal_surface_visibility(cx);
            cx.notify();
            cx.defer_in(window, move |_, window, cx| {
                terminal.read(cx).focus_handle(cx).focus(window);
            });
            return;
        }
        let Some(drawer) = self.dev_terminals.get_mut(&workspace_id) else {
            return;
        };
        if !drawer.visible {
            return;
        }
        drawer.visible = false;
        self.pending_focus_dev_terminal = false;
        self.sync_terminal_surface_visibility(cx);
        self.focus_selected_terminal(window, cx);
        cx.notify();
    }

    fn toggle_dev_terminal(
        &mut self,
        _: &ToggleDevTerminal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_dev_terminal_visible(!self.is_dev_terminal_visible(), window, cx);
    }

    fn add_dev_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace_id) = self.current_workspace_id() else {
            return;
        };
        if let Some(drawer) = self.dev_terminals.get_mut(&workspace_id) {
            drawer.visible = true;
        }
        let terminal = self.spawn_dev_terminal(workspace_id, cx);
        if let Some(drawer) = self.dev_terminals.get_mut(&workspace_id) {
            drawer.visible = true;
        }
        self.sync_terminal_surface_visibility(cx);
        cx.notify();
        cx.defer_in(window, move |_, window, cx| {
            terminal.read(cx).focus_handle(cx).focus(window);
        });
    }

    fn select_dev_terminal_tab(
        &mut self,
        session_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace_id) = self.dev_workspace_for_session(session_id, cx) else {
            return;
        };
        if let Some(drawer) = self.dev_terminals.get_mut(&workspace_id) {
            drawer.selected_id = session_id;
            drawer.visible = true;
        }
        self.sync_terminal_surface_visibility(cx);
        cx.notify();
        if let Some(terminal) = self.selected_dev_terminal(workspace_id, cx) {
            cx.defer_in(window, move |_, window, cx| {
                terminal.read(cx).focus_handle(cx).focus(window);
            });
        }
    }

    fn close_dev_terminal(
        &mut self,
        session_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace_id) = self.dev_workspace_for_session(session_id, cx) else {
            return;
        };
        let Some(index) = self.dev_terminals.get(&workspace_id).and_then(|drawer| {
            drawer
                .terminals
                .iter()
                .position(|terminal| terminal.read(cx).session_id() == session_id)
        }) else {
            return;
        };
        let terminal = self
            .dev_terminals
            .get_mut(&workspace_id)
            .map(|drawer| drawer.terminals.remove(index));
        if let Some(terminal) = terminal {
            terminal.read(cx).shutdown();
        }
        self.dev_terminal_subscriptions.remove(&session_id);
        let empty = self
            .dev_terminals
            .get(&workspace_id)
            .is_some_and(|drawer| drawer.terminals.is_empty());
        if empty {
            self.dev_terminals.remove(&workspace_id);
            self.pending_focus_dev_terminal = false;
            self.sync_terminal_surface_visibility(cx);
            self.focus_selected_terminal(window, cx);
            cx.notify();
            return;
        }
        if let Some(drawer) = self.dev_terminals.get_mut(&workspace_id)
            && drawer.selected_id == session_id
        {
            let next = index.min(drawer.terminals.len() - 1);
            drawer.selected_id = drawer.terminals[next].read(cx).session_id();
        }
        let selected = self.selected_dev_terminal(workspace_id, cx);
        self.sync_terminal_surface_visibility(cx);
        cx.notify();
        if let Some(terminal) = selected {
            cx.defer_in(window, move |_, window, cx| {
                terminal.read(cx).focus_handle(cx).focus(window);
            });
        }
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

    fn split_pane_left(&mut self, _: &SplitPaneLeft, window: &mut Window, cx: &mut Context<Self>) {
        self.split_pane(PaneSplitDirection::Left, window, cx);
    }

    fn split_pane_right(
        &mut self,
        _: &SplitPaneRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.split_pane(PaneSplitDirection::Right, window, cx);
    }

    fn split_pane_up(&mut self, _: &SplitPaneUp, window: &mut Window, cx: &mut Context<Self>) {
        self.split_pane(PaneSplitDirection::Up, window, cx);
    }

    fn split_pane_down(&mut self, _: &SplitPaneDown, window: &mut Window, cx: &mut Context<Self>) {
        self.split_pane(PaneSplitDirection::Down, window, cx);
    }

    fn focus_pane_left(&mut self, _: &FocusPaneLeft, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_pane(PaneFocusDirection::Left, window, cx);
    }

    fn focus_pane_right(
        &mut self,
        _: &FocusPaneRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_pane(PaneFocusDirection::Right, window, cx);
    }

    fn focus_pane_up(&mut self, _: &FocusPaneUp, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_pane(PaneFocusDirection::Up, window, cx);
    }

    fn focus_pane_down(&mut self, _: &FocusPaneDown, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_pane(PaneFocusDirection::Down, window, cx);
    }

    fn previous_pane(&mut self, _: &PreviousPane, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle_pane(-1, window, cx);
    }

    fn next_pane(&mut self, _: &NextPane, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle_pane(1, window, cx);
    }

    fn resize_pane_left(&mut self, _: &ResizePaneLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.resize_pane(PaneResizeDirection::Left, cx);
    }

    fn resize_pane_right(&mut self, _: &ResizePaneRight, _: &mut Window, cx: &mut Context<Self>) {
        self.resize_pane(PaneResizeDirection::Right, cx);
    }

    fn resize_pane_up(&mut self, _: &ResizePaneUp, _: &mut Window, cx: &mut Context<Self>) {
        self.resize_pane(PaneResizeDirection::Up, cx);
    }

    fn resize_pane_down(&mut self, _: &ResizePaneDown, _: &mut Window, cx: &mut Context<Self>) {
        self.resize_pane(PaneResizeDirection::Down, cx);
    }

    fn equalize_panes(&mut self, _: &EqualizePanes, _: &mut Window, cx: &mut Context<Self>) {
        if self.snapshot.equalize_selected_panes() {
            self.persist(cx);
        }
    }

    fn toggle_pane_zoom(
        &mut self,
        _: &TogglePaneZoom,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.snapshot.toggle_selected_pane_zoom() {
            self.sync_terminal_surface_visibility(cx);
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
            self.sync_terminal_surface_visibility(cx);
            self.sync_diff_root(cx);
            self.refresh_project_files(cx);
            self.persist(cx);
            self.focus_selected_terminal(window, cx);
        }
    }

    fn next_workspace(&mut self, _: &NextWorkspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.snapshot.cycle_workspace(1) {
            self.sync_terminal_surface_visibility(cx);
            self.sync_diff_root(cx);
            self.refresh_project_files(cx);
            self.persist(cx);
            self.focus_selected_terminal(window, cx);
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
            self.sync_terminal_surface_visibility(cx);
            self.sync_diff_root(cx);
            self.refresh_project_files(cx);
            self.refresh_sidebar_workspace_meta(cx);
            self.persist(cx);
            self.focus_selected_terminal(window, cx);
        }
    }

    fn select_tab(&mut self, tab_id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        if self.snapshot.select_tab(tab_id) {
            self.sync_terminal_surface_visibility(cx);
            self.sync_diff_root(cx);
            self.refresh_project_files(cx);
            self.refresh_sidebar_workspace_meta(cx);
            self.persist(cx);
            self.focus_selected_terminal(window, cx);
        }
    }

    fn titlebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let left_progress = self.left_sidebar_progress;
        let right_progress = self.right_sidebar_progress;
        // Keep titlebar controls aligned with the framed panels below.
        let left_chrome_width = if left_progress > 0.99 {
            self.left_sidebar_width()
        } else {
            TITLEBAR_CHROME_COLLAPSED
                + (self.left_sidebar_width() - TITLEBAR_CHROME_COLLAPSED) * left_progress
        };
        let right_chrome_width = if right_progress > 0.99 {
            self.right_sidebar_width()
        } else {
            TITLEBAR_RIGHT_CHROME_COLLAPSED
                + (self.right_sidebar_width() - TITLEBAR_RIGHT_CHROME_COLLAPSED) * right_progress
        };
        let right_open = right_progress > 0.5;
        let tabs = self
            .snapshot
            .selected_workspace()
            .map(|workspace| workspace.tabs.clone())
            .unwrap_or_default();
        let selected_tab_id = self
            .snapshot
            .selected_workspace()
            .and_then(|workspace| workspace.selected_tab_id);
        let show_tab_selector = tabs.len() > 1;
        let right_chrome_content = if right_open {
            self.utility_mode_tabs(cx)
        } else {
            div()
                .h_full()
                .flex_1()
                .window_control_area(WindowControlArea::Drag)
                .on_mouse_down(MouseButton::Left, |_, _, _| {
                    crate::infrastructure::window::start_drag();
                })
                .into_any_element()
        };

        let mut center_chrome = div()
            .h_full()
            .flex_1()
            .min_w(px(0.0))
            .flex()
            .items_center()
            .bg(colors().titlebar);
        if show_tab_selector {
            center_chrome = center_chrome.child(self.tab_bar(tabs, selected_tab_id, cx));
        } else {
            center_chrome = center_chrome
                .window_control_area(WindowControlArea::Drag)
                .on_mouse_down(MouseButton::Left, |_, _, _| {
                    crate::infrastructure::window::start_drag();
                });
        }

        div()
            .h(px(TITLEBAR_HEIGHT))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .bg(colors().titlebar)
            .child(
                div()
                    .w(px(left_chrome_width))
                    .h_full()
                    .flex_none()
                    .flex()
                    .items_center()
                    .pl(px(86.0))
                    .gap_1()
                    .bg(colors().titlebar)
                    .child(
                        self.sidebar_button("toggle-left-sidebar", true, cx, |this, _, cx| {
                            if !this.left_sidebar_visible {
                                this.left_sidebar_mode = LeftSidebarMode::Sessions;
                            }
                            this.set_left_sidebar_visible(!this.left_sidebar_visible, true, cx);
                        }),
                    )
                    .child(
                        div()
                            .h_full()
                            .flex_1()
                            .window_control_area(WindowControlArea::Drag)
                            .on_mouse_down(MouseButton::Left, |_, _, _| {
                                crate::infrastructure::window::start_drag();
                            }),
                    ),
            )
            .child(center_chrome)
            .child(
                div()
                    .w(px(right_chrome_width))
                    .h_full()
                    .flex_none()
                    .flex()
                    .items_center()
                    .pr_2()
                    .overflow_hidden()
                    .bg(colors().titlebar)
                    .child(right_chrome_content)
                    .child(self.sidebar_button(
                        "toggle-right-sidebar",
                        false,
                        cx,
                        |this, _, cx| {
                            this.toggle_diff_panel(cx);
                        },
                    )),
            )
    }

    fn sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.left_sidebar_mode {
            LeftSidebarMode::Sessions => self.sessions_sidebar_content(cx),
            LeftSidebarMode::Info => self.info_sidebar_content(cx),
        };
        let full_width = self.left_sidebar_width();
        let width = full_width * self.left_sidebar_progress;
        let show_handle = self.left_sidebar_progress > 0.99;
        // Outer clips to animated width; inner keeps full layout so content doesn't reflow.
        div()
            .w(px(width))
            .h_full()
            .flex_none()
            .relative()
            .overflow_hidden()
            .rounded(px(PANEL_RADIUS))
            .bg(colors().sidebar)
            .border_1()
            .border_color(colors().border_subtle)
            .child(
                div()
                    .w(px(full_width))
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(content),
            )
            .when(show_handle, |sidebar| {
                sidebar.child(self.sidebar_resize_handle(
                    "resize-left-sidebar",
                    SidebarResizeEdge::Left,
                    cx,
                ))
            })
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
                        identity
                            .agent_kind
                            .is_some()
                            .then(|| (sidebar_agent_priority(&identity), identity))
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
                            .font_family("JetBrains Mono")
                            .text_size(px(11.0))
                            .text_color(colors().muted)
                            .child(">_"),
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
                                            .font_family("JetBrains Mono")
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
                        let branch_color = match (
                            meta.map(|m| m.dirty).unwrap_or(false),
                            meta.map(|m| m.behind > 0).unwrap_or(false),
                        ) {
                            (true, _) => colors().warning,
                            (_, true) => colors().accent,
                            _ if selected => colors().muted,
                            _ => colors().subtle,
                        };
                        let path_color = if selected {
                            colors().muted
                        } else {
                            colors().subtle
                        };
                        let title_color = if selected {
                            colors().foreground
                        } else {
                            colors().muted
                        };
                        let agent_color = agent_status_color(
                            agent_identity
                                .as_ref()
                                .and_then(|identity| identity.agent_state),
                            agent_identity
                                .as_ref()
                                .and_then(|identity| identity.agent_attention),
                        )
                        .unwrap_or(if selected {
                            colors().muted
                        } else {
                            colors().subtle
                        });
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
                            branch_color
                        } else {
                            path_color
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
                            .bg(if selected {
                                colors().elevated
                            } else {
                                colors().sidebar
                            })
                            .border_1()
                            .border_color(if selected {
                                colors().border_subtle
                            } else {
                                gpui::rgba(0x00000000)
                            })
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
                            .child(
                                div()
                                    .w(px((self.left_sidebar_tab_text_width() - group_inset)
                                        .max(80.0)))
                                    .flex_none()
                                    .overflow_hidden()
                                    .flex()
                                    .flex_col()
                                    .justify_center()
                                    .gap(px(1.0))
                                    .child(sidebar_tab_line(
                                        &title_label,
                                        title_color,
                                        11.5,
                                        true,
                                        false,
                                    ))
                                    .child(sidebar_tab_line(
                                        &agent_label,
                                        agent_color,
                                        9.5,
                                        true,
                                        false,
                                    ))
                                    .child(sidebar_tab_line(
                                        &location_label,
                                        location_color,
                                        8.5,
                                        false,
                                        true,
                                    )),
                            )
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
        let project_name = project_root
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("FILES")
            .to_uppercase();
        let changed_count = git_statuses.len();

        div()
            .id("project-files-content")
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(colors().panel)
            .child(
                div()
                    .h(px(34.0))
                    .w_full()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(7.0))
                    .px(px(10.0))
                    .child(
                        svg()
                            .path("file-icons/folder.svg")
                            .size(px(14.0))
                            .text_color(colors().folder),
                    )
                    .child(
                        div()
                            .max_w(px(140.0))
                            .truncate()
                            .text_size(px(9.5))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(colors().muted)
                            .child(project_name),
                    )
                    .child(div().h(px(1.0)).flex_1().bg(colors().border_subtle))
                    .when(changed_count > 0, |header| {
                        header.child(
                            div()
                                .flex_none()
                                .font_family("JetBrains Mono")
                                .text_size(px(8.5))
                                .text_color(colors().warning)
                                .child(changed_count.to_string()),
                        )
                    }),
            )
            // File tree
            .child(
                div()
                    .id("project-file-tree")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .px_1()
                    .pt_0()
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
                    .font_family("JetBrains Mono")
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
                                    .font_family("JetBrains Mono")
                                    .text_size(px(8.5))
                                    .text_color(colors().subtle)
                                    .child(path),
                            ),
                    )
            })
            .into_any_element()
    }

    fn settings_modal(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.settings_open {
            return None;
        }
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
                        this.close_settings(cx);
                    }),
                )
                .child(
                    div()
                        .id("settings-modal")
                        .w(px(780.0))
                        .h(px(660.0))
                        .max_w_full()
                        .max_h_full()
                        .mx_4()
                        .rounded(px(12.0))
                        .border_1()
                        .border_color(colors().border_subtle)
                        .bg(colors().panel)
                        .shadow_lg()
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .child(
                            div()
                                .h(px(56.0))
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap_3()
                                .px_5()
                                .border_b_1()
                                .border_color(colors().border_subtle)
                                .child(
                                    div()
                                        .flex_1()
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_size(px(13.0))
                                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                                .text_color(colors().foreground)
                                                .child("Ajustes"),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(9.0))
                                                .text_color(colors().subtle)
                                                .child("Configura tu espacio de trabajo"),
                                        ),
                                )
                                .child(
                                    div()
                                        .px_2()
                                        .py_1()
                                        .rounded(px(4.0))
                                        .border_1()
                                        .border_color(colors().border_subtle)
                                        .text_size(px(8.0))
                                        .text_color(colors().subtle)
                                        .child("ESC"),
                                )
                                .child(self.sidebar_close_button(
                                    "close-settings-modal",
                                    cx,
                                    |this, cx| {
                                        this.close_settings(cx);
                                    },
                                )),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_1()
                                .min_h(px(0.0))
                                .child(self.settings_navigation(cx))
                                .child(self.settings_modal_content(window, cx)),
                        )
                        .child(
                            div()
                                .flex_none()
                                .px_5()
                                .py_2()
                                .border_t_1()
                                .border_color(colors().border_subtle)
                                .text_xs()
                                .text_color(colors().subtle)
                                .child("Los cambios se guardan automáticamente."),
                        ),
                )
                .into_any_element(),
        )
    }

    fn settings_navigation(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut navigation = div()
            .id("settings-navigation")
            .w(px(150.0))
            .flex_none()
            .p_3()
            .flex()
            .flex_col()
            .gap_1()
            .border_r_1()
            .border_color(colors().border_subtle);
        for page in [
            SettingsPage::General,
            SettingsPage::Appearance,
            SettingsPage::Iphone,
            SettingsPage::Agents,
            SettingsPage::Security,
        ] {
            let selected = self.settings_page == page;
            navigation = navigation.child(
                div()
                    .id(page.id())
                    .px_3()
                    .py_2()
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .text_sm()
                    .bg(if selected {
                        colors().selection
                    } else {
                        colors().panel
                    })
                    .text_color(if selected {
                        colors().foreground
                    } else {
                        colors().muted
                    })
                    .hover(|style| style.bg(colors().selection))
                    .child(page.label())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings_page = page;
                        this.remote_forget_confirm = false;
                        cx.notify();
                    })),
            );
        }
        navigation.into_any_element()
    }

    fn settings_modal_content(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let font_size = self.settings.terminal_font_size;
        let hidden = self.settings.show_hidden_files;
        let left_sidebar = self.settings.left_sidebar_visible;
        let git_panel = self.settings.git_panel_visible;
        let appearance = self.appearance_mode();
        let theme_id = self.settings.theme_id.clone();
        let system_dark = ThemeTone::from_window_appearance(window.appearance()) == ThemeTone::Dark;
        let preview_tone = theme::resolve_tone(appearance, system_dark);
        let agent_hooks = self.agent_hook_status.unwrap_or_default();
        let agent_hook_error = self.agent_hook_error.clone();
        let all_agent_hooks_installed = agent_hooks.all_installed();
        let any_agent_hooks_installed = agent_hooks.any_installed();
        let agent_hooks_status = if all_agent_hooks_installed {
            "Configurados"
        } else if any_agent_hooks_installed {
            "Parciales"
        } else {
            "Opcionales"
        };
        div()
            .id(self.settings_page.id())
            .min_w(px(0.0))
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .p_5()
            .flex()
            .flex_col()
            .gap_4()
            .when(self.settings_page == SettingsPage::Iphone, |panel| panel.child(self.remote_settings(cx)))
            .when(self.settings_page == SettingsPage::Appearance, |panel| panel
            .child(self.settings_section_heading(
                "Apariencia",
                "Elige cómo se ve Vibra y ajusta la lectura de la terminal.",
            ))
            .child(
                div()
                    .p_4()
                    .rounded(px(9.0))
                    .border_1()
                    .border_color(colors().border_subtle)
                    .bg(colors().elevated)
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_size(px(10.5))
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .text_color(colors().foreground)
                                            .child("Modo de apariencia"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(9.0))
                                            .text_color(colors().subtle)
                                            .child("Sigue macOS o fija un modo para la aplicación."),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(self.settings_mode_button(
                                        "Sistema",
                                        "settings-appearance-system",
                                        appearance == AppearanceMode::System,
                                        cx,
                                        |this, window, cx| {
                                            this.set_appearance_mode(
                                                AppearanceMode::System,
                                                window,
                                                cx,
                                            );
                                        },
                                    ))
                                    .child(self.settings_mode_button(
                                        "Claro",
                                        "settings-appearance-light",
                                        appearance == AppearanceMode::Light,
                                        cx,
                                        |this, window, cx| {
                                            this.set_appearance_mode(
                                                AppearanceMode::Light,
                                                window,
                                                cx,
                                            );
                                        },
                                    ))
                                    .child(self.settings_mode_button(
                                        "Oscuro",
                                        "settings-appearance-dark",
                                        appearance == AppearanceMode::Dark,
                                        cx,
                                        |this, window, cx| {
                                            this.set_appearance_mode(
                                                AppearanceMode::Dark,
                                                window,
                                                cx,
                                            );
                                        },
                                    )),
                            ),
                    )
                    .child(
                        div().h(px(1.0)).bg(colors().border_subtle),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(colors().foreground)
                                    .child("Tema"),
                            )
                            .child(
                                div()
                                    .text_size(px(9.0))
                                    .text_color(colors().subtle)
                                    .child("La paleta también se aplica al terminal y al resaltado de código."),
                            ),
                    )
                    .child(
                        div()
                            .id("settings-theme-list")
                            .max_h(px(220.0))
                            .pr_1()
                            .overflow_y_scroll()
                            // Nested scroller: without this the parent settings
                            // pane also moves when the wheel is over the themes.
                            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                            .child(self.settings_theme_grid(&theme_id, preview_tone, cx)),
                    ),
            )
            .child(
                div()
                    .p_4()
                    .rounded(px(9.0))
                    .border_1()
                    .border_color(colors().border_subtle)
                    .bg(colors().elevated)
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_size(px(10.5))
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .text_color(colors().foreground)
                                            .child("Tamaño del texto"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(9.0))
                                            .text_color(colors().subtle)
                                            .child("Fuente JetBrains Mono en todas las terminales."),
                                    ),
                            )
                            .child(
                                div()
                                    .w(px(52.0))
                                    .h(px(26.0))
                                    .rounded(px(5.0))
                                    .bg(colors().panel)
                                    .border_1()
                                    .border_color(colors().border_subtle)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .font_family("JetBrains Mono")
                                    .text_size(px(9.0))
                                    .text_color(colors().foreground)
                                    .child(format!("{font_size:.0} px")),
                            )
                            .child(self.settings_button(
                                "−",
                                "settings-font-down",
                                cx,
                                |this, cx| {
                                    this.set_terminal_font_size(
                                        this.settings.terminal_font_size - 1.0,
                                        cx,
                                    );
                                },
                            ))
                            .child(self.settings_button(
                                "Restablecer",
                                "settings-font-reset",
                                cx,
                                |this, cx| {
                                    this.set_terminal_font_size(12.0, cx);
                                },
                            ))
                            .child(self.settings_button(
                                "+",
                                "settings-font-up",
                                cx,
                                |this, cx| {
                                    this.set_terminal_font_size(
                                        this.settings.terminal_font_size + 1.0,
                                        cx,
                                    );
                                },
                            )),
                    ),
            )
            )
            .when(self.settings_page == SettingsPage::General, |panel| panel
            .child(self.settings_section_heading(
                "Al abrir Vibra",
                "Configura qué elementos estarán disponibles al abrir Vibra.",
            ))
            .child(
                div()
                    .rounded(px(9.0))
                    .overflow_hidden()
                    .border_1()
                    .border_color(colors().border_subtle)
                    .bg(colors().elevated)
                    .child(self.settings_toggle_row(
                        SettingsToggleRow {
                            label: "Archivos ocultos",
                            description: "Incluye archivos y carpetas que comienzan con punto.",
                            enabled: hidden,
                            divider: true,
                            id: "settings-hidden",
                        },
                        cx,
                        |this, cx| {
                            this.show_hidden_files = !this.show_hidden_files;
                            this.settings.show_hidden_files = this.show_hidden_files;
                            this.refresh_project_files(cx);
                            this.persist_settings(cx);
                        },
                    ))
                    .child(self.settings_toggle_row(
                        SettingsToggleRow {
                            label: "Lista de sesiones",
                            description: "Muestra la lista de sesiones al abrir la aplicación.",
                            enabled: left_sidebar,
                            divider: true,
                            id: "settings-sidebar-visible",
                        },
                        cx,
                        |this, cx| {
                            this.set_left_sidebar_visible(!this.left_sidebar_visible, true, cx);
                        },
                    ))
                    .child(self.settings_toggle_row(
                        SettingsToggleRow {
                            label: "Panel de archivos y Git",
                            description: "Abre el panel derecho del proyecto al iniciar.",
                            enabled: git_panel,
                            divider: false,
                            id: "settings-git-visible",
                        },
                        cx,
                        |this, cx| {
                            let open = !this.right_sidebar_visible;
                            this.set_right_sidebar_visible(open, true, cx);
                            if open {
                                this.sync_diff_root(cx);
                            }
                        },
                    )),
            )
            )
            .when(self.settings_page == SettingsPage::Agents, |panel| panel
            .child(self.settings_section_heading(
                "Agentes y notificaciones",
                "Sigue el estado de los asistentes que ejecutas dentro de Vibra.",
            ))
            .child(
                div()
                    .p_4()
                    .rounded(px(9.0))
                    .border_1()
                    .border_color(colors().border_subtle)
                    .bg(colors().elevated)
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(10.5))
                                    .text_color(colors().foreground)
                                    .child("Detección automática"),
                            )
                            .child(self.settings_status_chip("Siempre activa", true)),
                    )
                    .child(
                        div()
                            .text_size(px(9.0))
                            .line_height(px(13.0))
                            .text_color(colors().subtle)
                            .child(
                                "Vibra reconoce los agentes que ejecutas en sus terminales y muestra su actividad en panes, tabs y sesiones.",
                            ),
                    )
                    .child(
                        div().h(px(1.0)).bg(colors().border_subtle),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(10.5))
                                    .text_color(colors().foreground)
                                    .child("Hooks para estados precisos"),
                            )
                            .child(self.settings_status_chip(
                                agent_hooks_status,
                                all_agent_hooks_installed,
                            )),
                    )
                    .child(
                        div()
                            .text_size(px(9.0))
                            .line_height(px(13.0))
                            .text_color(colors().subtle)
                            .child(
                                "Opcional: instala hooks para que Claude y Codex informen cuándo trabajan, terminan o piden permiso. Los demás agentes se detectan por el proceso y la pantalla.",
                            ),
                    )
                    .child(self.settings_hook_status_row(
                        "Claude",
                        agent_hooks.claude_installed,
                    ))
                    .child(self.settings_hook_status_row("Codex", agent_hooks.codex_installed))
                    .child(
                        div()
                            .text_size(px(9.0))
                            .line_height(px(13.0))
                            .text_color(colors().subtle)
                            .child(
                                "En Codex tendrás que aprobar la configuración una vez con /hooks.",
                            ),
                    )
                    .when_some(agent_hook_error, |card, error| {
                        card.child(
                            div()
                                .text_size(px(9.0))
                                .line_height(px(13.0))
                                .text_color(colors().danger)
                                .child(error),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(self.settings_primary_button(
                                if all_agent_hooks_installed {
                                    "Actualizar hooks"
                                } else if any_agent_hooks_installed {
                                    "Instalar hooks faltantes"
                                } else {
                                    "Instalar hooks"
                                },
                                "settings-agent-hooks-install",
                                cx,
                                |this, cx| this.install_agent_hooks_from_settings(cx),
                            ))
                            .when(any_agent_hooks_installed, |buttons| {
                                buttons.child(self.settings_button(
                                    "Desactivar",
                                    "settings-agent-hooks-uninstall",
                                    cx,
                                    |this, cx| this.uninstall_agent_hooks_from_settings(cx),
                                ))
                            }),
                    ),
            )
            .child(
                div()
                    .rounded(px(9.0))
                    .overflow_hidden()
                    .border_1()
                    .border_color(colors().border_subtle)
                    .bg(colors().elevated)
                    .child(self.settings_toggle_row(
                        SettingsToggleRow {
                            label: "Notificaciones de actividad",
                            description: "Avisa si un agente termina o necesita atención fuera del pane actual.",
                            enabled: self.settings.agent_notifications,
                            divider: false,
                            id: "settings-agent-notifications",
                        },
                        cx,
                        |this, cx| {
                            this.settings.agent_notifications = !this.settings.agent_notifications;
                            if this.settings.agent_notifications {
                                crate::infrastructure::notifications::request_authorization();
                            }
                            this.persist_settings(cx);
                        },
                    )),
            )
            )
            .when(self.settings_page == SettingsPage::Security, |panel| panel
            .child(self.settings_section_heading(
                "Privacidad de la terminal",
                "Protecciones aplicadas a las integraciones locales de la terminal.",
            ))
            .child(
                div()
                    .p_4()
                    .rounded(px(9.0))
                    .border_1()
                    .border_color(colors().border_subtle)
                    .bg(colors().elevated)
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(10.5))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(colors().foreground)
                                    .child("Lectura del portapapeles (OSC 52)"),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded(px(4.0))
                                    .bg(colors().diff_added_bg)
                                    .text_size(px(8.0))
                                    .text_color(colors().success)
                                    .child("Confirmación obligatoria"),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(9.0))
                            .line_height(px(13.0))
                            .text_color(colors().subtle)
                            .child("Cada lectura requiere confirmación. La comunicación local usa un socket privado y un token distinto por pane."),
                    ),
            )
            )
            .child(div().h(px(1.0)))
            .into_any_element()
    }

    fn remote_result(&mut self, result: anyhow::Result<()>, cx: &mut Context<Self>) {
        match result {
            Err(error) => self.remote_feedback = Some(error.to_string()),
            Ok(()) => {
                self.remote_feedback = None;
                self.persistence_error = None;
            }
        }
        cx.notify();
    }
    fn remote_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        use crate::infrastructure::remote::hub;
        let status = hub().status();
        let mut panel = div()
            .p_4()
            .rounded(px(9.0))
            .border_1()
            .border_color(colors().border_subtle)
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_sm().child("iPhone"))
            .child(
                div()
                    .text_xs()
                    .text_color(colors().subtle)
                    .child("Controla tus terminales desde el iPhone en la misma red Wi-Fi. Sin servidor ni cuenta adicional."),
            )
            .child(div().text_xs().child(if status.paired && !status.enabled {
                "iPhone vinculado · acceso desactivado"
            } else if status.pending.is_some() {
                "Un iPhone espera tu aprobación"
            } else if status.paired {
                "iPhone vinculado"
            } else if status.invitation.is_some() {
                "Escanea el código con Vibra en tu iPhone"
            } else {
                "Vincula tu iPhone para comenzar"
            }));
        if let Some(message) = &self.remote_feedback {
            panel = panel.child(
                div()
                    .p_3()
                    .rounded(px(6.0))
                    .bg(colors().selection)
                    .text_xs()
                    .child(message.clone()),
            );
        }
        if status.paired {
            panel = panel.child(div().text_xs().text_color(colors().subtle)
                .child("Haz clic derecho dentro de una terminal y elige Compartir con iPhone. Puedes recuperar el control desde el Mac en cualquier momento."));
        } else if status.invitation.is_none() && status.pending.is_none() {
            panel = panel
                .child(
                    div()
                        .text_xs()
                        .text_color(colors().subtle)
                        .child("Conecta una vez. Después, elige qué terminales quieres compartir."),
                )
                .child(div().text_xs().child("1 · Crea tu código de vinculación"))
                .child(
                    div()
                        .text_xs()
                        .text_color(colors().subtle)
                        .child("2 · Escanéalo con Vibra en tu iPhone"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(colors().subtle)
                        .child("3 · Acepta el iPhone en este Mac"),
                )
                .child(
                    self.settings_primary_button(
                        "Vincular iPhone",
                        "remote-pair",
                        cx,
                        |this, cx| {
                            this.remote_copied = false;
                            this.remote_result(hub().pair(), cx);
                        },
                    )
                    .h(px(36.0))
                    .border_1()
                    .border_color(colors().accent),
                );
        }
        if let Some(name) = status.pending {
            panel = panel
                .child(format!(
                    "¿Permitir que {name} controle las terminales compartidas?"
                ))
                .child(self.settings_primary_button(
                    "Aceptar y vincular",
                    "remote-approve",
                    cx,
                    |_, cx| {
                        hub().approve(true);
                        cx.notify();
                    },
                ))
                .child(
                    self.settings_button("Rechazar", "remote-reject", cx, |_, cx| {
                        hub().approve(false);
                        cx.notify();
                    }),
                );
        }
        if let Some(invitation) = status.invitation {
            panel = panel
                .child(
                    div()
                        .text_xs()
                        .child("Abre Vibra en tu iPhone y toca Escanear código QR. Después, acepta la conexión aquí. El código dura 5 minutos."),
                )
                .child(self.settings_button(
                    if self.remote_copied { "Invitación copiada" } else { "Copiar invitación" },
                    "remote-copy",
                    cx,
                    |this, cx| {
                        if let Some(value) = hub().status().invitation {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(value));
                            this.remote_copied = true;
                            cx.notify();
                        }
                    },
                ));
            if let Ok(qr) = qrcode::QrCode::new(invitation.as_bytes()) {
                let width = qr.width();
                let modules = qr.to_colors();
                let mut grid = div()
                    .flex()
                    .flex_col()
                    .p_4()
                    .bg(gpui::rgb(0xffffff))
                    .w(px((width * 3 + 32) as f32));
                for row in 0..width {
                    grid = grid.child(div().flex().children((0..width).map(|col| {
                        div().w(px(3.0)).h(px(3.0)).flex_none().bg(gpui::rgb(
                            if modules[row * width + col] == qrcode::Color::Dark {
                                0x000000
                            } else {
                                0xffffff
                            },
                        ))
                    })));
                }
                panel = panel.child(grid);
            }
        }
        if status.paired || status.enabled {
            panel = panel.child(self.settings_toggle_row(
                SettingsToggleRow {
                    label: "Permitir acceso desde el iPhone",
                    description: "Al desactivarlo, el Mac recupera el control. La vinculación se conserva.",
                    enabled: status.enabled,
                    divider: false,
                    id: "remote-enabled-toggle",
                }, cx, |this, cx| {
                    if hub().status().enabled {
                        hub().disable();
                        cx.notify();
                    } else {
                        this.remote_result(hub().enable(), cx);
                    }
                }));
        }
        if status.paired {
            if self.remote_forget_confirm {
                panel = panel.child(div().text_xs().child("El iPhone perderá el acceso. Para volver, tendrás que escanear otro código."))
                        .child(self.settings_button("Confirmar desvinculación", "remote-revoke-confirm", cx, |this,cx| {
                            this.remote_forget_confirm = false;
                            this.remote_result(hub().revoke(),cx);
                        }))
                        .child(self.settings_button("Cancelar", "remote-revoke-cancel", cx, |this,cx| {
                            this.remote_forget_confirm = false; cx.notify();
                        }));
            } else {
                panel = panel.child(self.settings_button(
                    "Desvincular iPhone",
                    "remote-revoke",
                    cx,
                    |this, cx| {
                        this.remote_forget_confirm = true;
                        cx.notify();
                    },
                ));
            }
        }
        panel = panel.child(
            div()
                .text_xs()
                .text_color(colors().subtle)
                .child(status.description),
        );
        panel.into_any_element()
    }

    fn settings_section_heading(
        &self,
        title: &'static str,
        description: &'static str,
    ) -> AnyElement {
        div()
            .px_1()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_size(px(11.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(colors().foreground)
                    .child(title),
            )
            .child(
                div()
                    .text_size(px(9.0))
                    .text_color(colors().subtle)
                    .child(description),
            )
            .into_any_element()
    }

    fn settings_status_chip(&self, label: &'static str, active: bool) -> AnyElement {
        div()
            .px_2()
            .py_1()
            .rounded(px(4.0))
            .bg(if active {
                colors().diff_added_bg
            } else {
                colors().selection
            })
            .text_size(px(8.0))
            .text_color(if active {
                colors().success
            } else {
                colors().muted
            })
            .child(label)
            .into_any_element()
    }

    fn settings_hook_status_row(&self, label: &'static str, installed: bool) -> AnyElement {
        div()
            .flex()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .text_size(px(10.0))
                    .text_color(colors().muted)
                    .child(label),
            )
            .child(self.settings_status_chip(
                if installed {
                    "Instalado"
                } else {
                    "No instalado"
                },
                installed,
            ))
            .into_any_element()
    }

    fn settings_button(
        &self,
        label: &'static str,
        id: &'static str,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .h(px(24.0))
            .px_3()
            .rounded(px(5.0))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(colors().selection)
            .border_1()
            .border_color(colors().border_subtle)
            .text_size(px(9.0))
            .text_color(colors().muted)
            .hover(|button| button.bg(colors().hover).text_color(colors().foreground))
            .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
            .child(label)
    }

    fn settings_primary_button(
        &self,
        label: &'static str,
        id: &'static str,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .h(px(26.0))
            .px_3()
            .rounded(px(5.0))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(colors().accent)
            .text_size(px(9.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(colors().background)
            .hover(|button| button.opacity(0.88))
            .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
            .child(label)
    }

    fn settings_mode_button(
        &self,
        label: &'static str,
        id: &'static str,
        selected: bool,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .h(px(26.0))
            .px_3()
            .rounded(px(5.0))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if selected {
                colors().selection
            } else {
                colors().elevated
            })
            .border_1()
            .border_color(if selected {
                colors().accent
            } else {
                colors().border_subtle
            })
            .text_size(px(9.0))
            .text_color(if selected {
                colors().foreground
            } else {
                colors().muted
            })
            .hover(|button| button.bg(colors().hover).text_color(colors().foreground))
            .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
            .child(label)
    }

    fn settings_theme_grid(
        &self,
        active_theme_id: &str,
        preview_tone: ThemeTone,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut grid = div().flex().flex_col().gap_2();
        // Two columns of theme cards.
        let mut row = div().flex().gap_2();
        for (index, family) in theme::built_in_themes().iter().enumerate() {
            let selected = family.id == active_theme_id;
            let theme_id = family.id;
            let [sidebar, panel, accent] = family.preview(preview_tone);
            let card = div()
                .id(SharedString::from(format!("settings-theme-{theme_id}")))
                .flex_1()
                .min_w(px(0.0))
                .px_2()
                .py_1()
                .rounded(px(6.0))
                .cursor_pointer()
                .border_1()
                .border_color(if selected {
                    colors().accent
                } else {
                    colors().border_subtle
                })
                .bg(if selected {
                    colors().selection
                } else {
                    colors().elevated
                })
                .hover(|card| card.bg(colors().hover))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_theme_id(theme_id, window, cx);
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(div().size(px(9.0)).rounded_full().bg(sidebar))
                                .child(div().size(px(9.0)).rounded_full().bg(panel))
                                .child(div().size(px(9.0)).rounded_full().bg(accent)),
                        )
                        .child(
                            div()
                                .text_size(px(10.0))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(colors().foreground)
                                .child(family.label),
                        ),
                );
            row = row.child(card);
            if index % 2 == 1 {
                grid = grid.child(row);
                row = div().flex().gap_2();
            }
        }
        if theme::built_in_themes().len() % 2 == 1 {
            grid = grid.child(row.child(div().flex_1()));
        }
        grid.into_any_element()
    }

    fn settings_toggle_row(
        &self,
        row: SettingsToggleRow,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        div()
            .id(row.id)
            .min_h(px(52.0))
            .px_3()
            .py_2()
            .flex()
            .items_center()
            .cursor_pointer()
            .when(row.divider, |row| {
                row.border_b_1().border_color(colors().border_subtle)
            })
            .hover(|row| row.bg(colors().hover))
            .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(10.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(colors().foreground)
                            .child(row.label),
                    )
                    .child(
                        div()
                            .text_size(px(8.5))
                            .text_color(colors().subtle)
                            .child(row.description),
                    ),
            )
            .child(
                div()
                    .w(px(30.0))
                    .h(px(16.0))
                    .p(px(2.0))
                    .rounded_full()
                    .flex()
                    .justify_end()
                    .bg(if row.enabled {
                        colors().success
                    } else {
                        colors().selection
                    })
                    .when(!row.enabled, |toggle| toggle.justify_start())
                    .child(div().size(px(12.0)).rounded_full().bg(colors().foreground)),
            )
    }

    fn center_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let canvas = self.terminal_canvas(cx).into_any_element();
        let drawer = self.dev_terminal_drawer(cx);
        div()
            .flex_1()
            .min_w(px(360.0))
            .h_full()
            .flex()
            .flex_col()
            .min_h(px(0.0))
            .overflow_hidden()
            .rounded(px(PANEL_RADIUS))
            .border_1()
            .border_color(colors().border_subtle)
            .bg(colors().terminal)
            .child(canvas)
            .children(drawer)
    }

    fn dev_terminal_drawer(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.is_dev_terminal_visible() {
            return None;
        }
        let workspace_id = self.current_workspace_id()?;
        let drawer = self.dev_terminals.get(&workspace_id)?;
        let selected_id = drawer.selected_id;
        let tabs: Vec<(Uuid, String, Entity<TerminalView>)> = drawer
            .terminals
            .iter()
            .enumerate()
            .map(|(index, terminal)| {
                let session_id = terminal.read(cx).session_id();
                let cwd = terminal.read(cx).current_working_directory();
                let name = directory_basename(&cwd.to_string_lossy());
                let title = if name == "—" {
                    format!("Terminal {}", index + 1)
                } else {
                    name
                };
                (session_id, title, terminal.clone())
            })
            .collect();
        let selected_terminal = tabs
            .iter()
            .find(|(session_id, _, _)| *session_id == selected_id)
            .or(tabs.first())
            .map(|(_, _, terminal)| terminal.clone())?;
        Some(
            div()
                .id("dev-terminal-drawer")
                .h(px(DEV_TERMINAL_HEIGHT))
                .min_h(px(120.0))
                .flex_none()
                .flex()
                .flex_col()
                .overflow_hidden()
                .border_t_1()
                .border_color(colors().border_subtle)
                .bg(colors().terminal)
                .child(
                    div()
                        .h(px(28.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .px(px(8.0))
                        .bg(colors().panel)
                        .border_b_1()
                        .border_color(colors().border_subtle)
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .overflow_x_hidden()
                                .children(tabs.into_iter().map(|(session_id, title, _)| {
                                    let selected = session_id == selected_id;
                                    div()
                                        .id(SharedString::from(format!(
                                            "dev-terminal-tab-{session_id}"
                                        )))
                                        .h(px(22.0))
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .px(px(8.0))
                                        .rounded(px(5.0))
                                        .cursor_pointer()
                                        .bg(if selected {
                                            colors().selection
                                        } else {
                                            gpui::rgba(0x00000000)
                                        })
                                        .text_color(if selected {
                                            colors().foreground
                                        } else {
                                            colors().muted
                                        })
                                        .hover(|tab| {
                                            tab.bg(colors().hover).text_color(colors().foreground)
                                        })
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.select_dev_terminal_tab(session_id, window, cx);
                                        }))
                                        .on_mouse_down(
                                            MouseButton::Right,
                                            cx.listener(
                                                move |this, event: &MouseDownEvent, _, cx| {
                                                    this.open_context_menu(
                                                        ContextMenuKind::Pane { session_id },
                                                        f32::from(event.position.x),
                                                        f32::from(event.position.y),
                                                        cx,
                                                    );
                                                    cx.stop_propagation();
                                                },
                                            ),
                                        )
                                        .child(
                                            div()
                                                .max_w(px(140.0))
                                                .truncate()
                                                .text_size(px(10.5))
                                                .font_weight(if selected {
                                                    gpui::FontWeight::MEDIUM
                                                } else {
                                                    gpui::FontWeight::NORMAL
                                                })
                                                .child(title),
                                        )
                                        .child(
                                            div()
                                                .id(SharedString::from(format!(
                                                    "dev-terminal-close-{session_id}"
                                                )))
                                                .size(px(14.0))
                                                .rounded(px(3.0))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .text_size(px(11.0))
                                                .text_color(colors().subtle)
                                                .hover(|button| {
                                                    button
                                                        .bg(colors().hover)
                                                        .text_color(colors().foreground)
                                                })
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |_, _, _, cx| {
                                                        cx.stop_propagation();
                                                    }),
                                                )
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        cx.stop_propagation();
                                                        this.close_dev_terminal(
                                                            session_id, window, cx,
                                                        );
                                                    },
                                                ))
                                                .child("×"),
                                        )
                                })),
                        )
                        .child(
                            div()
                                .id("dev-terminal-add")
                                .size(px(22.0))
                                .flex_none()
                                .rounded(px(5.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .text_size(px(14.0))
                                .text_color(colors().muted)
                                .hover(|button| {
                                    button.bg(colors().hover).text_color(colors().foreground)
                                })
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.add_dev_terminal(window, cx);
                                }))
                                .child("+"),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h(px(0.0))
                        .overflow_hidden()
                        .child(selected_terminal),
                )
                .into_any_element(),
        )
    }

    fn tab_bar(
        &mut self,
        tabs: Vec<TabSnapshot>,
        selected_tab_id: Option<Uuid>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let can_reorder = tabs.len() > 1;
        let tab_count = tabs.len();
        let dragging_tab = match self.reorder_drag {
            Some(ReorderDrag::Tab(id)) if cx.has_active_drag() => Some(id),
            _ => None,
        };
        let tab_ids = tabs.iter().map(|tab| tab.id).collect::<Vec<_>>();
        let tab_list = div()
            .h_full()
            .flex_1()
            .min_w(px(0.0))
            .flex()
            .items_center()
            .justify_center()
            .gap(px(4.0))
            .overflow_x_hidden()
            .when(can_reorder, |list| {
                list.child(div().w(px(24.0)).flex_none())
            })
            .children(tabs.into_iter().enumerate().map(|(index, tab)| {
                let tab_id = tab.id;
                let after_tab_id = tab_ids.get(index + 1).copied();
                let tab_order = tab_ids.clone();
                let selected = Some(tab_id) == selected_tab_id;
                let session = tab
                    .sessions
                    .iter()
                    .find(|session| Some(session.id) == tab.selected_session_id)
                    .or_else(|| tab.sessions.first());
                let identity = session.map(|session| self.pane_identity(session, index, cx));
                let title = identity
                    .as_ref()
                    .map(|identity| identity.title.clone())
                    .unwrap_or_else(|| format!("Terminal {}", index + 1));
                let title = if tab_count > 1 {
                    format!("{title} {}", index + 1)
                } else {
                    title
                };
                let shortcut = (index < 9).then(|| format!("⌘{}", index + 1));
                let pane_count = tab.sessions.len();
                // A tab identifies its focused pane. Background agents keep their state in
                // their own pane headers instead of changing an unrelated tab dot.
                let agent_color = identity.as_ref().and_then(|identity| {
                    agent_status_color(identity.agent_state, identity.agent_attention)
                });
                let drag = TabDrag {
                    tab_id,
                    title: title.clone(),
                    selected,
                    shortcut: shortcut.clone(),
                    tab_count,
                };
                let is_source = dragging_tab == Some(tab_id);
                div()
                    .id(SharedString::from(format!("tab-{tab_id}")))
                    .h(px(26.0))
                    .min_w(px(0.0))
                    .flex_1()
                    .relative()
                    .flex()
                    .items_center()
                    .justify_center()
                    .px(px(10.0))
                    .rounded(px(7.0))
                    .when(can_reorder, |tab| tab.cursor_move())
                    .when(!can_reorder, |tab| tab.cursor_pointer())
                    .bg(if selected {
                        colors().selection
                    } else {
                        gpui::rgba(0x00000000)
                    })
                    .border_1()
                    .border_color(if selected {
                        colors().muted
                    } else {
                        colors().border_subtle
                    })
                    .text_color(if selected {
                        colors().foreground
                    } else {
                        colors().muted
                    })
                    .hover(|tab| {
                        if selected {
                            tab
                        } else {
                            tab.bg(colors().hover).text_color(colors().foreground)
                        }
                    })
                    .active(|tab| tab.opacity(0.88))
                    .when(is_source, |tab| tab.opacity(0.45))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.reorder_drag = Some(ReorderDrag::Tab(tab_id));
                            this.select_tab(tab_id, window, cx);
                        }),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_tab(tab_id, window, cx);
                    }))
                    .when(can_reorder, |tab| {
                        tab.on_drag(drag, |drag, _, window, cx| {
                            // Tabs fill the central chrome, so derive the preview width from the
                            // live window and tab count instead of rendering a compact chip.
                            let window_width: f32 = window.bounds().size.width.into();
                            let width =
                                (window_width * (0.52 / drag.tab_count as f32)).clamp(160.0, 420.0);
                            cx.new(|_| TabDragView {
                                title: drag.title.clone(),
                                selected: drag.selected,
                                shortcut: drag.shortcut.clone(),
                                width,
                            })
                        })
                        .can_drop(move |value, _, _| {
                            value
                                .downcast_ref::<TabDrag>()
                                .is_some_and(|drag| drag.tab_id != tab_id)
                        })
                        .on_drop(cx.listener(move |this, drag: &TabDrag, window, cx| {
                            let dragged_from_left = tab_order
                                .iter()
                                .position(|id| *id == drag.tab_id)
                                .is_some_and(|source_index| source_index < index);
                            let before_tab_id = if dragged_from_left {
                                after_tab_id
                            } else {
                                Some(tab_id)
                            };
                            this.reorder_tab(drag.tab_id, before_tab_id, window, cx);
                        }))
                        .drag_over::<TabDrag>(|style, _, _, _| style.bg(colors().hover))
                    })
                    .when_some(agent_color, |tab, color| {
                        tab.child(
                            div()
                                .absolute()
                                .left(px(12.0))
                                .size(px(6.0))
                                .rounded_full()
                                .bg(color),
                        )
                    })
                    .child(
                        div()
                            .min_w(px(0.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .min_w(px(0.0))
                                    .truncate()
                                    .text_center()
                                    .text_size(px(12.0))
                                    .font_weight(if selected {
                                        gpui::FontWeight::MEDIUM
                                    } else {
                                        gpui::FontWeight::NORMAL
                                    })
                                    .child(title),
                            )
                            .when(pane_count > 1, |label| {
                                label.child(
                                    div()
                                        .flex_none()
                                        .px(px(5.0))
                                        .py(px(1.0))
                                        .rounded(px(4.0))
                                        .bg(colors().elevated)
                                        .font_family("JetBrains Mono")
                                        .text_size(px(8.5))
                                        .text_color(colors().subtle)
                                        .child(format!("{pane_count} panes")),
                                )
                            }),
                    )
                    .when_some(shortcut, |tab, shortcut| {
                        tab.child(
                            div()
                                .absolute()
                                .right(px(10.0))
                                .font_family("JetBrains Mono")
                                .text_size(px(9.5))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(if selected {
                                    colors().muted
                                } else {
                                    colors().subtle
                                })
                                .child(shortcut),
                        )
                    })
            }))
            .when(can_reorder, |list| {
                list.child(
                    div()
                        .id("tab-drop-end")
                        .h_full()
                        .w(px(24.0))
                        .flex_none()
                        .can_drop(|value, _, _| value.downcast_ref::<TabDrag>().is_some())
                        .drag_over::<TabDrag>(|style, _, _, _| style.bg(colors().hover))
                        .on_drop(cx.listener(|this, drag: &TabDrag, window, cx| {
                            this.reorder_tab(drag.tab_id, None, window, cx);
                        })),
                )
            });

        div()
            .h_full()
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .px(px(12.0))
            .bg(colors().terminal)
            .child(tab_list)
    }

    fn render_pane_layout(
        &mut self,
        layout: &PaneLayoutSnapshot,
        path: Vec<PaneBranch>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match layout {
            PaneLayoutSnapshot::Terminal { id } => {
                let session_id = *id;
                let terminal = self.terminals.get(&session_id).cloned();
                let drag_preview = terminal
                    .as_ref()
                    .map(|terminal| terminal.read(cx).drag_preview())
                    .unwrap_or_else(TerminalDragPreview::empty);
                let pane_count = self
                    .snapshot
                    .selected_tab()
                    .map_or(1, |tab| tab.sessions.len());
                let framed = self
                    .snapshot
                    .selected_tab()
                    .is_some_and(|tab| tab.sessions.len() > 1 && tab.zoomed_session_id.is_none());
                let selected = self
                    .snapshot
                    .selected_tab()
                    .is_some_and(|tab| tab.selected_session_id == Some(session_id));
                let identity = self.snapshot.selected_tab().and_then(|tab| {
                    let index = tab
                        .sessions
                        .iter()
                        .position(|session| session.id == session_id)?;
                    tab.sessions
                        .get(index)
                        .map(|session| self.pane_identity(session, index, cx))
                });
                let can_drag = self
                    .snapshot
                    .selected_tab()
                    .is_some_and(|tab| tab.zoomed_session_id.is_none() && pane_count > 1);
                let dragging_pane = match self.reorder_drag {
                    Some(ReorderDrag::Pane(id)) if cx.has_active_drag() => Some(id),
                    _ => None,
                };
                let is_source = dragging_pane == Some(session_id);
                let drag = PaneDrag {
                    session_id,
                    preview: drag_preview,
                };
                div()
                    .id(SharedString::from(format!("pane-{session_id}")))
                    .size_full()
                    .min_w(px(80.0))
                    .min_h(px(48.0))
                    .relative()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .bg(colors().terminal)
                    .when(framed, |pane| {
                        pane.rounded(px(PANEL_RADIUS - 2.0))
                            .border_1()
                            .border_color(colors().border_subtle)
                    })
                    .when(is_source, |pane| pane.opacity(0.55))
                    .when(can_drag, |pane| {
                        pane.cursor_move()
                            .on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.preview.clone()))
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            if can_drag {
                                this.reorder_drag = Some(ReorderDrag::Pane(session_id));
                            }
                            this.close_context_menu(cx);
                            this.select_terminal(session_id, window, cx);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            this.select_terminal(session_id, window, cx);
                            let x: f32 = event.position.x.into();
                            let y: f32 = event.position.y.into();
                            this.open_context_menu(ContextMenuKind::Pane { session_id }, x, y, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .when(pane_count > 1, |pane| {
                        pane.when_some(identity, |pane, identity| {
                            let has_agent = identity.agent_kind.is_some();
                            pane.child(
                                div()
                                    .h(px(25.0))
                                    .w_full()
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .gap(px(7.0))
                                    .px(px(8.0))
                                    .overflow_hidden()
                                    .bg(if selected {
                                        colors().elevated
                                    } else {
                                        colors().terminal
                                    })
                                    .border_b_1()
                                    .border_color(if selected {
                                        colors().accent
                                    } else {
                                        colors().border_subtle
                                    })
                                    .child(agent_compact_badge(
                                        identity.agent_kind.as_deref(),
                                        identity.agent_state,
                                        identity.agent_attention,
                                        selected,
                                    ))
                                    .child(
                                        div()
                                            .min_w(px(0.0))
                                            .max_w(px(220.0))
                                            .truncate()
                                            .font_family("JetBrains Mono")
                                            .text_size(px(10.5))
                                            .font_weight(if has_agent || selected {
                                                gpui::FontWeight::MEDIUM
                                            } else {
                                                gpui::FontWeight::NORMAL
                                            })
                                            .text_color(if selected {
                                                colors().foreground
                                            } else {
                                                colors().muted
                                            })
                                            .child(identity.title),
                                    )
                                    .when_some(identity.detail, |header, detail| {
                                        header.child(
                                            div()
                                                .min_w(px(0.0))
                                                .flex_1()
                                                .truncate()
                                                .text_right()
                                                .font_family("JetBrains Mono")
                                                .text_size(px(9.0))
                                                .text_color(colors().subtle)
                                                .child(detail),
                                        )
                                    }),
                            )
                        })
                    })
                    .when_some(terminal, |pane, terminal| {
                        pane.child(
                            div()
                                .flex_1()
                                .min_h(px(0.0))
                                .min_w(px(0.0))
                                .overflow_hidden()
                                .child(terminal),
                        )
                    })
                    // GPUI registers drop listeners while laying out the element. Keep this
                    // transparent target mounted before a drag begins; mounting it only once a
                    // drag is active means it never receives the drop that started that drag.
                    .child(
                        div()
                            .id(SharedString::from(format!("pane-drop-{session_id}")))
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .can_drop(move |value, _, _| {
                                value
                                    .downcast_ref::<PaneDrag>()
                                    .is_some_and(|drag| drag.session_id != session_id)
                            })
                            .drag_over::<PaneDrag>(|style, _, _, _| {
                                style
                                    .border_2()
                                    .border_color(colors().accent)
                                    .bg(colors().selection)
                            })
                            .on_drop(cx.listener(move |this, drag: &PaneDrag, window, cx| {
                                this.swap_panes(drag.session_id, session_id, window, cx);
                            })),
                    )
                    .into_any_element()
            }
            PaneLayoutSnapshot::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let axis = *axis;
                let fraction = f32::from(*ratio) / 10_000.0;
                let mut first_path = path.clone();
                first_path.push(PaneBranch::First);
                let mut second_path = path.clone();
                second_path.push(PaneBranch::Second);
                let first = self.render_pane_layout(first, first_path, cx);
                let second = self.render_pane_layout(second, second_path, cx);
                let divider_id = format!(
                    "pane-divider-{}",
                    path.iter()
                        .map(|branch| match branch {
                            PaneBranch::First => '0',
                            PaneBranch::Second => '1',
                        })
                        .collect::<String>()
                );
                let drag = PaneDividerDrag {
                    path: path.clone(),
                    axis,
                };
                let divider = div()
                    .id(SharedString::from(divider_id))
                    .flex_none()
                    .bg(colors().background)
                    .hover(|divider| divider.bg(colors().muted))
                    .when(axis == WorkspaceSplitAxis::Horizontal, |divider| {
                        divider.w(px(PANEL_GAP)).h_full().cursor_ew_resize()
                    })
                    .when(axis == WorkspaceSplitAxis::Vertical, |divider| {
                        divider.h(px(PANEL_GAP)).w_full().cursor_ns_resize()
                    })
                    .on_drag(drag, move |drag, _, _, cx| {
                        cx.new(|_| PaneDividerDragView { axis: drag.axis })
                    });
                let listener_path = path;
                div()
                    .size_full()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .flex()
                    .when(axis == WorkspaceSplitAxis::Horizontal, |split| {
                        split.flex_row()
                    })
                    .when(axis == WorkspaceSplitAxis::Vertical, |split| {
                        split.flex_col()
                    })
                    .on_drag_move(cx.listener(
                        move |this, event: &DragMoveEvent<PaneDividerDrag>, _, cx| {
                            let drag = event.drag(cx).clone();
                            if drag.path != listener_path || drag.axis != axis {
                                return;
                            }
                            let (offset, length): (f32, f32) = match axis {
                                WorkspaceSplitAxis::Horizontal => (
                                    (event.event.position.x - event.bounds.left()).into(),
                                    event.bounds.size.width.into(),
                                ),
                                WorkspaceSplitAxis::Vertical => (
                                    (event.event.position.y - event.bounds.top()).into(),
                                    event.bounds.size.height.into(),
                                ),
                            };
                            if length <= 0.0 {
                                return;
                            }
                            let ratio = ((offset / length) * 10_000.0).round() as u16;
                            if this.snapshot.set_selected_split_ratio(&drag.path, ratio) {
                                this.pane_resize_dirty = true;
                                cx.notify();
                            }
                        },
                    ))
                    .child(
                        div()
                            .min_w(px(0.0))
                            .min_h(px(0.0))
                            .flex_none()
                            .when(axis == WorkspaceSplitAxis::Horizontal, |pane| {
                                pane.w(relative(fraction)).h_full()
                            })
                            .when(axis == WorkspaceSplitAxis::Vertical, |pane| {
                                pane.h(relative(fraction)).w_full()
                            })
                            .child(first),
                    )
                    .child(divider)
                    .child(div().min_w(px(0.0)).min_h(px(0.0)).flex_1().child(second))
                    .into_any_element()
            }
        }
    }

    fn terminal_canvas(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.snapshot.selected_tab().cloned();
        let panes = tab.as_ref().map(|tab| {
            if let Some(zoomed_id) = tab.zoomed_session_id {
                self.render_pane_layout(&PaneLayoutSnapshot::terminal(zoomed_id), Vec::new(), cx)
            } else {
                self.render_pane_layout(&tab.layout, Vec::new(), cx)
            }
        });
        let is_empty = panes.is_none();
        let zoomed = tab.as_ref().and_then(|tab| tab.zoomed_session_id).is_some();
        let framed_panes = tab
            .as_ref()
            .is_some_and(|tab| tab.sessions.len() > 1 && tab.zoomed_session_id.is_none());

        // Full-bleed: no padding. Agent TUIs paint pure black; any inset against
        // chrome makes the background look “cut off”.
        div()
            .flex_1()
            .min_h(px(0.0))
            .relative()
            .overflow_hidden()
            .bg(colors().terminal)
            .when(framed_panes, |canvas| {
                canvas.p(px(PANEL_GAP)).bg(colors().background)
            })
            .when_some(panes, |canvas, panes| canvas.child(panes))
            .when(zoomed, |canvas| {
                canvas.child(
                    div()
                        .absolute()
                        .left_3()
                        .bottom_3()
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .bg(colors().elevated)
                        .text_xs()
                        .text_color(colors().muted)
                        .child("Pane ampliado · ⇧⌘↵ restaurar"),
                )
            })
            .when(is_empty, |canvas| {
                canvas.child(
                    div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap_3()
                        .child(
                            div()
                                .size(px(32.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(13.0))
                                .text_color(colors().muted)
                                .child(">_"),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(colors().muted)
                                .child("Ninguna terminal seleccionada"),
                        ),
                )
            })
    }

    fn utility_mode_tabs(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let mode = self.right_sidebar_mode;
        let modes = [
            (RightSidebarMode::Files, "Files", "chrome-icons/files.svg"),
            (RightSidebarMode::Diff, "Git", "chrome-icons/git-branch.svg"),
        ];

        div()
            .h_full()
            .flex_1()
            .min_w(px(0.0))
            .flex()
            .items_center()
            .pl_3()
            .child(div().flex().items_center().children(modes.into_iter().map(
                |(item_mode, label, icon)| {
                    let selected = item_mode == mode;
                    div()
                        .id(SharedString::from(format!("utility-mode-{label}")))
                        .h(px(26.0))
                        .relative()
                        .w(px(32.0))
                        .mr_2()
                        .rounded(px(7.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .bg(if selected {
                            colors().selection
                        } else {
                            gpui::rgba(0x00000000)
                        })
                        .text_color(if selected {
                            colors().foreground
                        } else {
                            colors().subtle
                        })
                        .hover(|tab| tab.bg(colors().hover).text_color(colors().foreground))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.right_sidebar_mode = item_mode;
                            match item_mode {
                                RightSidebarMode::Files => this.refresh_project_files(cx),
                                RightSidebarMode::Diff => {
                                    this.sync_diff_root(cx);
                                    this.diff_view.update(cx, |diff_view, cx| {
                                        diff_view.refresh_now(cx);
                                    });
                                }
                            }
                            cx.notify();
                        }))
                        .child(svg().path(icon).size(px(15.0)).text_color(if selected {
                            colors().foreground
                        } else {
                            colors().subtle
                        }))
                },
            )))
            .child(self.ide_button(cx))
            .child(
                div()
                    .h_full()
                    .flex_1()
                    .window_control_area(WindowControlArea::Drag)
                    .on_mouse_down(MouseButton::Left, |_, _, _| {
                        crate::infrastructure::window::start_drag();
                    }),
            )
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

        // Outer clips to animated width; inner keeps the full panel layout.
        div()
            .w(px(width))
            .h_full()
            .flex_none()
            .relative()
            .overflow_hidden()
            .rounded(px(PANEL_RADIUS))
            .bg(colors().panel)
            .border_1()
            .border_color(colors().border_subtle)
            .child(
                div()
                    .w(px(full_width))
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(content),
            )
            .when(show_handle, |sidebar| {
                sidebar.child(self.sidebar_resize_handle(
                    "resize-right-sidebar",
                    SidebarResizeEdge::Right,
                    cx,
                ))
            })
    }

    fn palette_modal(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let mode = self.palette_mode?;
        let items = self.palette_items();
        let selected = self.palette_selected.min(items.len().saturating_sub(1));
        let query = self.palette_query.clone();
        let placeholder = match mode {
            PaletteMode::Commands => "Buscar comandos…",
            PaletteMode::Files => "Abrir archivo…",
        };
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_start()
                .justify_center()
                .pt(px(86.0))
                .bg(colors().overlay())
                .child(
                    div()
                        .w(px(560.0))
                        .max_w_full()
                        .max_h(px(460.0))
                        .mx_4()
                        .rounded_lg()
                        .border_1()
                        .border_color(colors().border_subtle)
                        .bg(colors().elevated)
                        .shadow_lg()
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .child(
                            div()
                                .h(px(48.0))
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_4()
                                .border_b_1()
                                .border_color(colors().border_subtle)
                                .child(
                                    div()
                                        .font_family("JetBrains Mono")
                                        .text_color(colors().subtle)
                                        .child(">"),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .font_family("JetBrains Mono")
                                        .text_size(px(12.0))
                                        .text_color(if query.is_empty() {
                                            colors().subtle
                                        } else {
                                            colors().foreground
                                        })
                                        .child(if query.is_empty() {
                                            placeholder.to_owned()
                                        } else {
                                            query
                                        }),
                                )
                                .child(div().text_xs().text_color(colors().subtle).child("esc")),
                        )
                        .child(
                            div()
                                .id("palette-results")
                                .flex_1()
                                .min_h(px(0.0))
                                .overflow_y_scroll()
                                .py_2()
                                .children(items.into_iter().enumerate().map(|(index, item)| {
                                    let active = index == selected;
                                    let action = item.action.clone();
                                    div()
                                        .id(SharedString::from(format!("palette-item-{index}")))
                                        .h(px(38.0))
                                        .mx_2()
                                        .px_3()
                                        .rounded(px(6.0))
                                        .flex()
                                        .items_center()
                                        .gap_3()
                                        .cursor_pointer()
                                        .bg(if active {
                                            colors().selection
                                        } else {
                                            colors().elevated
                                        })
                                        .hover(|row| row.bg(colors().hover))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.execute_palette_action(action.clone(), window, cx);
                                        }))
                                        .child(
                                            div()
                                                .w(px(18.0))
                                                .text_center()
                                                .font_family("JetBrains Mono")
                                                .text_color(if active {
                                                    colors().muted
                                                } else {
                                                    colors().subtle
                                                })
                                                .child(if active { "›" } else { "·" }),
                                        )
                                        .child(
                                            div()
                                                .min_w(px(0.0))
                                                .flex_1()
                                                .truncate()
                                                .text_size(px(11.0))
                                                .text_color(if active {
                                                    colors().foreground
                                                } else {
                                                    colors().muted
                                                })
                                                .child(item.label),
                                        )
                                        .child(
                                            div()
                                                .font_family("JetBrains Mono")
                                                .text_size(px(8.5))
                                                .text_color(colors().subtle)
                                                .child(item.detail),
                                        )
                                })),
                        )
                        .when(self.palette_items().is_empty(), |palette| {
                            palette.child(
                                div()
                                    .h(px(80.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_sm()
                                    .text_color(colors().subtle)
                                    .child("Sin resultados"),
                            )
                        })
                        .child(
                            div()
                                .h(px(28.0))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_end()
                                .px_3()
                                .border_t_1()
                                .border_color(colors().border_subtle)
                                .text_xs()
                                .text_color(colors().subtle)
                                .child("↑↓ navegar · ↵ ejecutar"),
                        ),
                )
                .into_any_element(),
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
            ContextMenuKind::Pane { session_id } => vec![
                (
                    if crate::infrastructure::remote::hub().shared(*session_id) {
                        "Dejar de compartir con iPhone"
                    } else {
                        "Compartir con iPhone"
                    },
                    ContextMenuAction::ShareRemote,
                    false,
                ),
                (
                    "Recuperar control del iPhone",
                    ContextMenuAction::ReclaimRemote,
                    false,
                ),
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
                                                    .font_family("JetBrains Mono")
                                                    .text_size(px(10.5))
                                                    .font_weight(gpui::FontWeight::MEDIUM)
                                                    .text_color(colors().foreground)
                                                    .child(identity.title),
                                            )
                                            .when_some(identity.detail, |column, detail| {
                                                column.child(
                                                    div()
                                                        .truncate()
                                                        .font_family("JetBrains Mono")
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
                                .font_family("JetBrains Mono")
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

    fn sidebar_close_button(
        &self,
        id: &'static str,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .size(px(18.0))
            .flex_none()
            .rounded(px(4.0))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_size(px(13.0))
            .text_color(colors().subtle)
            .hover(|close| close.bg(colors().hover).text_color(colors().foreground))
            .active(|close| close.opacity(0.72))
            .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
            .child("×")
    }

    fn sidebar_icon(left: bool) -> Div {
        let panel = div()
            .w(px(4.0))
            .h_full()
            .flex_none()
            .bg(colors().foreground);
        let content = div().h_full().flex_1();
        let icon = div()
            .w(px(14.0))
            .h(px(12.0))
            .flex()
            .overflow_hidden()
            .rounded(px(2.0))
            .border_1()
            .border_color(colors().muted);

        if left {
            icon.child(panel).child(content)
        } else {
            icon.child(content).child(panel)
        }
    }

    fn sidebar_button(
        &self,
        id: &'static str,
        left: bool,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .size(px(24.0))
            .flex_none()
            .rounded(px(5.0))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .bg(colors().titlebar)
            .hover(|button| button.bg(colors().hover))
            .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
            .child(Self::sidebar_icon(left))
    }

    fn ide_button(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id("open-ide")
            .h(px(26.0))
            .relative()
            .w(px(32.0))
            .mr_2()
            .rounded(px(7.0))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .bg(gpui::rgba(0x00000000))
            .text_color(colors().subtle)
            .hover(|button| button.bg(colors().hover).text_color(colors().foreground))
            .on_click(cx.listener(|this, _, window, cx| {
                this.open_ide(&OpenIde, window, cx);
            }))
            .child(
                svg()
                    .path("chrome-icons/open-external.svg")
                    .size(px(15.0))
                    .text_color(colors().subtle),
            )
    }

    fn ide_menu_overlay(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.ide_menu_open.then(|| {
            let right = (self.right_sidebar_width() - 164.0).max(8.0);
            div()
                .absolute()
                .inset_0()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.ide_menu_open = false;
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .id("ide-menu")
                        .absolute()
                        .top(px(34.0))
                        .right(px(right))
                        .min_w(px(190.0))
                        .py_1()
                        .rounded(px(8.0))
                        .border_1()
                        .border_color(colors().border_subtle)
                        .bg(colors().elevated)
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .px_3()
                                .py_2()
                                .text_size(px(9.0))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(colors().subtle)
                                .child("ABRIR CARPETA EN"),
                        )
                        .children(self.installed_editors.clone().into_iter().enumerate().map(
                            |(index, editor)| {
                                let label = editor.name;
                                div()
                                    .id(SharedString::from(format!("ide-menu-item-{index}")))
                                    .h(px(32.0))
                                    .mx_1()
                                    .px_3()
                                    .rounded(px(5.0))
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .cursor_pointer()
                                    .text_size(px(11.0))
                                    .text_color(colors().foreground)
                                    .hover(|item| item.bg(colors().hover))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.open_with_editor(editor.clone(), cx);
                                    }))
                                    .child(
                                        svg()
                                            .path("chrome-icons/open-external.svg")
                                            .size(px(13.0))
                                            .text_color(colors().subtle),
                                    )
                                    .child(label)
                            },
                        )),
                )
                .into_any_element()
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
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .gap(px(PANEL_GAP))
            .px(px(PANEL_GAP))
            .pb(px(PANEL_GAP))
            .on_drag_move(cx.listener(Self::on_sidebar_resize_move));
        // Keep sidebars mounted while progress > 0 so close animations can finish.
        if self.left_sidebar_progress > 0.001 {
            layout = layout.child(self.sidebar(cx));
        }
        layout = layout.child(self.center_panel(cx));
        if self.right_sidebar_progress > 0.001 {
            layout = layout.child(self.right_sidebar(cx));
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

mod automation;
mod chrome;
mod files;

pub(crate) use automation::*;
pub(crate) use chrome::*;
pub(crate) use files::*;

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
            "src"
        );
        assert_eq!(
            tab_display_title(
                None,
                Some("claude --dangerously-skip-permissions"),
                Some("/Users/demo/Dev/Vibra"),
                0,
            ),
            "claude --dangerously-…"
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
                                git_port: Arc::new(GitCliPort),
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

    #[test]
    fn sidebar_agent_line_includes_model_only_when_reported() {
        assert_eq!(
            sidebar_agent_line(Some("Codex"), None, Some(AgentRuntimeState::Working), None),
            "Codex · working"
        );
        assert_eq!(
            sidebar_agent_line(
                Some("Codex"),
                Some("gpt-5"),
                Some(AgentRuntimeState::Working),
                None
            ),
            "Codex · gpt-5 · working"
        );
        assert_eq!(
            sidebar_location_line(Some("main"), "~/Dev/Vibra"),
            "main  ·  ~/Dev/Vibra"
        );
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
                                git_port: Arc::new(GitCliPort),
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
                                git_port: Arc::new(GitCliPort),
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
