use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    AnyElement, Context, Div, DragMoveEvent, Entity, FocusHandle, Focusable, IntoElement,
    KeyDownEvent, MouseButton, MouseUpEvent, ParentElement, Render, SharedString, Stateful, Styled,
    Subscription, Window, WindowControlArea, div, prelude::*, px, relative,
};
use uuid::Uuid;

use crate::domain::workspace::{
    PaneBranch, PaneFocusDirection, PaneLayoutSnapshot, PaneResizeDirection, PaneSplitDirection,
    TabSnapshot, WorkspaceSnapshot, WorkspaceSplitAxis,
};
use crate::infrastructure::automation::{
    AgentRuntimeState, AutomationCommand, AutomationDirection, AutomationIncoming,
    AutomationResponse, AutomationServer,
};
use crate::infrastructure::persistence::WorkspaceRepository;
use crate::infrastructure::settings::{AppSettings, SettingsRepository};
use crate::ports::files::{FileEntry, FileEntryKind, FileSystemPort};
use crate::ports::git::GitPort;
use crate::ports::terminal::TerminalPort;
use crate::ports::terminal::{TerminalAgentPresence, TerminalAgentState};
use crate::ui::diff_view::{DiffView, DiffViewEvent};
use crate::ui::editor::{EditorView, EditorViewEvent};
use crate::ui::terminal::{TerminalView, TerminalViewEvent};
use crate::ui::theme::DARK;
use crate::{
    CloseTerminal, EqualizePanes, FocusPaneDown, FocusPaneLeft, FocusPaneRight, FocusPaneUp,
    NewTerminalTab, NewWorkspace, NextPane, NextWorkspace, PreviousPane, PreviousWorkspace,
    QuickOpen, ResizePaneDown, ResizePaneLeft, ResizePaneRight, ResizePaneUp, SplitPaneDown,
    SplitPaneLeft, SplitPaneRight, SplitPaneUp, ToggleCommandPalette, TogglePaneZoom,
    ToggleRightSidebar,
};

#[derive(Clone)]
struct PaneDividerDrag {
    path: Vec<PaneBranch>,
    axis: WorkspaceSplitAxis,
}

struct PaneDividerDragView {
    axis: WorkspaceSplitAxis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LeftSidebarMode {
    Sessions,
    Info,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RightSidebarMode {
    Files,
    Diff,
}

#[derive(Debug, Clone)]
struct ProjectFileRow {
    entry: FileEntry,
    depth: usize,
    expanded: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FilePromptKind {
    NewFile,
    NewDirectory,
    Rename,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaletteMode {
    Commands,
    Files,
}

#[derive(Debug, Clone)]
enum PaletteAction {
    NewTerminalTab,
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
struct FilePrompt {
    kind: FilePromptKind,
    directory: PathBuf,
    target: Option<PathBuf>,
    value: String,
}

impl Render for PaneDividerDragView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .when(self.axis == WorkspaceSplitAxis::Horizontal, |line| {
                line.w(px(3.0)).h(px(44.0))
            })
            .when(self.axis == WorkspaceSplitAxis::Vertical, |line| {
                line.w(px(44.0)).h(px(3.0))
            })
            .rounded_full()
            .bg(DARK.success)
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
    diff_view: Entity<DiffView>,
    _diff_subscription: Subscription,
    editor: Option<Entity<EditorView>>,
    editor_subscription: Option<Subscription>,
    terminals: HashMap<Uuid, Entity<TerminalView>>,
    terminal_subscriptions: HashMap<Uuid, Subscription>,
    automation_tokens: HashMap<Uuid, Uuid>,
    automation_socket: Option<PathBuf>,
    _automation_server: Option<AutomationServer>,
    _automation_task: Option<gpui::Task<()>>,
    agent_presence: HashMap<Uuid, TerminalAgentPresence>,
    explicit_agent_states: HashMap<Uuid, AgentRuntimeState>,
    focus_handle: FocusHandle,
    left_sidebar_visible: bool,
    left_sidebar_mode: LeftSidebarMode,
    expanded_directories: HashSet<PathBuf>,
    project_files: Vec<ProjectFileRow>,
    selected_file_path: Option<PathBuf>,
    show_hidden_files: bool,
    file_error: Option<SharedString>,
    file_prompt: Option<FilePrompt>,
    pending_file_trash: Option<PathBuf>,
    palette_mode: Option<PaletteMode>,
    palette_query: String,
    palette_selected: usize,
    palette_files: Vec<PathBuf>,
    right_sidebar_visible: bool,
    right_sidebar_mode: RightSidebarMode,
    initial_terminal_focus_pending: bool,
    pane_resize_dirty: bool,
    persistence_error: Option<SharedString>,
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
        let diff_view = cx.new(|cx| DiffView::new(diff_root, git_port, cx));
        let diff_subscription = cx.subscribe(
            &diff_view,
            |_this, _diff_view, _event: &DiffViewEvent, cx| cx.notify(),
        );
        let mut view = Self {
            snapshot,
            repository,
            settings_repository,
            settings: settings.clone(),
            launch_directory,
            terminal_port,
            file_port,
            diff_view,
            _diff_subscription: diff_subscription,
            editor: None,
            editor_subscription: None,
            terminals: HashMap::new(),
            terminal_subscriptions: HashMap::new(),
            automation_tokens: HashMap::new(),
            automation_socket,
            _automation_server: automation_server,
            _automation_task: automation_task,
            agent_presence: HashMap::new(),
            explicit_agent_states: HashMap::new(),
            focus_handle,
            left_sidebar_visible: settings.left_sidebar_visible,
            left_sidebar_mode: LeftSidebarMode::Sessions,
            expanded_directories: HashSet::new(),
            project_files: Vec::new(),
            selected_file_path: None,
            show_hidden_files: settings.show_hidden_files,
            file_error: None,
            file_prompt: None,
            pending_file_trash: None,
            palette_mode: None,
            palette_query: String::new(),
            palette_selected: 0,
            palette_files: Vec::new(),
            right_sidebar_visible: settings.git_panel_visible,
            right_sidebar_mode: RightSidebarMode::Diff,
            initial_terminal_focus_pending: true,
            pane_resize_dirty: false,
            persistence_error,
        };
        view.reconcile_terminal_views(cx);
        view.sync_diff_root(cx);
        view.refresh_project_files();
        view
    }

    fn sync_diff_root(&self, cx: &mut Context<Self>) {
        let root = self
            .snapshot
            .selected_session()
            .and_then(|session| self.terminals.get(&session.id))
            .map(|terminal| terminal.read(cx).current_working_directory())
            .or_else(|| {
                self.snapshot
                    .selected_project()
                    .map(|project| PathBuf::from(&project.root_path))
            })
            .unwrap_or_else(|| self.launch_directory.clone());
        self.diff_view
            .update(cx, |diff_view, cx| diff_view.set_root(root, cx));
    }

    fn project_root(&self) -> PathBuf {
        self.snapshot
            .selected_project()
            .map(|project| PathBuf::from(&project.root_path))
            .unwrap_or_else(|| self.launch_directory.clone())
    }

    fn refresh_project_files(&mut self) {
        let root = self.project_root();
        self.expanded_directories.insert(root.clone());
        let mut rows = Vec::new();
        let result = collect_project_files(
            self.file_port.as_ref(),
            &root,
            &root,
            0,
            &self.expanded_directories,
            self.show_hidden_files,
            &mut rows,
        );
        match result {
            Ok(()) => {
                self.project_files = rows;
                self.file_error = None;
                if self
                    .selected_file_path
                    .as_ref()
                    .is_some_and(|path| !path.exists())
                {
                    self.selected_file_path = None;
                }
            }
            Err(error) => {
                self.project_files = rows;
                self.file_error = Some(error.to_string().into());
            }
        }
    }

    fn toggle_directory(&mut self, path: &Path, cx: &mut Context<Self>) {
        if !self.expanded_directories.remove(path) {
            self.expanded_directories.insert(path.to_path_buf());
        }
        self.selected_file_path = Some(path.to_path_buf());
        self.refresh_project_files();
        cx.notify();
    }

    fn select_file_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.selected_file_path = Some(path);
        cx.notify();
    }

    fn open_selected_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.selected_file_path.clone() else {
            self.file_error = Some("Selecciona un archivo".into());
            cx.notify();
            return;
        };
        self.open_file(path, window, cx);
    }

    fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.project_root();
        let document = match self.file_port.read_text_file(&root, &path) {
            Ok(document) => document,
            Err(error) => {
                self.file_error = Some(error.to_string().into());
                self.right_sidebar_visible = true;
                self.right_sidebar_mode = RightSidebarMode::Files;
                cx.notify();
                return;
            }
        };
        let file_port = self.file_port.clone();
        let editor = cx.new(|cx| EditorView::new(root, document, file_port, cx));
        let subscription =
            cx.subscribe(
                &editor,
                |this, _editor, event: &EditorViewEvent, cx| match event {
                    EditorViewEvent::Close => {
                        this.editor = None;
                        this.editor_subscription = None;
                        cx.notify();
                    }
                    EditorViewEvent::Saved => {
                        this.refresh_project_files();
                        this.diff_view
                            .update(cx, |diff_view, cx| diff_view.refresh_now(cx));
                        cx.notify();
                    }
                },
            );
        self.selected_file_path = Some(path);
        self.file_error = None;
        self.editor = Some(editor.clone());
        self.editor_subscription = Some(subscription);
        editor.read(cx).focus_handle(cx).focus(window);
        cx.notify();
    }

