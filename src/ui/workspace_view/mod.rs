//! Workspace state and coordination. Feature modules share this view's state:
//! settings owns its pages and automation resolves agent activity.
//! None of them introduces a second workspace model.

mod actions;
mod automation;
mod bootstrap;
mod chrome;
mod context_menu;
mod drag;
mod explorer;
mod files;
mod inbox;
mod input;
mod navigation;
mod palette;
mod panes;
mod projects;
mod settings;
mod status_bar;
mod storage;
mod tab_drag;
mod tabs;
mod terminals;
mod titlebar;
mod types;
mod usage;
mod work_inbox;

use automation::HookAgentPresence;
pub(crate) use chrome::icon_button;
use chrome::*;
pub(crate) use drag::*;
pub(crate) use files::{file_tree_icon, file_tree_icon_color};
use settings::SettingsPage;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    Context, DragMoveEvent, Entity, FocusHandle, IntoElement, MouseButton, ParentElement, Render,
    SharedString, Styled, Subscription, Task, Timer, Window, div, prelude::*, px,
};
use uuid::Uuid;

use crate::domain::inbox::Inbox;
use crate::domain::workspace::{PaneSplitDirection, WorkspaceSnapshot};
use crate::infrastructure::automation::{AgentHookStatus, AutomationServer};
use crate::infrastructure::editor::InstalledEditor;
use crate::infrastructure::notifications::AgentActivitySnapshot;
use crate::infrastructure::persistence::{PersistenceQueue, WorkspaceRepository};
use crate::infrastructure::settings::{
    AppSettings, MAX_LEFT_SIDEBAR_WIDTH, MAX_RIGHT_SIDEBAR_WIDTH, MIN_LEFT_SIDEBAR_WIDTH,
    MIN_RIGHT_SIDEBAR_WIDTH, SettingsRepository,
};
use crate::ports::files::FileSystemPort;
use crate::ports::git::GitPort;
use crate::ports::terminal::TerminalAgentPresence;
use crate::ports::terminal::TerminalPort;
use crate::ui::diff_view::{DiffFileIndexView, DiffView};
use crate::ui::terminal::TerminalView;
use crate::ui::theme::{colors, surface, window_surface};

