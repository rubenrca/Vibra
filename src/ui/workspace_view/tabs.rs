//! The review as a tab beside the terminal tabs, back/forward navigation
//! between tabs and projects, and the resizable terminal/review split.

use gpui::{
    AnyElement, Context, DragMoveEvent, MouseButton, Window, div, prelude::*, px, relative, svg,
};
use uuid::Uuid;

use crate::domain::workspace::{PaneSplitDirection, WorkspaceTabId};
use crate::infrastructure::settings::{MAX_REVIEW_SPLIT, MIN_REVIEW_SPLIT};
use crate::ui::theme::{colors, surface_tint};
use crate::{GoToTab, NavigateBack, NavigateForward};

use super::panes::{
    TAB_HEIGHT, TAB_MAX_WIDTH, TAB_RADIUS, TAB_TEXT_SIZE, tab_drag_ghost, tab_shortcut,
};
use super::titlebar::{TITLEBAR_BUTTON_GAP, titlebar_button, titlebar_icon};
use super::{DragGhost, ReorderDrag, TabDrag, WorkspaceSection, WorkspaceView, sidebar_tooltip};

const MAX_NAVIGATION_HISTORY: usize = 50;

/// Numbered tab and project shortcuts use `9` for last and clamp larger
/// destinations to the last item.
pub(super) fn numbered_navigation_index(number: usize, count: usize) -> Option<usize> {
    if number == 0 || count == 0 {
        return None;
    }
    Some(if number >= 9 {
        count - 1
    } else {
        (number - 1).min(count - 1)
    })
}

pub(super) fn tab_shortcut_label(index: usize, count: usize) -> Option<String> {
    if index < 8 {
        Some(format!("⌘{}", index + 1))
    } else if index + 1 == count {
        Some("⌘9".into())
    } else {
        None
    }
}

/// A place the user can go back to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NavLocation {
    Section(WorkspaceSection),
    Terminal {
        project_id: Uuid,
        workspace_id: Uuid,
        tab_id: Uuid,
    },
    Review {
        project_id: Uuid,
        workspace_id: Uuid,
    },
}

/// Drag payload for the divider between the terminal and the review.
#[derive(Clone, Copy)]
pub(crate) struct ReviewSplitDrag;

pub(crate) struct ReviewSplitDragView;

impl gpui::Render for ReviewSplitDragView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// Back/forward stacks. A move is recorded when the visible location changes,
/// whatever caused it: a tab click, a shortcut, the palette, or the Inbox.
#[derive(Debug, Default)]
pub(super) struct Navigation {
    back: Vec<NavLocation>,
    forward: Vec<NavLocation>,
    current: Option<NavLocation>,
}

impl Navigation {
    /// Returns whether the stacks changed.
    pub(super) fn observe(&mut self, location: Option<NavLocation>) -> bool {
        let Some(location) = location else {
            return false;
        };
        if self.current == Some(location) {
            return false;
        }
        if let Some(previous) = self.current.replace(location) {
            self.back.push(previous);
            if self.back.len() > MAX_NAVIGATION_HISTORY {
                self.back.remove(0);
            }
            self.forward.clear();
        }
        true
    }

    fn step(&mut self, back: bool, mut apply: impl FnMut(NavLocation) -> bool) -> bool {
        loop {
            let target = if back {
                self.back.pop()
            } else {
                self.forward.pop()
            };
            let Some(target) = target else {
                return false;
            };
            if self.current == Some(target) {
                continue;
            }
            if apply(target) {
                if let Some(current) = self.current.replace(target) {
                    if back {
                        self.forward.push(current);
                    } else {
                        self.back.push(current);
                    }
                }
                return true;
            }
        }
    }

    pub(super) fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub(super) fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }
}

impl WorkspaceView {
    pub(super) fn review_dock_owner(&self) -> Option<Uuid> {
        self.review_docked_tab_id.filter(|id| {
            self.snapshot
                .selected_workspace()
                .is_some_and(|workspace| workspace.tabs.iter().any(|tab| tab.id == *id))
        })
    }

    /// The review is transient, but while open a dock belongs to one tab.
    pub(super) fn sync_review_docking(&mut self, cx: &mut Context<Self>) {
        let diff = self.diff_view.read(cx);
        if !diff.review_expanded() || diff.review_focused() {
            self.review_docked_tab_id = None;
        } else if self.review_docked_tab_id.is_none() {
            self.review_docked_tab_id = self.snapshot.selected_tab().map(|tab| tab.id);
        } else if self.review_dock_owner().is_none() {
            // Closing the owner leaves the review available as an independent tab.
            self.review_docked_tab_id = None;
            self.diff_view
                .update(cx, |diff, cx| diff.set_review_focused(true, cx));
        }
    }