    fn begin_file_prompt(&mut self, kind: FilePromptKind, cx: &mut Context<Self>) {
        let root = self.project_root();
        let selected = self.selected_file_path.clone();
        let (directory, target, value) = match kind {
            FilePromptKind::NewFile | FilePromptKind::NewDirectory => {
                let directory = selected
                    .as_ref()
                    .filter(|path| path.is_dir())
                    .cloned()
                    .or_else(|| selected.as_ref()?.parent().map(Path::to_path_buf))
                    .unwrap_or(root);
                (directory, None, String::new())
            }
            FilePromptKind::Rename => {
                let Some(target) = selected else {
                    self.file_error = Some("Selecciona un archivo o carpeta para renombrar".into());
                    cx.notify();
                    return;
                };
                let Some(directory) = target.parent().map(Path::to_path_buf) else {
                    return;
                };
                let value = target
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                (directory, Some(target), value)
            }
        };
        self.file_prompt = Some(FilePrompt {
            kind,
            directory,
            target,
            value,
        });
        self.file_error = None;
        cx.notify();
    }

    fn confirm_file_prompt(&mut self, cx: &mut Context<Self>) {
        let Some(prompt) = self.file_prompt.clone() else {
            return;
        };
        let root = self.project_root();
        let result = match prompt.kind {
            FilePromptKind::NewFile => {
                self.file_port
                    .create_file(&root, &prompt.directory, &prompt.value)
            }
            FilePromptKind::NewDirectory => {
                self.file_port
                    .create_directory(&root, &prompt.directory, &prompt.value)
            }
            FilePromptKind::Rename => self.file_port.rename(
                &root,
                prompt.target.as_deref().expect("rename has a target"),
                &prompt.value,
            ),
        };
        match result {
            Ok(path) => {
                self.file_prompt = None;
                self.file_error = None;
                self.selected_file_path = Some(path.clone());
                if path.is_dir() {
                    self.expanded_directories.insert(path);
                }
                self.expanded_directories.insert(prompt.directory);
                self.refresh_project_files();
            }
            Err(error) => self.file_error = Some(error.to_string().into()),
        }
        cx.notify();
    }

    fn request_file_trash(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.selected_file_path.clone() else {
            self.file_error = Some("Selecciona un archivo o carpeta".into());
            cx.notify();
            return;
        };
        self.pending_file_trash = Some(path);
        self.file_error = None;
        cx.notify();
    }

    fn confirm_file_trash(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.pending_file_trash.take() else {
            return;
        };
        let root = self.project_root();
        match self.file_port.move_to_trash(&root, &path) {
            Ok(()) => {
                self.selected_file_path = None;
                self.expanded_directories.remove(&path);
                self.file_error = None;
                self.refresh_project_files();
            }
            Err(error) => self.file_error = Some(error.to_string().into()),
        }
        cx.notify();
    }

    fn open_palette(&mut self, mode: PaletteMode, cx: &mut Context<Self>) {
        self.palette_mode = Some(mode);
        self.palette_query.clear();
        self.palette_selected = 0;
        self.file_prompt = None;
        self.pending_file_trash = None;
        self.palette_files.clear();
        if mode == PaletteMode::Files {
            let root = self.project_root();
            let _ = collect_search_files(
                self.file_port.as_ref(),
                &root,
                &root,
                &mut self.palette_files,
            );
        }
        cx.notify();
    }

    fn toggle_command_palette(
        &mut self,
        _: &ToggleCommandPalette,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.palette_mode.is_some() {
            self.palette_mode = None;
            cx.notify();
        } else {
            self.open_palette(PaletteMode::Commands, cx);
        }
    }

