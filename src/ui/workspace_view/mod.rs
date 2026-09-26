//! Workspace state and coordination. Feature modules share this view's state:
//! settings owns its pages and automation resolves agent activity.
//! None of them introduces a second workspace model.

mod automation;
mod automations_page;
mod chrome;
mod context_menu;
mod drag;
mod explorer;
mod files;
mod inbox;
mod input;
mod navigation;
mod notes;
mod palette;
mod panes;
mod persistence;
mod projects;
mod settings;
mod status_bar;
mod storage;
mod tabs;
mod terminals;
mod titlebar;

use automation::HookAgentPresence;
use automations_page::AutomationForm;
use chrome::*;
pub(crate) use drag::*;
pub(crate) use files::{file_tree_icon, file_tree_icon_color};
use persistence::{FinishError, PersistenceQueue};
use settings::SettingsPage;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    Context, DragMoveEvent, Entity, FocusHandle, IntoElement, MouseButton, ParentElement, Render,
    SharedString, Styled, Subscription, Task, Timer, Window, div, prelude::*, px,
};
use uuid::Uuid;

use crate::domain::inbox::Inbox;
use crate::domain::library::Library;
use crate::domain::workspace::{PaneSplitDirection, WorkspaceSnapshot};
use crate::infrastructure::automation::{
    AgentAttention, AgentHookStatus, AgentRuntimeState, AutomationServer, agent_hook_status,
};
use crate::infrastructure::editor::InstalledEditor;
use crate::infrastructure::library::LibraryRepository;
use crate::infrastructure::notifications::AgentActivitySnapshot;
use crate::infrastructure::persistence::WorkspaceRepository;
use crate::infrastructure::settings::{
    AppSettings, MAX_LEFT_SIDEBAR_WIDTH, MAX_RIGHT_SIDEBAR_WIDTH, MIN_LEFT_SIDEBAR_WIDTH,
    MIN_RIGHT_SIDEBAR_WIDTH, SettingsRepository,
};
use crate::ports::files::{FileEntry, FileSystemPort};
use crate::ports::git::GitPort;
use crate::ports::terminal::TerminalAgentPresence;
use crate::ports::terminal::TerminalPort;
use crate::ui::diff_view::{DiffFileIndexView, DiffView, DiffViewEvent};
use crate::ui::terminal::{TerminalInsertStatus, TerminalView};
use crate::ui::theme::{self, colors, surface, window_surface};
use crate::{
    CloseTerminal, NewTerminalTab, NextProject, PreviousProject, ShowSettings, ToggleLeftSidebar,
    ToggleRightSidebar,
};