    pub(super) fn review_has_tab(&self, cx: &gpui::App) -> bool {
        self.diff_view.read(cx).review_expanded() && self.review_dock_owner().is_none()
    }

    pub(super) fn visible_tab_order(&self, cx: &gpui::App) -> Vec<WorkspaceTabId> {
        let review_open = self.review_has_tab(cx);
        self.snapshot
            .selected_workspace()
            .map(|workspace| workspace.tab_order(review_open))
            .unwrap_or_else(|| {
                if review_open {
                    vec![WorkspaceTabId::Review]
                } else {
                    Vec::new()
                }
            })
    }

    /// The review tab is open and in front.
    pub(super) fn review_visible(&self, cx: &gpui::App) -> bool {
        self.workspace_section == WorkspaceSection::Workspace
            && match self.review_dock_owner() {
                Some(tab_id) => self
                    .snapshot
                    .selected_tab()
                    .is_some_and(|tab| tab.id == tab_id),
                None => self.review_tab_active,
            }
            && self.diff_view.read(cx).review_expanded()
    }

    /// The review fills the center and the terminal is hidden.
    pub(super) fn review_covers_terminal(&self, cx: &gpui::App) -> bool {
        self.review_visible(cx) && self.diff_view.read(cx).review_focused()
    }

    pub(super) fn current_location(&self, cx: &gpui::App) -> Option<NavLocation> {
        if self.workspace_section != WorkspaceSection::Workspace {
            return Some(NavLocation::Section(self.workspace_section));
        }
        let project_id = self.snapshot.selected_project_id?;
        let workspace = self.snapshot.selected_workspace()?;
        if self.review_visible(cx) && self.review_dock_owner().is_none() {
            return Some(NavLocation::Review {
                project_id,
                workspace_id: workspace.id,
            });
        }
        Some(NavLocation::Terminal {
            project_id,
            workspace_id: workspace.id,
            tab_id: self.snapshot.selected_tab()?.id,
        })
    }

    pub(super) fn record_navigation(&mut self, cx: &gpui::App) {
        let location = self.current_location(cx);
        self.navigation.observe(location);
    }