    fn quick_open(&mut self, _: &QuickOpen, _: &mut Window, cx: &mut Context<Self>) {
        self.open_palette(PaletteMode::Files, cx);
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
                    label: "Sidebar: Toggle Files and Diff".into(),
                    detail: "⌘R".into(),
                    action: PaletteAction::ToggleGit,
                },
                PaletteItem {
                    label: "Sidebar: Sessions".into(),
                    detail: String::new(),
                    action: PaletteAction::ShowSessions,
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
                    detail: String::new(),
                    action: PaletteAction::ShowSettings,
                },
            ],
            PaletteMode::Files => {
                let root = self.project_root();
                self.palette_files
                    .iter()
                    .map(|path| PaletteItem {
                        label: path
                            .strip_prefix(&root)
                            .unwrap_or(path)
                            .display()
                            .to_string(),
                        detail: path
                            .extension()
                            .map(|extension| extension.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        action: PaletteAction::OpenFile(path.clone()),
                    })
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
        let query = self.palette_query.to_lowercase();
        if !query.is_empty() {
            let tokens: Vec<_> = query.split_whitespace().collect();
            items.retain(|item| {
                let haystack = item.label.to_lowercase();
                tokens.iter().all(|token| haystack.contains(token))
            });
        }
        items.truncate(100);
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
                if self.snapshot.create_terminal_tab() {
                    self.reconcile_terminal_views(cx);
                    self.persist(cx);
                    self.focus_selected_terminal(window, cx);
                }
            }
            PaletteAction::NewWorkspace => {
                self.snapshot.create_workspace(&self.launch_directory);
                self.reconcile_terminal_views(cx);
                self.sync_diff_root(cx);
                self.refresh_project_files();
                self.persist(cx);
                self.focus_selected_terminal(window, cx);
            }
            PaletteAction::Split(direction) => self.split_pane(direction, window, cx),
            PaletteAction::EqualizePanes => {
                if self.snapshot.equalize_selected_panes() {
                    self.persist(cx);
                }
            }
            PaletteAction::TogglePaneZoom => {
                if self.snapshot.toggle_selected_pane_zoom() {
                    self.persist(cx);
                }
            }
            PaletteAction::ToggleGit => self.toggle_diff_panel(cx),
            PaletteAction::ShowSessions => {
                self.left_sidebar_visible = true;
                self.settings.left_sidebar_visible = true;
                self.left_sidebar_mode = LeftSidebarMode::Sessions;
                self.persist_settings(cx);
            }
            PaletteAction::ShowFiles => {
                self.right_sidebar_visible = true;
                self.settings.git_panel_visible = true;
                self.right_sidebar_mode = RightSidebarMode::Files;
                self.refresh_project_files();
                self.persist_settings(cx);
            }
            PaletteAction::ShowInfo => {
                self.left_sidebar_visible = true;
                self.settings.left_sidebar_visible = true;
                self.left_sidebar_mode = LeftSidebarMode::Info;
                self.persist_settings(cx);
            }
            PaletteAction::ShowSettings => {
                self.left_sidebar_visible = true;
                self.settings.left_sidebar_visible = true;
                self.left_sidebar_mode = LeftSidebarMode::Settings;
                self.persist_settings(cx);
            }
            PaletteAction::SelectWorkspace {
                project_id,
                workspace_id,
            } => self.select_workspace(project_id, workspace_id, window, cx),
            PaletteAction::OpenFile(path) => self.open_file(path, window, cx),
        }
    }

    fn on_workspace_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.to_ascii_lowercase();
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
            return;
        }
        if self.pending_file_trash.is_some() {
            match key.as_str() {
                "enter" | "return" => self.confirm_file_trash(cx),
                "escape" | "esc" => {
                    self.pending_file_trash = None;
                    cx.notify();
                }
                _ => {}
            }
            cx.stop_propagation();
            return;
        }

        let Some(prompt) = self.file_prompt.as_mut() else {
            return;
        };
        match key.as_str() {
            "enter" | "return" => self.confirm_file_prompt(cx),
            "escape" | "esc" => {
                self.file_prompt = None;
                self.file_error = None;
                cx.notify();
            }
            "backspace" => {
                prompt.value.pop();
                cx.notify();
            }
            _ if !event.keystroke.modifiers.platform
                && !event.keystroke.modifiers.control
                && !event.keystroke.modifiers.alt =>
            {
                if let Some(text) = event.keystroke.key_char.as_ref() {
                    prompt.value.push_str(text);
                    cx.notify();
                }
            }
            _ => {}
        }
        cx.stop_propagation();
    }

    fn toggle_diff_panel(&mut self, cx: &mut Context<Self>) {
        self.right_sidebar_visible = !self.right_sidebar_visible;
        self.settings.git_panel_visible = self.right_sidebar_visible;
        self.persist_settings(cx);
        if self.right_sidebar_visible {
            self.sync_diff_root(cx);
            self.diff_view
                .update(cx, |diff_view, cx| diff_view.refresh_now(cx));
        }
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
            self.explicit_agent_states.remove(&session_id);
        }

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
    }

    fn handle_terminal_view_event(&mut self, event: &TerminalViewEvent, cx: &mut Context<Self>) {
        match event {
            TerminalViewEvent::TitleChanged { session_id, title } => {
                if self.snapshot.update_session_title(*session_id, title) {
                    self.persist(cx);
                }
            }
            TerminalViewEvent::WorkingDirectoryChanged { session_id, path } => {
                let changed = self
                    .snapshot
                    .update_session_working_directory(*session_id, path);
                if self
                    .snapshot
                    .selected_session()
                    .is_some_and(|session| session.id == *session_id)
                {
                    self.diff_view.update(cx, |diff_view, cx| {
                        diff_view.set_root(path.clone(), cx);
                    });
                }
                if changed {
                    self.persist(cx);
                }
            }
            TerminalViewEvent::Exited { session_id, code } => {
                let _exited_session = (*session_id, *code);
                cx.notify();
            }
            TerminalViewEvent::AgentPresenceChanged {
                session_id,
                presence,
            } => {
                if let Some(presence) = presence {
                    self.agent_presence.insert(*session_id, presence.clone());
                } else {
                    self.agent_presence.remove(session_id);
                }
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
        let result = match request.envelope.command {
            AutomationCommand::List => Ok(self.automation_pane_list(pane_id)),
            AutomationCommand::Send { text, newline } => {
                let Some(terminal) = self.terminals.get(&pane_id) else {
                    let _ = request
                        .response
                        .send(AutomationResponse::failure("el pane ya no existe"));
                    return;
                };
                terminal.read(cx).send_automation_input(&text, newline);
                Ok(serde_json::json!({ "paneId": pane_id, "sent": text.len() }))
            }
            AutomationCommand::Split { direction } => {
                if !self.snapshot.select_terminal_global(pane_id) {
                    Err("el pane ya no existe".to_owned())
                } else {
                    let direction = automation_split_direction(direction);
                    match self.snapshot.split_selected_terminal(direction) {
                        Some(created) => {
                            self.reconcile_terminal_views(cx);
                            self.persist(cx);
                            Ok(serde_json::json!({ "paneId": created }))
                        }
                        None => Err("no se pudo dividir el pane".to_owned()),
                    }
                }
            }
            AutomationCommand::Focus { direction } => {
                if !self.snapshot.select_terminal_global(pane_id) {
                    Err("el pane ya no existe".to_owned())
                } else if self
                    .snapshot
                    .focus_terminal(automation_focus_direction(direction))
                {
                    let selected = self.snapshot.selected_session().map(|session| session.id);
                    self.sync_diff_root(cx);
                    self.persist(cx);
                    Ok(serde_json::json!({ "paneId": selected }))
                } else {
                    Err("no hay un pane en esa dirección".to_owned())
                }
            }
            AutomationCommand::Close => {
                if self.snapshot.select_terminal_global(pane_id)
                    && self.snapshot.close_selected_terminal()
                {
                    self.reconcile_terminal_views(cx);
                    self.sync_diff_root(cx);
                    self.persist(cx);
                    Ok(serde_json::json!({ "closed": pane_id }))
                } else {
                    Err("el pane ya no existe".to_owned())
                }
            }
            AutomationCommand::Zoom => {
                if self.snapshot.select_terminal_global(pane_id)
                    && self.snapshot.toggle_selected_pane_zoom()
                {
                    self.persist(cx);
                    Ok(serde_json::json!({ "paneId": pane_id, "zoomToggled": true }))
                } else {
                    Err("el pane ya no existe".to_owned())
                }
            }
            AutomationCommand::AgentStatus => Ok(self.automation_agent_status(pane_id)),
            AutomationCommand::SetAgentState { state } => {
                self.explicit_agent_states.insert(pane_id, state);
                cx.notify();
                Ok(self.automation_agent_status(pane_id))
            }
        };
        let response = match result {
            Ok(data) => AutomationResponse::success(data),
            Err(error) => AutomationResponse::failure(error),
        };
        let _ = request.response.send(response);
    }

    fn automation_pane_list(&self, caller: Uuid) -> serde_json::Value {
        let project = self.snapshot.projects.iter().find(|project| {
            project
                .workspaces
                .as_deref()
                .unwrap_or_default()
                .iter()
                .flat_map(|workspace| &workspace.tabs)
                .flat_map(|tab| &tab.sessions)
                .any(|session| session.id == caller)
        });
        let panes: Vec<_> = project
            .into_iter()
            .flat_map(|project| project.workspaces.as_deref().unwrap_or_default())
            .flat_map(|workspace| &workspace.tabs)
            .flat_map(|tab| &tab.sessions)
            .map(|session| {
                let agent = self.agent_status_value(session.id);
                serde_json::json!({
                    "paneId": session.id,
                    "title": session.title,
                    "workingDirectory": session.working_directory,
                    "selected": self.snapshot.selected_session().is_some_and(|selected| selected.id == session.id),
                    "agent": agent,
                })
            })
            .collect();
        serde_json::json!({ "panes": panes })
    }

    fn automation_agent_status(&self, pane_id: Uuid) -> serde_json::Value {
        serde_json::json!({
            "paneId": pane_id,
            "agent": self.agent_status_value(pane_id),
        })
    }

    fn agent_status_value(&self, pane_id: Uuid) -> serde_json::Value {
        let detected = self.agent_presence.get(&pane_id);
        let state = self
            .explicit_agent_states
            .get(&pane_id)
            .copied()
            .map(agent_runtime_state_label)
            .or_else(|| detected.map(|presence| terminal_agent_state_label(presence.state)));
        serde_json::json!({
            "kind": detected.map(|presence| presence.kind.as_str()),
            "state": state,
            "source": if self.explicit_agent_states.contains_key(&pane_id) { "hook" } else { "screen" },
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
            self.sync_diff_root(cx);
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
            self.sync_diff_root(cx);
            self.persist(cx);
            self.focus_selected_terminal(window, cx);
        }
    }

    fn cycle_pane(&mut self, offset: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.snapshot.cycle_terminal(offset) {
            self.sync_diff_root(cx);
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
    }

    fn persist(&mut self, cx: &mut Context<Self>) {
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

    fn set_terminal_font_size(&mut self, size: f32, cx: &mut Context<Self>) {
        let size = size.clamp(8.0, 32.0);
        if self.settings.terminal_font_size == size {
            return;
        }
        self.settings.terminal_font_size = size;
        for terminal in self.terminals.values() {
            terminal.update(cx, |terminal, cx| terminal.apply_font_size(size, cx));
        }
        self.persist_settings(cx);
    }

    fn new_workspace(&mut self, _: &NewWorkspace, window: &mut Window, cx: &mut Context<Self>) {
        self.snapshot.create_workspace(&self.launch_directory);
        self.reconcile_terminal_views(cx);
        self.sync_diff_root(cx);
        self.refresh_project_files();
        self.persist(cx);
        self.focus_selected_terminal(window, cx);
    }

    fn new_terminal_tab(
        &mut self,
        _: &NewTerminalTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.snapshot.create_terminal_tab() {
            self.reconcile_terminal_views(cx);
            self.sync_diff_root(cx);
            self.refresh_project_files();
            self.persist(cx);
            self.focus_selected_terminal(window, cx);
        }
    }

    fn close_terminal(&mut self, _: &CloseTerminal, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = &self.editor {
            editor.update(cx, |editor, cx| editor.request_close(cx));
            return;
        }
        if self.snapshot.close_selected_terminal() {
            self.reconcile_terminal_views(cx);
            self.sync_diff_root(cx);
            self.refresh_project_files();
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
            self.persist(cx);
            self.focus_selected_terminal(window, cx);
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
            self.sync_diff_root(cx);
            self.refresh_project_files();
            self.persist(cx);
            self.focus_selected_terminal(window, cx);
        }
    }

    fn next_workspace(&mut self, _: &NextWorkspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.snapshot.cycle_workspace(1) {
            self.sync_diff_root(cx);
            self.refresh_project_files();
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
            self.sync_diff_root(cx);
            self.persist(cx);
            self.focus_selected_terminal(window, cx);
        }
    }

    fn select_tab(&mut self, tab_id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        if self.snapshot.select_tab(tab_id) {
            self.sync_diff_root(cx);
            self.persist(cx);
            self.focus_selected_terminal(window, cx);
        }
    }

    fn close_tab(&mut self, tab_id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        if self.snapshot.close_tab(tab_id) {
            self.reconcile_terminal_views(cx);
            self.sync_diff_root(cx);
            self.persist(cx);
            self.focus_selected_terminal(window, cx);
        }
    }

    fn titlebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let left_chrome_width = if self.left_sidebar_visible {
            260.0
        } else {
            156.0
        };
        let workspace_name = self
            .snapshot
            .selected_workspace()
            .map(|workspace| workspace.name.clone())
            .unwrap_or_else(|| "Sin espacio".to_owned());
        let project_name = self
            .snapshot
            .selected_project()
            .map(|project| project.name.clone())
            .unwrap_or_else(|| "Proyecto".to_owned());
        let show_project_name = project_name != workspace_name;

        div()
            .h(px(42.0))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .bg(DARK.titlebar)
            .border_b_1()
            .border_color(DARK.border_subtle)
            .child(
                div()
                    .w(px(left_chrome_width))
                    .h_full()
                    .flex_none()
                    .flex()
                    .items_center()
                    .pl(px(86.0))
                    .gap_1()
                    .bg(if self.left_sidebar_visible {
                        DARK.sidebar
                    } else {
                        DARK.titlebar
                    })
                    .when(self.left_sidebar_visible, |chrome| {
                        chrome.border_r_1().border_color(DARK.border_subtle)
                    })
                    .child(
                        self.sidebar_button("toggle-left-sidebar", true, cx, |this, _, cx| {
                            this.left_sidebar_visible = !this.left_sidebar_visible;
                            this.settings.left_sidebar_visible = this.left_sidebar_visible;
                            this.persist_settings(cx);
                        }),
                    )
                    .child(self.chrome_button(
                        "+",
                        "new-workspace-toolbar",
                        false,
                        cx,
                        |this, window, cx| {
                            this.snapshot.create_workspace(&this.launch_directory);
                            this.reconcile_terminal_views(cx);
                            this.sync_diff_root(cx);
                            this.persist(cx);
                            this.focus_selected_terminal(window, cx);
                        },
                    ))
                    .child(
                        div()
                            .h_full()
                            .flex_1()
                            .window_control_area(WindowControlArea::Drag),
                    ),
            )
            .child(
                div()
                    .h_full()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .items_center()
                    .px_4()
                    .gap_2()
                    .window_control_area(WindowControlArea::Drag)
                    .child(
                        div()
                            .min_w(px(0.0))
                            .flex_1()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.0))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(DARK.foreground)
                                    .child(workspace_name.clone()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(10.0))
                                    .text_color(DARK.subtle)
                                    .child(if show_project_name {
                                        format!("/ {project_name}")
                                    } else {
                                        "Local".to_owned()
                                    }),
                            ),
                    ),
            )
            .child(
                div()
                    .h_full()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_end()
                    .pr_2()
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
            LeftSidebarMode::Settings => self.settings_sidebar_content(cx),
        };
        let modes = [
            (LeftSidebarMode::Sessions, "Sesiones"),
            (LeftSidebarMode::Info, "Info"),
            (LeftSidebarMode::Settings, "Prefs"),
        ];
        div()
            .w(px(260.0))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(DARK.sidebar)
            .border_r_1()
            .border_color(DARK.border_subtle)
            .child(
                div()
                    .h(px(34.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .border_b_1()
                    .border_color(DARK.border_subtle)
                    .children(modes.into_iter().map(|(mode, label)| {
                        let selected = mode == self.left_sidebar_mode;
                        div()
                            .id(SharedString::from(format!("sidebar-mode-{label}")))
                            .flex_1()
                            .h(px(24.0))
                            .rounded(px(5.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .text_size(px(9.5))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(if selected {
                                DARK.foreground
                            } else {
                                DARK.subtle
                            })
                            .bg(if selected {
                                DARK.selection
                            } else {
                                DARK.sidebar
                            })
                            .hover(|tab| tab.bg(DARK.hover).text_color(DARK.foreground))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.left_sidebar_mode = mode;
                                cx.notify();
                            }))
                            .child(label)
                    })),
            )
            .child(content)
    }

    fn workspace_agent_summary(&self, workspace_id: Uuid) -> Option<(String, AgentRuntimeState)> {
        let workspace = self
            .snapshot
            .projects
            .iter()
            .flat_map(|project| project.workspaces.as_deref().unwrap_or_default())
            .find(|workspace| workspace.id == workspace_id)?;
        workspace
            .tabs
            .iter()
            .flat_map(|tab| &tab.sessions)
            .filter_map(|session| {
                let detected = self.agent_presence.get(&session.id);
                let explicit = self.explicit_agent_states.get(&session.id).copied();
                let state = explicit.or_else(|| {
                    detected.map(|presence| match presence.state {
                        TerminalAgentState::Idle => AgentRuntimeState::Idle,
                        TerminalAgentState::Working => AgentRuntimeState::Working,
                        TerminalAgentState::Waiting => AgentRuntimeState::Waiting,
                    })
                })?;
                let kind = detected
                    .map(|presence| presence.kind.clone())
                    .unwrap_or_else(|| "Agent".into());
                Some((kind, state))
            })
            .max_by_key(|(_, state)| match state {
                AgentRuntimeState::Idle => 0,
                AgentRuntimeState::Working => 1,
                AgentRuntimeState::Waiting => 2,
            })
    }

    fn sessions_sidebar_content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let entries = self.snapshot.workspace_entries();
        let agent_summaries: HashMap<_, _> = entries
            .iter()
            .filter_map(|entry| {
                self.workspace_agent_summary(entry.workspace_id)
                    .map(|summary| (entry.workspace_id, summary))
            })
            .collect();
        let mut panel = div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(DARK.sidebar);

        if entries.is_empty() {
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
                            .bg(DARK.elevated)
                            .font_family("JetBrains Mono")
                            .text_size(px(11.0))
                            .text_color(DARK.muted)
                            .child(">_"),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(DARK.muted)
                            .child("Sin sesiones"),
                    )
                    .child(
                        div()
                            .text_center()
                            .text_size(px(10.5))
                            .text_color(DARK.subtle)
                            .child("Usa + para crear una"),
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
                    .children(entries.into_iter().map(|entry| {
                        let project_id = entry.project_id;
                        let workspace_id = entry.workspace_id;
                        let selected = entry.is_selected;
                        let agent = agent_summaries.get(&workspace_id).cloned();
                        div()
                            .id(SharedString::from(format!("workspace-{workspace_id}")))
                            .h(px(64.0))
                            .w_full()
                            .mb(px(1.0))
                            .px_3()
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded(px(8.0))
                            .cursor_pointer()
                            .bg(if selected {
                                DARK.selection
                            } else {
                                DARK.sidebar
                            })
                            .hover(|item| item.bg(DARK.hover))
                            .active(|item| item.opacity(0.82))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_workspace(project_id, workspace_id, window, cx);
                            }))
                            .child(
                                div()
                                    .size(px(32.0))
                                    .flex_none()
                                    .relative()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(8.0))
                                    .bg(DARK.elevated)
                                    .font_family("JetBrains Mono")
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_size(px(9.5))
                                    .text_color(DARK.foreground)
                                    .child(">_")
                                    .when(selected, |icon| {
                                        icon.child(
                                            div()
                                                .absolute()
                                                .right_0()
                                                .bottom_0()
                                                .size(px(6.0))
                                                .rounded_full()
                                                .bg(DARK.success),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .min_w(px(0.0))
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .gap(px(3.0))
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(px(11.5))
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .text_color(if selected {
                                                DARK.foreground
                                            } else {
                                                DARK.muted
                                            })
                                            .child(entry.workspace_name),
                                    )
                                    .child(
                                        div()
                                            .min_w(px(0.0))
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .text_size(px(9.5))
                                            .child(
                                                div()
                                                    .font_weight(gpui::FontWeight::MEDIUM)
                                                    .text_color(
                                                        match agent.as_ref().map(|item| item.1) {
                                                            Some(AgentRuntimeState::Waiting) => {
                                                                DARK.warning
                                                            }
                                                            Some(AgentRuntimeState::Working) => {
                                                                DARK.success
                                                            }
                                                            Some(AgentRuntimeState::Idle) => {
                                                                DARK.accent
                                                            }
                                                            None if selected => DARK.success,
                                                            None => DARK.subtle,
                                                        },
                                                    )
                                                    .child(agent.as_ref().map_or_else(
                                                        || {
                                                            if selected {
                                                                "Activa".to_owned()
                                                            } else {
                                                                "En espera".to_owned()
                                                            }
                                                        },
                                                        |(kind, state)| {
                                                            format!(
                                                                "{kind} {}",
                                                                agent_runtime_state_label(*state)
                                                            )
                                                        },
                                                    )),
                                            )
                                            .child(div().text_color(DARK.subtle).child("·"))
                                            .child(
                                                div()
                                                    .min_w(px(0.0))
                                                    .truncate()
                                                    .text_color(DARK.subtle)
                                                    .child(entry.project_name),
                                            ),
                                    ),
                            )
                    })),
            );
        }

        panel.into_any_element()
    }

    fn files_sidebar_content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let rows = self.project_files.clone();
        let selected_path = self.selected_file_path.clone();
        let file_error = self.file_error.clone();
        let show_hidden = self.show_hidden_files;
        div()
            .id("project-files-content")
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .h(px(34.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .px_3()
                    .gap_2()
                    .border_b_1()
                    .border_color(DARK.border_subtle)
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .text_size(px(10.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(DARK.muted)
                            .child(
                                self.snapshot
                                    .selected_project()
                                    .map(|project| project.name.clone())
                                    .unwrap_or_else(|| "Proyecto".into()),
                            ),
                    )
                    .child(
                        div()
                            .id("toggle-hidden-files")
                            .px_2()
                            .py_1()
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .text_size(px(9.0))
                            .text_color(if show_hidden {
                                DARK.foreground
                            } else {
                                DARK.subtle
                            })
                            .bg(if show_hidden {
                                DARK.selection
                            } else {
                                DARK.sidebar
                            })
                            .hover(|button| button.bg(DARK.hover))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_hidden_files = !this.show_hidden_files;
                                this.settings.show_hidden_files = this.show_hidden_files;
                                this.refresh_project_files();
                                this.persist_settings(cx);
                            }))
                            .child(".files"),
                    ),
            )
            .child(
                div()
                    .id("project-file-tree")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .py_1()
                    .children(rows.into_iter().map(|row| {
                        let path = row.entry.path.clone();
                        let selected = selected_path.as_ref() == Some(&path);
                        let is_directory = row.entry.kind == FileEntryKind::Directory;
                        let icon = match row.entry.kind {
                            FileEntryKind::Directory if row.expanded => "▾",
                            FileEntryKind::Directory => "▸",
                            FileEntryKind::File => "·",
                            FileEntryKind::Symlink => "↗",
                        };
                        div()
                            .id(SharedString::from(format!(
                                "file-row-{}",
                                path.to_string_lossy()
                            )))
                            .h(px(24.0))
                            .w_full()
                            .flex()
                            .items_center()
                            .gap_1()
                            .pr_2()
                            .pl(px(8.0 + row.depth as f32 * 13.0))
                            .cursor_pointer()
                            .bg(if selected {
                                DARK.selection
                            } else {
                                DARK.sidebar
                            })
                            .text_color(if selected {
                                DARK.foreground
                            } else {
                                DARK.muted
                            })
                            .hover(|item| item.bg(DARK.hover).text_color(DARK.foreground))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if is_directory {
                                    this.toggle_directory(&path, cx);
                                } else {
                                    this.select_file_path(path.clone(), cx);
                                }
                            }))
                            .child(
                                div()
                                    .w(px(12.0))
                                    .flex_none()
                                    .text_center()
                                    .text_size(px(10.0))
                                    .text_color(if is_directory {
                                        DARK.warning
                                    } else {
                                        DARK.subtle
                                    })
                                    .child(icon),
                            )
                            .child(
                                div()
                                    .min_w(px(0.0))
                                    .flex_1()
                                    .truncate()
                                    .text_size(px(10.5))
                                    .child(row.entry.name),
                            )
                    })),
            )
            .when_some(file_error, |panel, error| {
                panel.child(
                    div()
                        .mx_2()
                        .mb_1()
                        .p_2()
                        .rounded(px(5.0))
                        .bg(DARK.elevated)
                        .text_size(px(9.0))
                        .text_color(DARK.danger)
                        .child(error),
                )
            })
            .child(
                div()
                    .h(px(36.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .border_t_1()
                    .border_color(DARK.border_subtle)
                    .child(
                        self.file_action_button("Open", "open-file", cx, |this, window, cx| {
                            this.open_selected_file(window, cx);
                        }),
                    )
                    .child(
                        self.file_action_button("+ File", "new-file", cx, |this, _, cx| {
                            this.begin_file_prompt(FilePromptKind::NewFile, cx);
                        }),
                    )
                    .child(
                        self.file_action_button("+ Dir", "new-directory", cx, |this, _, cx| {
                            this.begin_file_prompt(FilePromptKind::NewDirectory, cx);
                        }),
                    )
                    .child(
                        self.file_action_button("Rename", "rename-file", cx, |this, _, cx| {
                            this.begin_file_prompt(FilePromptKind::Rename, cx);
                        }),
                    )
                    .child(
                        self.file_action_button("Trash", "trash-file", cx, |this, _, cx| {
                            this.request_file_trash(cx);
                        }),
                    )
                    .child(
                        self.file_action_button("↻", "refresh-files", cx, |this, _, cx| {
                            this.refresh_project_files();
                            cx.notify();
                        }),
                    ),
            )
            .into_any_element()
    }

    fn info_sidebar_content(&self, _: &mut Context<Self>) -> AnyElement {
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
                    .text_size(px(10.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(DARK.muted)
                    .child("PROJECT ROOT"),
            )
            .child(
                div()
                    .p_2()
                    .rounded(px(6.0))
                    .bg(DARK.elevated)
                    .font_family("JetBrains Mono")
                    .text_size(px(9.0))
                    .text_color(DARK.foreground)
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
                            .text_color(DARK.subtle)
                            .child(label),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(DARK.foreground)
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
                            .text_color(DARK.muted)
                            .child("SELECCIÓN"),
                    )
                    .child(
                        div()
                            .p_2()
                            .rounded(px(6.0))
                            .bg(DARK.elevated)
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(DARK.foreground)
                                    .child(name),
                            )
                            .child(
                                div()
                                    .font_family("JetBrains Mono")
                                    .text_size(px(8.5))
                                    .text_color(DARK.subtle)
                                    .child(path),
                            ),
                    )
            })
            .into_any_element()
    }

    fn settings_sidebar_content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let font_size = self.settings.terminal_font_size;
        let hidden = self.settings.show_hidden_files;
        let left_sidebar = self.settings.left_sidebar_visible;
        let git_panel = self.settings.git_panel_visible;
        div()
            .id("settings-sidebar")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .p_3()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_size(px(10.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(DARK.muted)
                    .child("APARIENCIA"),
            )
            .child(
                div()
                    .p_3()
                    .rounded(px(7.0))
                    .bg(DARK.elevated)
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
                                    .text_color(DARK.foreground)
                                    .child("Fuente de terminal"),
                            )
                            .child(
                                div()
                                    .font_family("JetBrains Mono")
                                    .text_size(px(9.0))
                                    .text_color(DARK.muted)
                                    .child(format!("{font_size:.0} px")),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(self.settings_button("−", "settings-font-down", cx, |this, cx| {
                                this.set_terminal_font_size(
                                    this.settings.terminal_font_size - 1.0,
                                    cx,
                                );
                            }))
                            .child(self.settings_button("Reset", "settings-font-reset", cx, |this, cx| {
                                this.set_terminal_font_size(12.0, cx);
                            }))
                            .child(self.settings_button("+", "settings-font-up", cx, |this, cx| {
                                this.set_terminal_font_size(
                                    this.settings.terminal_font_size + 1.0,
                                    cx,
                                );
                            })),
                    ),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(DARK.muted)
                    .child("WORKSPACE"),
            )
            .child(
                div()
                    .rounded(px(7.0))
                    .overflow_hidden()
                    .bg(DARK.elevated)
                    .child(self.settings_toggle_row(
                        "Archivos ocultos",
                        hidden,
                        "settings-hidden",
                        cx,
                        |this, cx| {
                            this.show_hidden_files = !this.show_hidden_files;
                            this.settings.show_hidden_files = this.show_hidden_files;
                            this.refresh_project_files();
                            this.persist_settings(cx);
                        },
                    ))
                    .child(self.settings_toggle_row(
                        "Sidebar al iniciar",
                        left_sidebar,
                        "settings-sidebar-visible",
                        cx,
                        |this, cx| {
                            this.left_sidebar_visible = !this.left_sidebar_visible;
                            this.settings.left_sidebar_visible = this.left_sidebar_visible;
                            this.persist_settings(cx);
                        },
                    ))
                    .child(self.settings_toggle_row(
                        "Panel Files y Diff al iniciar",
                        git_panel,
                        "settings-git-visible",
                        cx,
                        |this, cx| {
                            this.right_sidebar_visible = !this.right_sidebar_visible;
                            this.settings.git_panel_visible = this.right_sidebar_visible;
                            if this.right_sidebar_visible {
                                this.sync_diff_root(cx);
                            }
                            this.persist_settings(cx);
                        },
                    )),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(DARK.muted)
                    .child("SEGURIDAD"),
            )
            .child(
                div()
                    .p_3()
                    .rounded(px(7.0))
                    .bg(DARK.elevated)
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
                                    .text_size(px(10.0))
                                    .text_color(DARK.foreground)
                                    .child("OSC 52 clipboard read"),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded(px(4.0))
                                    .bg(DARK.diff_added_bg)
                                    .text_size(px(8.0))
                                    .text_color(DARK.success)
                                    .child("Confirmación obligatoria"),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(9.0))
                            .line_height(px(13.0))
                            .text_color(DARK.subtle)
                            .child("La automatización usa un socket 0600 y un token distinto por pane."),
                    ),
            )
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
            .bg(DARK.selection)
            .text_size(px(9.0))
            .text_color(DARK.muted)
            .hover(|button| button.bg(DARK.hover).text_color(DARK.foreground))
            .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
            .child(label)
    }

    fn settings_toggle_row(
        &self,
        label: &'static str,
        enabled: bool,
        id: &'static str,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .h(px(38.0))
            .px_3()
            .flex()
            .items_center()
            .cursor_pointer()
            .border_b_1()
            .border_color(DARK.border_subtle)
            .hover(|row| row.bg(DARK.hover))
            .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
            .child(
                div()
                    .flex_1()
                    .text_size(px(10.0))
                    .text_color(DARK.muted)
                    .child(label),
            )
            .child(
                div()
                    .w(px(30.0))
                    .h(px(16.0))
                    .p(px(2.0))
                    .rounded_full()
                    .flex()
                    .justify_end()
                    .bg(if enabled {
                        DARK.success
                    } else {
                        DARK.selection
                    })
                    .when(!enabled, |toggle| toggle.justify_start())
                    .child(div().size(px(12.0)).rounded_full().bg(DARK.foreground)),
            )
    }

    fn file_action_button(
        &self,
        label: &'static str,
        id: &'static str,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .h(px(22.0))
            .px_2()
            .rounded(px(4.0))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .bg(DARK.elevated)
            .text_size(px(8.5))
            .text_color(DARK.muted)
            .hover(|button| button.bg(DARK.hover).text_color(DARK.foreground))
            .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
            .child(label)
    }

    fn center_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let editor = self.editor.clone();
        let tabs = self
            .snapshot
            .selected_workspace()
            .map(|workspace| workspace.tabs.clone())
            .unwrap_or_default();
        let selected_tab_id = self
            .snapshot
            .selected_workspace()
            .and_then(|workspace| workspace.selected_tab_id);

        let panel = div()
            .flex_1()
            .min_w(px(360.0))
            .h_full()
            .flex()
            .flex_col()
            .min_h(px(0.0))
            .overflow_hidden()
            .bg(DARK.terminal);
        if let Some(editor) = editor {
            panel.child(editor)
        } else {
            panel
                .child(self.tab_bar(tabs, selected_tab_id, cx))
                .child(self.terminal_canvas(cx))
        }
    }

    fn tab_bar(
        &mut self,
        tabs: Vec<TabSnapshot>,
        selected_tab_id: Option<Uuid>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let tab_list = div()
            .h_full()
            .flex_1()
            .min_w(px(0.0))
            .flex()
            .items_center()
            .gap(px(2.0))
            .px_2()
            .overflow_x_hidden()
            .children(tabs.into_iter().enumerate().map(|(index, tab)| {
                let tab_id = tab.id;
                let selected = Some(tab_id) == selected_tab_id;
                let title = tab
                    .sessions
                    .iter()
                    .find(|session| Some(session.id) == tab.selected_session_id)
                    .map(|session| session.title.clone())
                    .unwrap_or_else(|| format!("Terminal {}", index + 1));
                div()
                    .id(SharedString::from(format!("tab-{tab_id}")))
                    .h(px(28.0))
                    .min_w(px(112.0))
                    .max_w(px(192.0))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .rounded(px(5.0))
                    .cursor_pointer()
                    .bg(if selected {
                        DARK.elevated
                    } else {
                        DARK.terminal
                    })
                    .text_color(if selected {
                        DARK.foreground
                    } else {
                        DARK.muted
                    })
                    .hover(|tab| tab.bg(DARK.hover))
                    .active(|tab| tab.opacity(0.84))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_tab(tab_id, window, cx);
                    }))
                    .child(
                        div()
                            .min_w(px(0.0))
                            .flex_1()
                            .truncate()
                            .text_size(px(10.5))
                            .font_weight(if selected {
                                gpui::FontWeight::MEDIUM
                            } else {
                                gpui::FontWeight::NORMAL
                            })
                            .child(title),
                    )
                    .when(selected, |tab| {
                        tab.child(
                            div()
                                .id(SharedString::from(format!("close-tab-{tab_id}")))
                                .size(px(16.0))
                                .flex_none()
                                .rounded(px(4.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(13.0))
                                .text_color(DARK.subtle)
                                .hover(|close| close.bg(DARK.elevated).text_color(DARK.foreground))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.close_tab(tab_id, window, cx);
                                }))
                                .child("×"),
                        )
                    })
            }));

        div()
            .h(px(36.0))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .bg(DARK.terminal)
            .border_b_1()
            .border_color(DARK.border_subtle)
            .child(tab_list)
            .child(
                div()
                    .h_full()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_1()
                    .child(self.chrome_button(
                        "□",
                        "toggle-pane-zoom",
                        false,
                        cx,
                        |this, window, cx| {
                            if this.snapshot.toggle_selected_pane_zoom() {
                                this.persist(cx);
                                this.focus_selected_terminal(window, cx);
                            }
                        },
                    ))
                    .child(div().w(px(1.0)).h(px(18.0)).mx_1().bg(DARK.border_subtle))
                    .child(self.chrome_button(
                        "+",
                        "new-terminal-tab",
                        false,
                        cx,
                        |this, window, cx| {
                            if this.snapshot.create_terminal_tab() {
                                this.reconcile_terminal_views(cx);
                                this.sync_diff_root(cx);
                                this.persist(cx);
                                this.focus_selected_terminal(window, cx);
                            }
                        },
                    )),
            )
    }

    fn render_pane_layout(
        &mut self,
        layout: &PaneLayoutSnapshot,
        selected_session_id: Option<Uuid>,
        path: Vec<PaneBranch>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match layout {
            PaneLayoutSnapshot::Terminal { id } => {
                let session_id = *id;
                let selected = selected_session_id == Some(session_id);
                let terminal = self.terminals.get(&session_id).cloned();
                div()
                    .id(SharedString::from(format!("pane-{session_id}")))
                    .size_full()
                    .min_w(px(80.0))
                    .min_h(px(48.0))
                    .relative()
                    .overflow_hidden()
                    .border_1()
                    .border_color(if selected {
                        DARK.success
                    } else {
                        DARK.border_subtle
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.select_terminal(session_id, window, cx);
                        }),
                    )
                    .when_some(terminal, |pane, terminal| pane.child(terminal))
                    .when(selected, |pane| {
                        pane.child(
                            div()
                                .absolute()
                                .top_1()
                                .right_1()
                                .size(px(5.0))
                                .rounded_full()
                                .bg(DARK.success),
                        )
                    })
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
                let first = self.render_pane_layout(first, selected_session_id, first_path, cx);
                let second = self.render_pane_layout(second, selected_session_id, second_path, cx);
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
                    .bg(DARK.border_subtle)
                    .hover(|divider| divider.bg(DARK.success))
                    .when(axis == WorkspaceSplitAxis::Horizontal, |divider| {
                        divider.w(px(5.0)).h_full().cursor_ew_resize()
                    })
                    .when(axis == WorkspaceSplitAxis::Vertical, |divider| {
                        divider.h(px(5.0)).w_full().cursor_ns_resize()
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
                self.render_pane_layout(
                    &PaneLayoutSnapshot::terminal(zoomed_id),
                    tab.selected_session_id,
                    Vec::new(),
                    cx,
                )
            } else {
                self.render_pane_layout(&tab.layout, tab.selected_session_id, Vec::new(), cx)
            }
        });
        let is_empty = panes.is_none();
        let zoomed = tab.as_ref().and_then(|tab| tab.zoomed_session_id).is_some();

        div()
            .flex_1()
            .min_h(px(0.0))
            .relative()
            .overflow_hidden()
            .p_2()
            .bg(DARK.terminal)
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
                        .bg(DARK.elevated)
                        .text_xs()
                        .text_color(DARK.muted)
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
                                .text_color(DARK.muted)
                                .child(">_"),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(DARK.muted)
                                .child("Ninguna terminal seleccionada"),
                        ),
                )
            })
    }

    fn right_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = self.right_sidebar_mode;
        let width = match mode {
            RightSidebarMode::Files => 300.0,
            RightSidebarMode::Diff => self.diff_view.read(cx).preferred_width(),
        };
        let content = match mode {
            RightSidebarMode::Files => self.files_sidebar_content(cx),
            RightSidebarMode::Diff => self.diff_view.clone().into_any_element(),
        };
        let modes = [
            (RightSidebarMode::Files, "Files"),
            (RightSidebarMode::Diff, "Diff"),
        ];

        div()
            .w(px(width))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(DARK.panel)
            .border_l_1()
            .border_color(DARK.border_subtle)
            .child(
                div()
                    .h(px(34.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .border_b_1()
                    .border_color(DARK.border_subtle)
                    .children(modes.into_iter().map(|(item_mode, label)| {
                        let selected = item_mode == mode;
                        div()
                            .id(SharedString::from(format!("utility-mode-{label}")))
                            .flex_1()
                            .h(px(24.0))
                            .rounded(px(5.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .text_size(px(9.5))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(if selected {
                                DARK.foreground
                            } else {
                                DARK.subtle
                            })
                            .bg(if selected { DARK.selection } else { DARK.panel })
                            .hover(|tab| tab.bg(DARK.hover).text_color(DARK.foreground))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.right_sidebar_mode = item_mode;
                                match item_mode {
                                    RightSidebarMode::Files => this.refresh_project_files(),
                                    RightSidebarMode::Diff => {
                                        this.sync_diff_root(cx);
                                        this.diff_view.update(cx, |diff_view, cx| {
                                            diff_view.refresh_now(cx);
                                        });
                                    }
                                }
                                cx.notify();
                            }))
                            .child(label)
                    })),
            )
            .child(content)
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
                .bg(gpui::rgba(0x08080acc))
                .child(
                    div()
                        .w(px(560.0))
                        .max_w_full()
                        .max_h(px(460.0))
                        .mx_4()
                        .rounded_lg()
                        .border_1()
                        .border_color(DARK.border_subtle)
                        .bg(DARK.elevated)
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
                                .border_color(DARK.border_subtle)
                                .child(
                                    div()
                                        .font_family("JetBrains Mono")
                                        .text_color(DARK.success)
                                        .child(">"),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .font_family("JetBrains Mono")
                                        .text_size(px(12.0))
                                        .text_color(if query.is_empty() {
                                            DARK.subtle
                                        } else {
                                            DARK.foreground
                                        })
                                        .child(if query.is_empty() {
                                            placeholder.to_owned()
                                        } else {
                                            query
                                        }),
                                )
                                .child(div().text_xs().text_color(DARK.subtle).child("esc")),
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
                                            DARK.selection
                                        } else {
                                            DARK.elevated
                                        })
                                        .hover(|row| row.bg(DARK.hover))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.execute_palette_action(action.clone(), window, cx);
                                        }))
                                        .child(
                                            div()
                                                .w(px(18.0))
                                                .text_center()
                                                .font_family("JetBrains Mono")
                                                .text_color(if active {
                                                    DARK.success
                                                } else {
                                                    DARK.subtle
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
                                                    DARK.foreground
                                                } else {
                                                    DARK.muted
                                                })
                                                .child(item.label),
                                        )
                                        .child(
                                            div()
                                                .font_family("JetBrains Mono")
                                                .text_size(px(8.5))
                                                .text_color(DARK.subtle)
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
                                    .text_color(DARK.subtle)
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
                                .border_color(DARK.border_subtle)
                                .text_xs()
                                .text_color(DARK.subtle)
                                .child("↑↓ navegar · ↵ ejecutar"),
                        ),
                )
                .into_any_element(),
        )
    }

    fn file_modal(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if let Some(prompt) = self.file_prompt.clone() {
            let title = match prompt.kind {
                FilePromptKind::NewFile => "Crear archivo",
                FilePromptKind::NewDirectory => "Crear carpeta",
                FilePromptKind::Rename => "Renombrar",
            };
            let value = if prompt.value.is_empty() {
                "Escribe un nombre…".to_owned()
            } else {
                prompt.value
            };
            return Some(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(gpui::rgba(0x08080acc))
                    .child(
                        div()
                            .w(px(420.0))
                            .max_w_full()
                            .mx_4()
                            .p_4()
                            .rounded_lg()
                            .border_1()
                            .border_color(DARK.border_subtle)
                            .bg(DARK.elevated)
                            .shadow_lg()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(DARK.foreground)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .h(px(34.0))
                                    .px_3()
                                    .rounded(px(5.0))
                                    .border_1()
                                    .border_color(DARK.success)
                                    .bg(DARK.terminal)
                                    .flex()
                                    .items_center()
                                    .font_family("JetBrains Mono")
                                    .text_size(px(11.0))
                                    .text_color(if value == "Escribe un nombre…" {
                                        DARK.subtle
                                    } else {
                                        DARK.foreground
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
                                            .text_color(DARK.subtle)
                                            .child("↵ confirmar · esc cancelar"),
                                    )
                                    .child(
                                        div()
                                            .id("confirm-file-prompt")
                                            .px_3()
                                            .py_1()
                                            .rounded(px(5.0))
                                            .cursor_pointer()
                                            .bg(DARK.success)
                                            .text_xs()
                                            .text_color(DARK.terminal)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.confirm_file_prompt(cx);
                                            }))
                                            .child("Confirmar"),
                                    ),
                            ),
                    )
                    .into_any_element(),
            );
        }

        self.pending_file_trash.clone().map(|path| {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::rgba(0x08080acc))
                .child(
                    div()
                        .w(px(420.0))
                        .max_w_full()
                        .mx_4()
                        .p_4()
                        .rounded_lg()
                        .border_1()
                        .border_color(DARK.border_subtle)
                        .bg(DARK.elevated)
                        .shadow_lg()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(DARK.foreground)
                                .child(format!("¿Mover “{name}” a la Papelera?")),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(DARK.muted)
                                .child("La operación es recuperable desde la Papelera de macOS."),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(DARK.subtle)
                                        .child("↵ mover · esc cancelar"),
                                )
                                .child(
                                    div()
                                        .id("confirm-file-trash")
                                        .px_3()
                                        .py_1()
                                        .rounded(px(5.0))
                                        .cursor_pointer()
                                        .bg(DARK.danger)
                                        .text_xs()
                                        .text_color(DARK.foreground)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.confirm_file_trash(cx);
                                        }))
                                        .child("Mover"),
                                ),
                        ),
                )
                .into_any_element()
        })
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
                .bg(DARK.elevated)
                .border_b_1()
                .border_color(DARK.danger)
                .text_size(px(10.5))
                .text_color(DARK.danger)
                .child(div().size(px(5.0)).rounded_full().bg(DARK.danger))
                .child(error.clone())
        })
    }

    fn chrome_button(
        &self,
        label: &'static str,
        id: &'static str,
        selected: bool,
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
            .bg(if selected {
                DARK.selection
            } else {
                DARK.terminal
            })
            .text_size(px(14.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(if selected {
                DARK.foreground
            } else {
                DARK.muted
            })
            .hover(|button| button.bg(DARK.hover).text_color(DARK.foreground))
            .active(|button| button.opacity(0.72))
            .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
            .child(label)
    }

    fn sidebar_icon(left: bool) -> Div {
        let panel = div().w(px(4.0)).h_full().flex_none().bg(DARK.foreground);
        let content = div().h_full().flex_1();
        let icon = div()
            .w(px(14.0))
            .h(px(12.0))
            .flex()
            .overflow_hidden()
            .rounded(px(2.0))
            .border_1()
            .border_color(DARK.muted);

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
            .bg(DARK.titlebar)
            .hover(|button| button.bg(DARK.hover))
            .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
            .child(Self::sidebar_icon(left))
    }
}

impl Render for WorkspaceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.initial_terminal_focus_pending {
            self.initial_terminal_focus_pending = false;
            cx.defer_in(window, |this, window, cx| {
                this.focus_selected_terminal(window, cx);
            });
        }
        let mut body = div()
            .id("vibra-root")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::new_workspace))
            .on_action(cx.listener(Self::new_terminal_tab))
            .on_action(cx.listener(Self::close_terminal))
            .on_action(cx.listener(Self::toggle_right_sidebar))
            .on_action(cx.listener(Self::previous_workspace))
            .on_action(cx.listener(Self::next_workspace))
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
            .capture_key_down(cx.listener(Self::on_workspace_key_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::finish_pane_resize))
            .size_full()
            .flex()
            .flex_col()
            .font_family(".SystemUIFont")
            .text_size(px(12.0))
            .text_color(DARK.foreground)
            .bg(DARK.background)
            .child(self.titlebar(cx));

        if let Some(banner) = self.error_banner() {
            body = body.child(banner);
        }

        let mut layout = div().flex_1().min_h(px(0.0)).flex();
        if self.left_sidebar_visible {
            layout = layout.child(self.sidebar(cx));
        }
        layout = layout.child(self.center_panel(cx));
        if self.right_sidebar_visible {
            layout = layout.child(self.right_sidebar(cx));
        }

        body = body.child(layout);
        if let Some(modal) = self.palette_modal(cx) {
            body = body.child(modal);
        } else if let Some(modal) = self.file_modal(cx) {
            body = body.child(modal);
        }
        body
    }
}