/// Titlebar chrome width when the left sidebar is fully collapsed.
const TITLEBAR_CHROME_COLLAPSED: f32 = 184.0;
/// Titlebar chrome width when the right sidebar is fully collapsed (toggle only).
const TITLEBAR_RIGHT_CHROME_COLLAPSED: f32 = 44.0;
const TITLEBAR_HEIGHT: f32 = 40.0;
/// Open/close duration — short enough to feel snappy, long enough to read as motion.
const SIDEBAR_ANIM_DURATION: Duration = Duration::from_millis(160);
/// ~60 fps ticks; only runs while a sidebar is mid-animation.
const SIDEBAR_ANIM_FRAME: Duration = Duration::from_millis(16);
pub(super) use types::*;

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
    project_diff_stats: HashMap<PathBuf, projects::ProjectDiffStats>,
    _status_task: Option<Task<()>>,
    usage: usage::UsageState,
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
    /// Visual open amount for the left sidebar (`0.0` closed … `1.0` open).
    left_sidebar_progress: f32,
    workspace_section: WorkspaceSection,
    /// The open review's tab is in front of the terminal tabs.
    review_tab_active: bool,
    review_split_direction: PaneSplitDirection,
    review_docked_tab_id: Option<Uuid>,
    pane_drop_preview: Option<(Uuid, PaneSplitDirection)>,
    tab_strip_drop: Option<TabStripDrop>,
    tab_motion: SlotMotion<crate::domain::workspace::WorkspaceTabId>,
    /// Width of a tab in the strip, which is the distance a reordered tab slides.
    tab_width: Rc<Cell<gpui::Pixels>>,
    /// Pinned and unpinned sections slide independently.
    project_motion: RefCell<[SlotMotion<Uuid>; 2]>,
    /// The project a dragged one lands before; `Some(None)` is the end.
    project_drop: Option<Option<Uuid>>,
    navigation: tabs::Navigation,
    /// Agent and automation events, newest last; lives only while the app runs.
    inbox: Inbox,
    work_inbox: work_inbox::WorkInbox,
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
    settings_page: SettingsPage,
    theme_query: String,
    context_menu: Option<ContextMenuState>,
    ide_menu_open: bool,
    ide_discovering: bool,
    installed_editors: Vec<InstalledEditor>,
    ide_icons: HashMap<&'static str, Arc<gpui::Image>>,
    rename_prompt: Option<RenamePrompt>,
    /// Visual open amount for the right sidebar (`0.0` closed … `1.0` open).
    right_sidebar_progress: f32,
    right_sidebar_mode: RightSidebarMode,
    sidebar_anim_token: u64,
    _sidebar_anim_task: Option<Task<()>>,
    initial_terminal_focus_pending: bool,
    pane_resize_dirty: bool,
    sidebar_resize_dirty: bool,
    reorder_drag: Option<ReorderDrag>,
    dismissed_banner_errors: HashSet<SharedString>,
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
    fn sync_git_panel_visibility(&self, cx: &mut Context<Self>) {
        let visible = self.has_project_context()
            && self.workspace_section == WorkspaceSection::Workspace
            && (self.review_visible(cx)
                || self.settings.right_sidebar_visible
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
        self.set_right_sidebar_visible(!self.settings.right_sidebar_visible, true, cx);
        if self.settings.right_sidebar_visible {
            self.sync_diff_root(cx);
        }
    }

    /// Desired open/closed state for the left sidebar, with a light width animation.
    fn set_left_sidebar_visible(&mut self, visible: bool, persist: bool, cx: &mut Context<Self>) {
        if self.settings.left_sidebar_visible == visible {
            // Caller may have changed mode/content; repaint without restarting motion.
            cx.notify();
            return;
        }
        self.settings.left_sidebar_visible = visible;
        if persist {
            self.persist_settings(cx);
        }
        self.start_sidebar_animation(cx);
    }

    /// Desired open/closed state for the right sidebar, with a light width animation.
    fn set_right_sidebar_visible(&mut self, visible: bool, persist: bool, cx: &mut Context<Self>) {
        if self.settings.right_sidebar_visible == visible {
            self.sync_git_panel_visibility(cx);
            self.sync_files_watcher(cx);
            cx.notify();
            return;
        }
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
        let left_to = if self.settings.left_sidebar_visible {
            1.0
        } else {
            0.0
        };
        let right_to = if self.settings.right_sidebar_visible {
            1.0
        } else {
            0.0
        };
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
        self.review_docked_tab_id = None;
        self.diff_view
            .update(cx, |diff, cx| diff.set_review_expanded(false, cx));
        self.sync_terminal_surface_visibility(cx);
        self.sync_git_panel_visibility(cx);
        self.focus_selected_terminal(window, cx);
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

    fn sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let content = self.global_sidebar_content(cx);
        let full_width = self.left_sidebar_width();
        let width = full_width * self.left_sidebar_progress;
        let show_handle = self.left_sidebar_progress > 0.99;
        // The native backdrop is tinted once underneath this panel.
        clipped_width_panel(
            width,
            full_width,
            SidebarResizeEdge::Left,
            colors().sidebar,
            content,
        )
        .when(show_handle, |sidebar| {
            sidebar.child(self.sidebar_resize_handle(
                "resize-left-sidebar",
                SidebarResizeEdge::Left,
                cx,
            ))
        })
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
                    "Link a folder to this project"
                } else {
                    "Select a project"
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
        clipped_width_panel(
            width,
            full_width,
            SidebarResizeEdge::Right,
            colors().sidebar,
            content,
        )
        .when(show_handle, |sidebar| {
            sidebar.child(self.sidebar_resize_handle(
                "resize-right-sidebar",
                SidebarResizeEdge::Right,
                cx,
            ))
        })
    }

    fn current_banner_errors(&self) -> Vec<SharedString> {
        [
            self.persistence_error.as_ref(),
            self.workspace_save_error.as_ref(),
            self.settings_save_error.as_ref(),
        ]
        .into_iter()
        .flatten()
        .cloned()
        .collect()
    }

    fn dismiss_error_banner(&mut self, cx: &mut Context<Self>) {
        // Load errors also prevent unsafe saves; dismiss only their presentation.
        self.dismissed_banner_errors
            .extend(self.current_banner_errors());
        cx.notify();
    }

    fn error_banner(&mut self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let mut errors = self.current_banner_errors();
        // Once an error clears, a later occurrence may be shown again.
        self.dismissed_banner_errors
            .retain(|error| errors.contains(error));
        errors.retain(|error| !self.dismissed_banner_errors.contains(error));
        (!errors.is_empty()).then(|| {
            let error = errors
                .iter()
                .map(|error| error.as_ref())
                .collect::<Vec<_>>()
                .join(" · ");
            div()
                .w_full()
                .min_w(px(0.0))
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
                .child(
                    div()
                        .id("dismiss-error-banner")
                        .size(px(22.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.0))
                        .cursor_pointer()
                        .hover(|button| button.bg(colors().hover))
                        .tooltip(|_, cx| sidebar_tooltip("Dismiss errors", cx))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.dismiss_error_banner(cx);
                            cx.stop_propagation();
                        }))
                        .child(
                            gpui::svg()
                                .path("chrome-icons/close.svg")
                                .size(px(12.0))
                                .text_color(colors().danger),
                        ),
                )
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
        let mut body = self
            .bind_workspace_actions(div().id("vibra-root").track_focus(&self.focus_handle), cx)
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

        if let Some(banner) = self.error_banner(cx) {
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
        if self.workspace_section == WorkspaceSection::Settings {
            layout = layout.child(self.settings_sidebar(cx));
        } else if self.left_sidebar_progress > 0.001 {
            layout = layout.child(self.sidebar(cx));
        }
        if self.workspace_section == WorkspaceSection::Workspace {
            let review_pane = || {
                div()
                    .id("review-pane")
                    .flex_1()
                    .min_w(px(0.0))
                    .h_full()
                    .overflow_hidden()
                    .bg(surface(colors().panel))
            };
            if expanded_review && self.diff_view.read(cx).review_focused() {
                layout = layout.child(
                    review_pane()
                        .on_drop(cx.listener(|this, drag: &TabDrag, window, cx| {
                            this.dock_tab(
                                drag.tab_id,
                                crate::domain::workspace::WorkspaceTabId::Review,
                                window,
                                cx,
                            );
                        }))
                        .drag_over::<TabDrag>(|style, _, _, _| {
                            style.border_2().border_color(colors().accent)
                        })
                        .child(self.diff_view.clone()),
                );
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
            layout = layout.child(self.global_section_content(window, cx));
        }

        body = body.child(layout);
        if self.workspace_section != WorkspaceSection::Settings {
            body = body.child(self.status_bar(cx));
        }
        if let Some(popover) = self.inbox_popover(window, cx) {
            body = body.child(popover);
        }
        if let Some(popover) = self.usage_popover(window, cx) {
            body = body.child(popover);
        }
        if let Some(modal) = self.palette_modal(cx) {
            body = body.child(modal);
        } else if let Some(modal) = self.rename_modal(cx) {
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