    pub(super) fn activate_review_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.diff_view.read(cx).review_expanded() {
            return;
        }
        if let Some(tab_id) = self.review_dock_owner() {
            self.snapshot.select_tab(tab_id);
        }
        self.workspace_section = WorkspaceSection::Workspace;
        self.review_tab_active = true;
        self.pending_focus_session = None;
        self.context_menu = None;
        self.ide_menu_open = false;
        self.sync_terminal_surface_visibility(cx);
        self.sync_git_panel_visibility(cx);
        self.sync_files_watcher(cx);
        self.diff_view.read(cx).focus_review(window);
        cx.notify();
    }

    /// Brings a terminal tab forward; an open review keeps its tab.
    pub(super) fn show_terminal_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prepare_terminal_tab(cx);
        self.persist(cx);
        self.focus_selected_terminal(window, cx);
    }

    /// Shared transition for interactive navigation and commands that focus
    /// their new terminal on the next frame (for example, review launches).
    pub(super) fn prepare_terminal_tab(&mut self, cx: &mut Context<Self>) {
        self.sync_review_docking(cx);
        self.workspace_section = WorkspaceSection::Workspace;
        self.review_tab_active = false;
        self.pending_focus_session = None;
        self.context_menu = None;
        self.ide_menu_open = false;
        self.close_palette(cx);
        if let Some(session) = self.snapshot.selected_session() {
            self.inbox.mark_pane_read(session.id);
        }
        self.sync_terminal_surface_visibility(cx);
        self.sync_diff_root(cx);
        self.sync_git_panel_visibility(cx);
        self.refresh_project_files(cx);
    }

    /// Opens a tab named `title` in the project and types `command` into its
    /// shell. Returns the new terminal and whether the shell took the
    /// command. With `reveal` off the user's current tab stays selected.
    pub(super) fn run_in_new_tab(
        &mut self,
        project_id: Uuid,
        title: &str,
        command: &str,
        reveal: bool,
        cx: &mut Context<Self>,
    ) -> Result<(Uuid, bool), &'static str> {
        let (_, session_id) = self
            .snapshot
            .open_tab_in_project(project_id, reveal)
            .ok_or("The project has no associated folder.")?;
        // The tab keeps its name while its shell runs.
        self.pane_names.insert(session_id, title.to_owned());
        self.reconcile_terminal_views(cx);
        let started = self
            .terminals
            .get(&session_id)
            .cloned()
            .is_some_and(|terminal| {
                terminal.update(cx, |terminal, cx| terminal.run_command(command, cx))
            });
        if reveal {
            self.prepare_terminal_tab(cx);
            self.pending_focus_session = Some(session_id);
        }
        self.persist(cx);
        Ok((session_id, started))
    }

    fn apply_location(
        &mut self,
        location: NavLocation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let current_workspace = self.snapshot.selected_workspace().map(|item| item.id);
        match location {
            NavLocation::Section(section) => {
                self.select_section(section, window, cx);
                true
            }
            NavLocation::Review {
                project_id,
                workspace_id,
            } => {
                let here = self.snapshot.selected_project_id == Some(project_id)
                    && current_workspace == Some(workspace_id);
                if !here || !self.diff_view.read(cx).review_expanded() {
                    return false;
                }
                self.activate_review_tab(window, cx);
                true
            }
            NavLocation::Terminal {
                project_id,
                workspace_id,
                tab_id,
            } => {
                let exists = self
                    .snapshot
                    .project(project_id)
                    .and_then(|project| project.workspaces.as_deref())
                    .and_then(|workspaces| workspaces.iter().find(|item| item.id == workspace_id))
                    .is_some_and(|workspace| workspace.tabs.iter().any(|tab| tab.id == tab_id));
                if !exists {
                    return false;
                }
                self.snapshot.select_workspace(project_id, workspace_id);
                self.snapshot.select_tab(tab_id);
                self.show_terminal_tab(window, cx);
                true
            }
        }
    }

    fn navigate(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.record_navigation(cx);
        let mut navigation = std::mem::take(&mut self.navigation);
        navigation.step(back, |target| self.apply_location(target, window, cx));
        self.navigation = navigation;
        cx.notify();
    }

    pub(super) fn navigate_back(
        &mut self,
        _: &NavigateBack,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate(true, window, cx);
    }

    pub(super) fn navigate_forward(
        &mut self,
        _: &NavigateForward,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate(false, window, cx);
    }

    /// `⌘1`–`⌘8` follow the visible strip, including review; `⌘9` is the last tab.
    pub(super) fn go_to_tab(
        &mut self,
        action: &GoToTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.palette_mode.is_some() || self.rename_prompt.is_some() {
            return;
        }
        let order = self.visible_tab_order(cx);
        let Some(index) = numbered_navigation_index(action.index, order.len()) else {
            return;
        };
        match order[index] {
            WorkspaceTabId::Review => self.activate_review_tab(window, cx),
            WorkspaceTabId::Terminal(tab_id) => self.select_tab(tab_id, window, cx),
        }
    }

    /// Navigation arrows for the titlebar.
    pub(super) fn navigation_buttons(&self, cx: &mut Context<Self>) -> AnyElement {
        let button = |id: &'static str, icon: &'static str, label: &'static str, enabled: bool| {
            titlebar_button(id, enabled)
                .tooltip(move |_, cx| sidebar_tooltip(label, cx))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(titlebar_icon(icon))
        };
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(TITLEBAR_BUTTON_GAP))
            .child(
                button(
                    "navigate-back",
                    "chrome-icons/chevron-left.svg",
                    "Back · ⌃⌘←",
                    self.navigation.can_go_back(),
                )
                .on_click(cx.listener(|this, _, window, cx| this.navigate(true, window, cx))),
            )
            .child(
                button(
                    "navigate-forward",
                    "chrome-icons/chevron-right.svg",
                    "Forward · ⌃⌘→",
                    self.navigation.can_go_forward(),
                )
                .on_click(cx.listener(|this, _, window, cx| this.navigate(false, window, cx))),
            )
            .into_any_element()
    }

    /// The review, drawn like the terminal tabs so switching is one click.
    pub(super) fn review_tab(
        &self,
        index: usize,
        count: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.review_visible(cx);
        let diff = self.diff_view.read(cx);
        let title = diff.review_title();
        let icon = diff.review_icon();
        let close_label = if diff.preview_path().is_some() {
            "Close file · ⌘W"
        } else {
            "Close review · ⌘W"
        };
        let shortcut = tab_shortcut_label(index, count);
        let drag = TabDrag {
            tab_id: WorkspaceTabId::Review,
            title: title.to_string(),
            from_pane: false,
        };
        let ghost = {
            let title = title.to_string();
            DragGhost::new(TAB_RADIUS, colors().terminal, move || {
                tab_drag_ghost(
                    svg()
                        .path(icon)
                        .size(px(15.0))
                        .flex_none()
                        .text_color(colors().foreground),
                    title.clone(),
                )
            })
        };
        div()
            .id("review-tab")
            .relative()
            .group("title-tab")
            .h(px(TAB_HEIGHT))
            .min_w(px(0.0))
            .max_w(px(TAB_MAX_WIDTH))
            .flex_1()
            .flex()
            .items_center()
            .pl(px(10.0))
            .pr(px(6.0))
            .gap(px(8.0))
            .overflow_hidden()
            .rounded(px(TAB_RADIUS))
            .cursor_move()
            .bg(if selected {
                surface_tint(colors().selection, colors().terminal)
            } else {
                gpui::rgba(0x00000000)
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
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.reorder_drag = Some(ReorderDrag::Tab(WorkspaceTabId::Review));
                    cx.stop_propagation();
                }),
            )
            .child(ghost.measure())
            .on_drag(drag, move |_, _, _, cx| ghost.preview(cx))
            .on_click(cx.listener(|this, _, window, cx| this.activate_review_tab(window, cx)))
            .child(
                svg()
                    .path(icon)
                    .size(px(15.0))
                    .flex_none()
                    .text_color(if selected {
                        colors().foreground
                    } else {
                        colors().muted
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_size(px(TAB_TEXT_SIZE))
                    .font_weight(if selected {
                        gpui::FontWeight::MEDIUM
                    } else {
                        gpui::FontWeight::NORMAL
                    })
                    .child(title),
            )
            .when_some(shortcut, |tab, shortcut| tab.child(tab_shortcut(shortcut)))
            .children(self.tab_drag_zone(WorkspaceTabId::Review, cx))
            .child(
                div()
                    .id("close-review-tab")
                    .group("review-close")
                    .size(px(20.0))
                    .flex_none()
                    .rounded(px(5.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .text_color(colors().subtle)
                    .hover(|button| button.bg(colors().hover).text_color(colors().foreground))
                    .tooltip(move |_, cx| sidebar_tooltip(close_label, cx))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, window, cx| {
                        cx.stop_propagation();
                        this.close_review(window, cx);
                    }))
                    .child(
                        svg()
                            .path("chrome-icons/close.svg")
                            .size(px(12.0))
                            .flex_none()
                            .text_color(colors().muted)
                            .group_hover("review-close", |icon| {
                                icon.text_color(colors().foreground)
                            }),
                    ),
            )
            .map(|tab| {
                let slide = self.tab_motion.slide(WorkspaceTabId::Review);
                super::slide_into_place(tab, slide, "tab", false)
            })
    }

    /// Review can dock on any edge of the terminal area.
    pub(super) fn review_split(
        &mut self,
        terminal: AnyElement,
        review: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ratio = self.settings.review_split;
        let vertical = matches!(
            self.review_split_direction,
            PaneSplitDirection::Up | PaneSplitDirection::Down
        );
        let review_first = matches!(
            self.review_split_direction,
            PaneSplitDirection::Left | PaneSplitDirection::Up
        );
        let review = div()
            .size_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .child(self.review_pane_header(cx))
            .child(review)
            .into_any_element();
        let (first, second) = if review_first {
            (review, terminal)
        } else {
            (terminal, review)
        };
        div()
            .id("review-split")
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .h_full()
            .flex()
            .when(vertical, |split| split.flex_col())
            .on_drag_move(cx.listener(Self::on_review_split_move))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .when(vertical, |pane| pane.h(relative(ratio)).w_full())
                    .when(!vertical, |pane| pane.w(relative(ratio)).h_full())
                    .child(first),
            )
            .child(
                div()
                    .id("review-split-divider")
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(vertical, |divider| {
                        divider.w_full().h(px(5.0)).cursor_ns_resize()
                    })
                    .when(!vertical, |divider| {
                        divider.h_full().w(px(5.0)).cursor_ew_resize()
                    })
                    .hover(|divider| divider.bg(surface_tint(colors().hover, colors().panel)))
                    .on_drag(ReviewSplitDrag, |_, _, _, cx| {
                        cx.new(|_| ReviewSplitDragView)
                    })
                    .child(
                        div()
                            .bg(colors().border_subtle)
                            .when(vertical, |line| line.w_full().h(px(1.0)))
                            .when(!vertical, |line| line.h_full().w(px(1.0))),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .flex()
                    .child(second),
            )
            .into_any_element()
    }

    fn review_pane_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let title = self.diff_view.read(cx).review_title();
        let drag = TabDrag {
            tab_id: WorkspaceTabId::Review,
            title: title.to_string(),
            from_pane: true,
        };
        div()
            .id("review-pane-header")
            .h(px(30.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.0))
            .px(px(8.0))
            .border_b_1()
            .border_color(colors().border_subtle)
            .cursor_move()
            .text_size(px(12.5))
            .text_color(colors().foreground)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.reorder_drag = Some(ReorderDrag::Tab(WorkspaceTabId::Review));
                    cx.stop_propagation();
                }),
            )
            .on_drag(drag, |drag, offset, _, cx| drag.preview(offset, cx))
            .child(
                svg()
                    .path("chrome-icons/grip.svg")
                    .size(px(14.0))
                    .text_color(colors().subtle),
            )
            .child(div().flex_1().min_w(px(0.0)).truncate().child(title))
            .child(
                titlebar_button("review-pane-detach", true)
                    .tooltip(|_, cx| sidebar_tooltip("Open as tab", cx))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.review_docked_tab_id = None;
                        this.diff_view
                            .update(cx, |diff, cx| diff.set_review_focused(true, cx));
                        this.activate_review_tab(window, cx);
                    }))
                    .child(titlebar_icon("chrome-icons/maximize.svg")),
            )
            .child(
                titlebar_button("review-pane-close", true)
                    .tooltip(|_, cx| sidebar_tooltip("Close review", cx))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, window, cx| this.close_review(window, cx)))
                    .child(titlebar_icon("chrome-icons/close.svg")),
            )
            .into_any_element()
    }

    fn on_review_split_move(
        &mut self,
        event: &DragMoveEvent<ReviewSplitDrag>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let vertical = matches!(
            self.review_split_direction,
            PaneSplitDirection::Up | PaneSplitDirection::Down
        );
        let (position, origin, length): (f32, f32, f32) = if vertical {
            (
                event.event.position.y.into(),
                event.bounds.top().into(),
                event.bounds.size.height.into(),
            )
        } else {
            (
                event.event.position.x.into(),
                event.bounds.left().into(),
                event.bounds.size.width.into(),
            )
        };
        if length <= 1.0 {
            return;
        }
        let ratio = ((position - origin) / length).clamp(MIN_REVIEW_SPLIT, MAX_REVIEW_SPLIT);
        if (ratio - self.settings.review_split).abs() > 0.002 {
            self.settings.review_split = ratio;
            self.sidebar_resize_dirty = true;
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_shortcuts_match_their_numbered_destination() {
        assert_eq!(numbered_navigation_index(0, 3), None);
        assert_eq!(numbered_navigation_index(1, 0), None);
        assert_eq!(numbered_navigation_index(8, 3), Some(2));
        for count in 1..=12 {
            assert_eq!(numbered_navigation_index(9, count), Some(count - 1));
            for index in 0..count {
                if let Some(label) = tab_shortcut_label(index, count) {
                    let number: usize = label.trim_start_matches('⌘').parse().unwrap();
                    assert_eq!(numbered_navigation_index(number, count), Some(index));
                }
            }
        }
        assert_eq!(tab_shortcut_label(8, 10), None);
        assert_eq!(tab_shortcut_label(9, 10).as_deref(), Some("⌘9"));
    }

    fn terminal(tab: u128) -> NavLocation {
        NavLocation::Terminal {
            project_id: Uuid::from_u128(1),
            workspace_id: Uuid::from_u128(2),
            tab_id: Uuid::from_u128(tab),
        }
    }

    #[test]
    fn history_records_moves_and_skips_unreachable_places() {
        let mut navigation = Navigation::default();
        assert!(!navigation.observe(None));
        navigation.observe(Some(terminal(1)));
        navigation.observe(Some(terminal(2)));
        navigation.observe(Some(terminal(2)));
        navigation.observe(Some(terminal(3)));
        assert!(navigation.can_go_back());
        assert!(!navigation.can_go_forward());

        // Tab 2 was closed: going back lands on tab 1.
        assert!(navigation.step(true, |target| target != terminal(2)));
        assert_eq!(navigation.current, Some(terminal(1)));
        assert!(navigation.can_go_forward());
        assert!(navigation.step(false, |_| true));
        assert_eq!(navigation.current, Some(terminal(3)));

        // A new move drops the forward stack.
        navigation.step(true, |_| true);
        navigation.observe(Some(terminal(4)));
        assert!(!navigation.can_go_forward());
        assert!(!navigation.step(false, |_| true));
    }
}