fn collect_project_files(
    port: &dyn FileSystemPort,
    root: &Path,
    directory: &Path,
    depth: usize,
    expanded: &HashSet<PathBuf>,
    show_hidden: bool,
    output: &mut Vec<ProjectFileRow>,
) -> anyhow::Result<()> {
    const MAX_VISIBLE_FILE_ROWS: usize = 5_000;
    if output.len() >= MAX_VISIBLE_FILE_ROWS {
        return Ok(());
    }
    for entry in port.list_directory(root, directory, show_hidden)? {
        if output.len() >= MAX_VISIBLE_FILE_ROWS {
            break;
        }
        let is_expanded = entry.kind == FileEntryKind::Directory && expanded.contains(&entry.path);
        let child_path = entry.path.clone();
        let is_directory = entry.kind == FileEntryKind::Directory;
        output.push(ProjectFileRow {
            entry,
            depth,
            expanded: is_expanded,
        });
        if is_directory && is_expanded {
            collect_project_files(
                port,
                root,
                &child_path,
                depth + 1,
                expanded,
                show_hidden,
                output,
            )?;
        }
    }
    Ok(())
}

fn collect_search_files(
    port: &dyn FileSystemPort,
    root: &Path,
    directory: &Path,
    output: &mut Vec<PathBuf>,
) -> anyhow::Result<()> {
    const MAX_INDEXED_FILES: usize = 20_000;
    if output.len() >= MAX_INDEXED_FILES {
        return Ok(());
    }
    for entry in port.list_directory(root, directory, false)? {
        if output.len() >= MAX_INDEXED_FILES {
            break;
        }
        match entry.kind {
            FileEntryKind::Directory
                if !matches!(
                    entry.name.as_str(),
                    "target" | "node_modules" | "dist" | "build" | ".next" | "DerivedData"
                ) =>
            {
                collect_search_files(port, root, &entry.path, output)?;
            }
            FileEntryKind::File => output.push(entry.path),
            FileEntryKind::Directory | FileEntryKind::Symlink => {}
        }
    }
    Ok(())
}

fn automation_split_direction(direction: AutomationDirection) -> PaneSplitDirection {
    match direction {
        AutomationDirection::Left => PaneSplitDirection::Left,
        AutomationDirection::Right => PaneSplitDirection::Right,
        AutomationDirection::Up => PaneSplitDirection::Up,
        AutomationDirection::Down => PaneSplitDirection::Down,
    }
}

fn automation_focus_direction(direction: AutomationDirection) -> PaneFocusDirection {
    match direction {
        AutomationDirection::Left => PaneFocusDirection::Left,
        AutomationDirection::Right => PaneFocusDirection::Right,
        AutomationDirection::Up => PaneFocusDirection::Up,
        AutomationDirection::Down => PaneFocusDirection::Down,
    }
}

fn agent_runtime_state_label(state: AgentRuntimeState) -> &'static str {
    match state {
        AgentRuntimeState::Idle => "idle",
        AgentRuntimeState::Working => "working",
        AgentRuntimeState::Waiting => "waiting",
    }
}

fn terminal_agent_state_label(state: TerminalAgentState) -> &'static str {
    match state {
        TerminalAgentState::Idle => "idle",
        TerminalAgentState::Working => "working",
        TerminalAgentState::Waiting => "waiting",
    }
}