/// Titlebar chrome width when the left sidebar is fully collapsed.
const TITLEBAR_CHROME_COLLAPSED: f32 = 184.0;
/// Titlebar chrome width when the right sidebar is fully collapsed (toggle only).
const TITLEBAR_RIGHT_CHROME_COLLAPSED: f32 = 44.0;
const TITLEBAR_HEIGHT: f32 = 40.0;
/// Open/close duration — short enough to feel snappy, long enough to read as motion.
const SIDEBAR_ANIM_DURATION: Duration = Duration::from_millis(160);
/// ~60 fps ticks; only runs while a sidebar is mid-animation.
const SIDEBAR_ANIM_FRAME: Duration = Duration::from_millis(16);
#[derive(Clone)]
struct PaneIdentity {
    title: String,
    detail: Option<String>,
    agent_kind: Option<String>,
    agent_state: Option<AgentRuntimeState>,
    agent_attention: Option<AgentAttention>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkspaceSection {
    Workspace,
    Inbox,
    Notes,
    Automations,
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
    AddProject,
    SelectProject(Uuid),
    NewTerminalTab,
    OpenIde,
    Split(PaneSplitDirection),
    EqualizePanes,
    TogglePaneZoom,
    ToggleWorkspacePanel,
    ShowFiles,
    ShowSettings,
    ShowSection(WorkspaceSection),
    NewNote,
    NewAutomation,
    RunAutomation(Uuid),
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
    Pane { session_id: Uuid },
    Project { project_id: Uuid },
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
    Pane { session_id: Uuid },
    Project { project_id: Uuid },
    NewFile { directory: PathBuf },
    NewFolder { directory: PathBuf },
}

#[derive(Debug, Clone)]
struct RenamePrompt {
    kind: RenamePromptKind,
    value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContextMenuAction {
    Rename,
    AddProject,
    NewTab,
    AssociateFolder,
    RevealProject,
    RemoveProject,
    ToggleProjectPin,
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
    branch_summary: Option<(PathBuf, crate::ports::git::GitBranchSummary)>,
    _status_task: Option<Task<()>>,
    diff_view: Entity<DiffView>,
    diff_file_index: Entity<DiffFileIndexView>,
    _diff_subscription: Subscription,
    pending_focus_session: Option<Uuid>,
    terminals: HashMap<Uuid, Entity<TerminalView>>,
    terminal_subscriptions: HashMap<Uuid, Subscription>,
    /// External paste token → destination pane. DiffView owns the comments.
    pending_review_pastes: HashMap<Uuid, Uuid>,
    automation_tokens: HashMap<Uuid, Uuid>,
    automation_socket: Option<PathBuf>,
    _automation_server: Option<AutomationServer>,
    _automation_task: Option<gpui::Task<()>>,
    agent_presence: HashMap<Uuid, TerminalAgentPresence>,
    hook_agent_presence: HashMap<Uuid, HookAgentPresence>,
    /// Display names assigned by the user or an automation. Agent identity
    /// and process lifetime do not own these names; closing the pane does.
    pane_names: HashMap<Uuid, String>,
    agent_activity_seen: HashMap<Uuid, AgentActivitySnapshot>,
    agent_hook_status: Option<AgentHookStatus>,
    agent_hook_error: Option<SharedString>,
    window_is_active: bool,
    focus_handle: FocusHandle,
    left_sidebar_visible: bool,
    /// Visual open amount for the left sidebar (`0.0` closed … `1.0` open).
    left_sidebar_progress: f32,
    workspace_section: WorkspaceSection,
    /// The open review's tab is in front of the terminal tabs.
    review_tab_active: bool,
    navigation: tabs::Navigation,
    /// Agent and automation events, newest last; lives only while the app runs.
    inbox: Inbox,
    /// Notes and automations, saved to `library.json`.
    library: Library,
    library_repository: Option<LibraryRepository>,
    library_error: Option<SharedString>,
    library_load_error: Option<SharedString>,
    library_save_error: Option<SharedString>,
    library_generation: u64,
    _library_task: Option<Task<()>>,
    selected_note_id: Option<Uuid>,
    note_editing: bool,
    automation_form: Option<AutomationForm>,
    _automation_scheduler: Option<Task<()>>,
    expanded_directories: HashSet<PathBuf>,
    project_files_root: Option<PathBuf>,
    project_files: Arc<Vec<ProjectFileRow>>,
    selected_file_path: Option<PathBuf>,
    file_error: Option<SharedString>,
    palette_mode: Option<PaletteMode>,
    palette_query: String,
    palette_selected: usize,
    palette_files: Vec<PathBuf>,
    palette_loading: bool,
    palette_error: Option<SharedString>,
    settings_open: bool,
    settings_page: SettingsPage,
    theme_query: String,
    context_menu: Option<ContextMenuState>,
    ide_menu_open: bool,
    ide_discovering: bool,
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
    workspace_save_error: Option<SharedString>,
    settings_save_error: Option<SharedString>,
    workspace_load_error: Option<SharedString>,
    settings_load_error: Option<SharedString>,
    persistence_queue: Option<PersistenceQueue>,
    _persistence_result_task: Option<Task<()>>,
    persist_generation: u64,
    _persist_task: Option<Task<()>>,
    settings_generation: u64,
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
        let (mut snapshot, mut persistence_error, first_launch, workspace_load_error) =
            match repository.load() {
                Ok(Some(snapshot)) => (snapshot, None, false, None),
                Ok(None) => (WorkspaceSnapshot::default(), None, true, None),
                Err(error) => {
                    let message: SharedString = format!(
                        concat!(
                            "No se pudo restaurar el workspace: {error}. ",
                            "Los cambios no se guardarán hasta reparar el archivo."
                        ),
                        error = error
                    )
                    .into();
                    (WorkspaceSnapshot::default(), None, false, Some(message))
                }
            };
        theme::refresh_user_themes();
        let (settings, settings_load_error) = match settings_repository.load() {
            Ok(settings) => (settings, None),
            Err(error) => {
                let message: SharedString = format!(
                    "No se pudieron cargar los settings: {error}. Los cambios no se guardarán hasta reparar el archivo."
                )
                .into();
                (AppSettings::default(), Some(message))
            }
        };
        let library_repository = settings_repository
            .directory()
            .map(LibraryRepository::in_directory);
        let (library, library_load_error) = match library_repository
            .as_ref()
            .map(|repo| repo.load())
        {
            Some(Ok(library)) => (library, None),
            Some(Err(error)) => (
                Library::default(),
                Some(SharedString::from(format!(
                    "No se pudieron cargar las notas y automatizaciones: {error}. No se guardarán cambios hasta reparar el archivo."
                ))),
            ),
            None => (Library::default(), None),
        };
        // Earlier versions kept several sessions per project; the UI now has
        // one row of tabs per project, so their tabs are merged on load.
        let consolidated =
            workspace_load_error.is_none() && snapshot.consolidate_project_sessions();
        let relocated =
            launch_directory.is_dir() && snapshot.relocate_root(Path::new("/"), &launch_directory);
        let mut snapshot_changed = consolidated || relocated;
        if first_launch {
            let project = snapshot.add_project(&launch_directory);
            snapshot.open_tab_in_project(project, true);
            snapshot_changed = true;
        }
        if snapshot_changed
            && workspace_load_error.is_none()
            && let Err(error) = repository.save(&snapshot)
        {
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
            .and_then(|project| project.directory().map(PathBuf::from))
            .unwrap_or_else(|| launch_directory.clone());
        let diff_view = cx.new(|cx| {
            let mut view = DiffView::new(diff_root, git_port.clone(), cx);
            view.set_preferences(
                settings.diff_split,
                settings.diff_wrap,
                settings.diff_font_size,
                cx,
            );
            view
        });
        let diff_file_index = cx.new(|cx| DiffFileIndexView::new(diff_view.clone(), cx));
        let diff_subscription = cx.subscribe(
            &diff_view,
            |this, _diff_view, event: &DiffViewEvent, cx| match event {
                DiffViewEvent::ReviewOpened => {
                    this.review_tab_active = true;
                    this.sync_terminal_surface_visibility(cx);
                    cx.notify();
                }
                DiffViewEvent::Changed => {
                    if !this.diff_view.read(cx).review_expanded() {
                        this.review_tab_active = false;
                    }
                    this.sync_terminal_surface_visibility(cx);
                    this.sync_git_panel_visibility(cx);
                    cx.notify();
                }
                DiffViewEvent::ReturnToTerminal => {
                    if this.workspace_section == WorkspaceSection::Workspace {
                        this.pending_focus_session =
                            this.snapshot.selected_session().map(|session| session.id);
                        cx.notify();
                    }
                }
                DiffViewEvent::RunInTerminal { title, command } => {
                    if let Some(project_id) = this.snapshot.selected_project_id
                        && let Err(reason) =
                            this.run_in_new_tab(project_id, title, command, true, cx)
                    {
                        this.persistence_error = Some(reason.into());
                    }
                    cx.notify();
                }
                DiffViewEvent::PreferencesChanged { split, wrap } => {
                    this.settings.diff_split = *split;
                    this.settings.diff_wrap = *wrap;
                    this.persist_settings(cx);
                }
                DiffViewEvent::SendReview {
                    prompt,
                    delivery_id,
                } => {
                    let status = this.send_review_to_agent(prompt, *delivery_id, cx);
                    if status == TerminalInsertStatus::Pending {
                        return;
                    }
                    let diff_view = this.diff_view.clone();
                    let delivery_id = *delivery_id;
                    cx.spawn(async move |_, cx| {
                        let _ = diff_view.update(cx, |view, cx| {
                            view.resolve_review_delivery(
                                delivery_id,
                                status == TerminalInsertStatus::Accepted,
                                cx,
                            );
                        });
                    })
                    .detach();
                }
            },
        );
        let (agent_hook_status, agent_hook_error) = match agent_hook_status() {
            Ok(status) => (Some(status), None),
            Err(error) => (
                None,
                Some(format!("No se pudo consultar las integraciones: {error}").into()),
            ),
        };
        let (persistence_queue, persistence_result_task) = match PersistenceQueue::start(
            repository.clone(),
            settings_repository.clone(),
            library_repository.clone(),
        ) {
            Ok((queue, results)) => {
                let task = cx.spawn(async move |this, cx| {
                    while let Ok(result) = results.recv().await {
                        if this
                            .update(cx, |this, cx| this.apply_persistence_result(result, cx))
                            .is_err()
                        {
                            break;
                        }
                    }
                });
                (Some(queue), Some(task))
            }
            Err(error) => {
                if persistence_error.is_none() {
                    persistence_error =
                        Some(format!("Guardado en segundo plano no disponible: {error}").into());
                }
                (None, None)
            }
        };
        let release_subscription = cx.on_release(|this, _| {
            let workspace = (this.persist_generation > 0 && this.workspace_load_error.is_none())
                .then(|| (this.persist_generation, this.snapshot.clone()));
            let settings = ((this.settings_generation > 0
                || this.window_size_persist_generation > 0)
                && this.settings_load_error.is_none())
            .then(|| (this.settings_generation, this.settings.clone()));
            let library = (this.library_generation > 0 && this.library_load_error.is_none())
                .then(|| (this.library_generation, this.library.clone()));
            if let Some(queue) = &this.persistence_queue {
                match queue.finish(workspace, settings, library) {
                    Ok(()) => {}
                    Err(FinishError::Save(error)) => eprintln!("{error}"),
                    Err(FinishError::Unavailable) => this.save_final_direct(),
                }
            } else {
                this.save_final_direct();
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
            branch_summary: None,
            _status_task: None,
            diff_view,
            diff_file_index,
            _diff_subscription: diff_subscription,
            pending_focus_session: None,
            terminals: HashMap::new(),
            terminal_subscriptions: HashMap::new(),
            pending_review_pastes: HashMap::new(),
            automation_tokens: HashMap::new(),
            automation_socket,
            _automation_server: automation_server,
            _automation_task: automation_task,
            agent_presence: HashMap::new(),
            hook_agent_presence: HashMap::new(),
            pane_names: HashMap::new(),
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
            workspace_section: WorkspaceSection::Workspace,
            review_tab_active: false,
            navigation: tabs::Navigation::default(),
            inbox: Inbox::default(),
            library,
            library_repository,
            library_error: None,
            library_load_error,
            library_save_error: None,
            library_generation: 0,
            _library_task: None,
            selected_note_id: None,
            note_editing: false,
            automation_form: None,
            _automation_scheduler: None,
            expanded_directories: HashSet::new(),
            project_files_root: None,
            project_files: Arc::new(Vec::new()),
            selected_file_path: None,
            file_error: None,
            palette_mode: None,
            palette_query: String::new(),
            palette_selected: 0,
            palette_files: Vec::new(),
            palette_loading: false,
            palette_error: None,
            settings_open: false,
            settings_page: SettingsPage::General,
            theme_query: String::new(),
            context_menu: None,
            ide_menu_open: false,
            ide_discovering: false,
            installed_editors: Vec::new(),
            ide_icons: HashMap::new(),
            rename_prompt: None,
            right_sidebar_visible: settings.right_sidebar_visible,
            right_sidebar_progress: if settings.right_sidebar_visible {
                1.0
            } else {
                0.0
            },
            right_sidebar_mode: RightSidebarMode::Files,
            sidebar_anim_token: 0,
            _sidebar_anim_task: None,
            initial_terminal_focus_pending: true,
            pane_resize_dirty: false,
            sidebar_resize_dirty: false,
            reorder_drag: None,
            persistence_error,
            workspace_save_error: workspace_load_error.clone(),
            settings_save_error: settings_load_error.clone(),
            workspace_load_error,
            settings_load_error,
            persistence_queue,
            _persistence_result_task: persistence_result_task,
            persist_generation: 0,
            _persist_task: None,
            settings_generation: 0,
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
        view.sync_git_panel_visibility(cx);
        view.start_automation_scheduler(cx);
        view.start_status_poll(cx);
        view
    }

    fn sync_git_panel_visibility(&self, cx: &mut Context<Self>) {
        let visible = self.has_project_context()
            && self.workspace_section == WorkspaceSection::Workspace
            && (self.review_visible(cx)
                || self.right_sidebar_visible
                || self.right_sidebar_progress > 0.001);
        self.diff_view
            .update(cx, |diff_view, cx| diff_view.set_panel_visible(visible, cx));
    }

    fn sync_diff_root(&self, cx: &mut Context<Self>) {
        if !self.has_project_context() {
            self.diff_view
                .update(cx, |diff, cx| diff.set_review_expanded(false, cx));
            self.sync_git_panel_visibility(cx);
            return;
        }
        let root = self.project_root();
        self.diff_view
            .update(cx, |diff_view, cx| diff_view.set_root(root, cx));
    }

    /// The project folder is stable even when a terminal changes its cwd.
    fn project_root(&self) -> PathBuf {
        self.snapshot
            .selected_project()
            .and_then(|project| project.directory())
            .map(PathBuf::from)
            .or_else(|| {
                self.snapshot
                    .selected_session()
                    .map(|session| PathBuf::from(&session.working_directory))
            })
            .unwrap_or_else(|| self.launch_directory.clone())
    }

    fn has_project_context(&self) -> bool {
        self.snapshot.selected_project().is_some_and(|project| {
            project.directory().is_some() || self.snapshot.selected_session().is_some()
        })
    }

    fn toggle_workspace_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace_section != WorkspaceSection::Workspace {
            self.set_workspace_mode(self.right_sidebar_mode, cx);
            self.focus_selected_terminal(window, cx);
            return;
        }
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
        if self.right_sidebar_visible == visible {
            self.sync_git_panel_visibility(cx);
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
                            this.sync_git_panel_visibility(cx);
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

    fn close_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.diff_view
            .update(cx, |diff, cx| diff.set_review_expanded(false, cx));
        self.sync_terminal_surface_visibility(cx);
        self.sync_git_panel_visibility(cx);
        self.focus_selected_terminal(window, cx);
        cx.notify();
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
            self.show_terminal_tab(window, cx);
        }
        cx.notify();
    }

    fn left_sidebar_width(&self) -> f32 {
        self.settings.left_sidebar_width
    }

    fn right_sidebar_width(&self) -> f32 {
        self.settings.right_sidebar_width
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

    fn new_terminal_tab(
        &mut self,
        _: &NewTerminalTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_terminal_tab_in_project(window, cx);
    }

    /// `⌘T` / `⌘N`: a new tab in the selected project, or a folder picker
    /// when there is no project yet.
    fn open_terminal_tab_in_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.snapshot.selected_project_id {
            Some(project_id) => self.open_project_tab(project_id, window, cx),
            None => self.choose_project_folder(None, true, window, cx),
        }
    }

    fn close_terminal(&mut self, _: &CloseTerminal, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace_section != WorkspaceSection::Workspace {
            self.select_section(WorkspaceSection::Workspace, window, cx);
            return;
        }
        if self.review_visible(cx) {
            self.close_review(window, cx);
            return;
        }
        if self.snapshot.close_selected_terminal() {
            self.reconcile_terminal_views(cx);
            self.sync_diff_root(cx);
            self.refresh_project_files(cx);
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_workspace_panel(window, cx);
    }

    fn previous_project(
        &mut self,
        _: &PreviousProject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_project(-1, window, cx);
    }

    fn next_project(&mut self, _: &NextProject, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle_project(1, window, cx);
    }

    /// Moves through projects in sidebar order (pinned first).
    fn cycle_project(&mut self, offset: isize, window: &mut Window, cx: &mut Context<Self>) {
        let (pinned, others): (Vec<Uuid>, Vec<Uuid>) = self
            .snapshot
            .projects
            .iter()
            .map(|project| project.id)
            .partition(|id| self.settings.pinned_project_ids.contains(id));
        let order: Vec<Uuid> = pinned.into_iter().chain(others).collect();
        if order.is_empty() {
            return;
        }
        let current = self
            .snapshot
            .selected_project_id
            .and_then(|id| order.iter().position(|item| *item == id))
            .unwrap_or(0) as isize;
        let next = (current + offset).rem_euclid(order.len() as isize) as usize;
        self.select_project(order[next], window, cx);
    }

    fn select_tab(&mut self, tab_id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        if self.snapshot.select_tab(tab_id) {
            self.show_terminal_tab(window, cx);
        }
    }

    fn sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let content = self.global_sidebar_content(cx);
        let full_width = self.left_sidebar_width();
        let width = full_width * self.left_sidebar_progress;
        let show_handle = self.left_sidebar_progress > 0.99;
        // The native backdrop is tinted once underneath this panel.
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

    fn right_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = self.right_sidebar_mode;
        let full_width = self.right_sidebar_width();
        let width = full_width * self.right_sidebar_progress;
        let show_handle = self.right_sidebar_progress > 0.99;
        let content = if !self.has_project_context() {
            div()
                .p_3()
                .text_size(px(12.0))
                .text_color(colors().muted)
                .child(if self.snapshot.selected_project().is_some() {
                    "Asocia una carpeta a este proyecto"
                } else {
                    "Selecciona un proyecto"
                })
                .into_any_element()
        } else {
            match mode {
                RightSidebarMode::Files => self.files_sidebar_content(cx),
                RightSidebarMode::Diff => self.diff_file_index.clone().into_any_element(),
            }
        };

        let content = div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.utility_mode_tabs(cx))
            .child(content);
        clipped_width_panel(width, full_width, colors().sidebar, content).when(
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

    fn error_banner(&self) -> Option<impl IntoElement> {
        let errors: Vec<_> = [
            self.persistence_error.as_ref(),
            self.workspace_save_error.as_ref(),
            self.settings_save_error.as_ref(),
            self.library_load_error.as_ref(),
            self.library_save_error.as_ref(),
            self.library_error.as_ref(),
        ]
        .into_iter()
        .flatten()
        .map(ToString::to_string)
        .collect();
        (!errors.is_empty()).then(|| {
            let error = errors.join(" · ");
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
                .child(
                    div()
                        .size(px(5.0))
                        .flex_none()
                        .rounded_full()
                        .bg(colors().danger),
                )
                .child(div().min_w(px(0.0)).flex_1().truncate().child(error))
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
        let mut body = div()
            .id("vibra-root")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::add_project))
            .on_action(cx.listener(Self::new_terminal_tab))
            .on_action(cx.listener(Self::close_terminal))
            .on_action(cx.listener(Self::toggle_left_sidebar))
            .on_action(cx.listener(Self::toggle_right_sidebar))
            .on_action(cx.listener(Self::previous_project))
            .on_action(cx.listener(Self::next_project))
            .on_action(cx.listener(Self::go_to_tab))
            .on_action(cx.listener(Self::navigate_back))
            .on_action(cx.listener(Self::navigate_forward))
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
            .bg(window_surface());

        self.record_navigation(cx);
        body = body.child(self.titlebar(cx));

        if let Some(banner) = self.error_banner() {
            body = body.child(banner);
        }

        let expanded_review = self.has_project_context() && self.review_visible(cx);
        let mut layout = div()
            .id("workspace-columns")
            .relative()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .on_drag_move(cx.listener(Self::on_sidebar_resize_move));
        // Keep both navigators visible while reviewing code in the center.
        if self.left_sidebar_progress > 0.001 {
            layout = layout.child(self.sidebar(cx));
        }
        if self.workspace_section == WorkspaceSection::Workspace {
            let review_pane = || {
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .h_full()
                    .overflow_hidden()
                    .bg(surface(colors().panel))
            };
            if expanded_review && self.diff_view.read(cx).review_focused() {
                layout = layout.child(review_pane().child(self.diff_view.clone()));
            } else if expanded_review {
                // The review opens beside the terminal, like an editor split.
                let terminal = self.center_panel(window, cx).into_any_element();
                let review = review_pane()
                    .child(self.diff_view.clone())
                    .into_any_element();
                layout = layout.child(self.review_split(terminal, review, cx));
            } else {
                layout = layout.child(self.center_panel(window, cx));
            }
            if self.right_sidebar_progress > 0.001 {
                layout = layout.child(self.right_sidebar(cx));
            }
        } else {
            layout = layout.child(self.global_section_content(cx));
        }

        body = body.child(layout);
        body = body.child(self.status_bar(cx));
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
mod tests;
