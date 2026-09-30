use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{Context, FocusHandle, SharedString, px};

use crate::domain::inbox::Inbox;
use crate::domain::library::Library;
use crate::domain::workspace::{PaneSplitDirection, WorkspaceSnapshot};
use crate::infrastructure::automation::{AutomationServer, agent_hook_status};
use crate::infrastructure::library::LibraryRepository;
use crate::infrastructure::persistence::{FinishError, PersistenceQueue};
use crate::infrastructure::settings::AppSettings;
use crate::ui::diff_view::{DiffFileIndexView, DiffView, DiffViewEvent};
use crate::ui::terminal::TerminalInsertStatus;
use crate::ui::theme;

use super::*;

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
                            "Could not restore the workspace: {error}. ",
                            "Changes will not be saved until the file is repaired."
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
                    "Could not load settings: {error}. Changes will not be saved until the file is repaired."
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
                    "Could not load notes and automations: {error}. Changes will not be saved until the file is repaired."
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
            persistence_error = Some(format!("Could not save the workspace: {error}").into());
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
                            Some(format!("Local automation unavailable: {error}").into());
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
                    this.sync_review_docking(cx);
                    if let Some(tab_id) = this.review_dock_owner() {
                        this.snapshot.select_tab(tab_id);
                    }
                    this.review_tab_active = true;
                    this.sync_terminal_surface_visibility(cx);
                    cx.notify();
                }
                DiffViewEvent::Changed => {
                    this.sync_review_docking(cx);
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
                    this.sync_inbox_review_preferences(cx);
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
                Some(format!("Could not query integrations: {error}").into()),
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
                        Some(format!("Background saving unavailable: {error}").into());
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
            project_diff_stats: HashMap::new(),
            _status_task: None,
            usage: usage::UsageState::default(),
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
            left_sidebar_progress: if settings.left_sidebar_visible {
                1.0
            } else {
                0.0
            },
            workspace_section: WorkspaceSection::Workspace,
            review_tab_active: false,
            review_split_direction: PaneSplitDirection::Right,
            review_docked_tab_id: None,
            pane_drop_preview: None,
            tab_strip_drop: None,
            tab_motion: SlotMotion::default(),
            tab_width: Rc::new(Cell::new(px(0.0))),
            project_motion: RefCell::new([SlotMotion::default(), SlotMotion::default()]),
            project_drop: None,
            navigation: tabs::Navigation::default(),
            inbox: Inbox::default(),
            work_inbox: work_inbox::WorkInbox::default(),
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
            settings_page: SettingsPage::General,
            theme_query: String::new(),
            context_menu: None,
            ide_menu_open: false,
            ide_discovering: false,
            installed_editors: Vec::new(),
            ide_icons: HashMap::new(),
            rename_prompt: None,
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
            dismissed_banner_errors: HashSet::new(),
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
        view.start_usage_poll(cx);
        view.start_inbox_poll(cx);
        view
    }
}
