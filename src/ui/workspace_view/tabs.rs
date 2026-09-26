//! The review as a tab beside the terminal tabs, back/forward navigation
//! between tabs and sessions, and the resizable terminal/review split.

use gpui::{
    AnyElement, Context, DragMoveEvent, MouseButton, Window, div, prelude::*, px, relative, svg,
};
use uuid::Uuid;

use crate::infrastructure::settings::{MAX_REVIEW_SPLIT, MIN_REVIEW_SPLIT};
use crate::ui::theme::{colors, surface_tint};
use crate::{GoToTab, NavigateBack, NavigateForward};

use super::panes::{TAB_HEIGHT, TAB_MAX_WIDTH, TAB_RADIUS, TAB_TEXT_SIZE, tab_shortcut};
use super::titlebar::{TITLEBAR_BUTTON_GAP, titlebar_button, titlebar_icon};
use super::{WorkspaceSection, WorkspaceView, sidebar_tooltip};

const MAX_NAVIGATION_HISTORY: usize = 50;

/// A place the user can go back to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NavLocation {
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
    /// The review tab is open and in front.
    pub(super) fn review_visible(&self, cx: &gpui::App) -> bool {
        self.workspace_section == WorkspaceSection::Workspace
            && self.review_tab_active
            && self.diff_view.read(cx).review_expanded()
    }

    /// The review fills the center and the terminal is hidden.
    pub(super) fn review_covers_terminal(&self, cx: &gpui::App) -> bool {
        self.review_visible(cx) && self.diff_view.read(cx).review_focused()
    }

    pub(super) fn current_location(&self, cx: &gpui::App) -> Option<NavLocation> {
        if self.workspace_section != WorkspaceSection::Workspace {
            return None;
        }
        let project_id = self.snapshot.selected_project_id?;
        let workspace = self.snapshot.selected_workspace()?;
        if self.review_visible(cx) {
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
        self.leave_library_section(WorkspaceSection::Workspace);
        self.workspace_section = WorkspaceSection::Workspace;
        self.review_tab_active = true;
        self.sync_terminal_surface_visibility(cx);
        self.sync_git_panel_visibility(cx);
        self.diff_view.read(cx).focus_review(window);
        cx.notify();
    }

    /// Brings a terminal tab forward; an open review keeps its tab.
    pub(super) fn show_terminal_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_library_section(WorkspaceSection::Workspace);
        self.workspace_section = WorkspaceSection::Workspace;
        self.review_tab_active = false;
        self.sync_terminal_surface_visibility(cx);
        self.sync_diff_root(cx);
        self.sync_git_panel_visibility(cx);
        self.refresh_project_files(cx);
        self.persist(cx);
        self.focus_selected_terminal(window, cx);
    }

    fn apply_location(
        &mut self,
        location: NavLocation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let current_workspace = self.snapshot.selected_workspace().map(|item| item.id);
        match location {
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
                    .projects
                    .iter()
                    .find(|project| project.id == project_id)
                    .and_then(|project| project.workspaces.as_deref())
                    .and_then(|workspaces| workspaces.iter().find(|item| item.id == workspace_id))
                    .is_some_and(|workspace| workspace.tabs.iter().any(|tab| tab.id == tab_id));
                if !exists {
                    return false;
                }
                let same_workspace = self.snapshot.selected_project_id == Some(project_id)
                    && current_workspace == Some(workspace_id);
                self.snapshot.select_workspace(project_id, workspace_id);
                self.snapshot.select_tab(tab_id);
                if same_workspace {
                    self.show_terminal_tab(window, cx);
                } else {
                    self.apply_workspace_selection_change(window, cx);
                }
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

    /// `⌘1`–`⌘8` count the review tab after the terminal tabs; `⌘9` is the last tab.
    pub(super) fn go_to_tab(
        &mut self,
        action: &GoToTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.palette_mode.is_some() || self.settings_open || self.rename_prompt.is_some() {
            return;
        }
        let terminal_tabs = self
            .snapshot
            .selected_workspace()
            .map_or(0, |workspace| workspace.tabs.len());
        if self.diff_view.read(cx).review_expanded()
            && (action.index == 9 || action.index == terminal_tabs + 1)
        {
            self.activate_review_tab(window, cx);
            return;
        }
        let changed = self.snapshot.select_tab_number(action.index);
        if changed || self.review_tab_active {
            self.show_terminal_tab(window, cx);
        }
        self.focus_selected_terminal(window, cx);
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
                    "Atrás · ⌃⌘←",
                    self.navigation.can_go_back(),
                )
                .on_click(cx.listener(|this, _, window, cx| this.navigate(true, window, cx))),
            )
            .child(
                button(
                    "navigate-forward",
                    "chrome-icons/chevron-right.svg",
                    "Adelante · ⌃⌘→",
                    self.navigation.can_go_forward(),
                )
                .on_click(cx.listener(|this, _, window, cx| this.navigate(false, window, cx))),
            )
            .into_any_element()
    }

    /// The review, drawn like the terminal tabs so switching is one click.
    pub(super) fn review_tab(&self, number: usize, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.review_visible(cx);
        let diff = self.diff_view.read(cx);
        let title = diff.review_title();
        let icon = if diff.review_is_commit() {
            "chrome-icons/git-commit.svg"
        } else {
            "chrome-icons/diff-unified.svg"
        };
        let shortcut = (number <= 8).then(|| format!("⌘{number}"));
        div()
            .id("review-tab")
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
            .cursor_pointer()
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
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, window, cx| this.activate_review_tab(window, cx)))
            .child(
                svg()
                    .path(icon)
                    .size(px(15.0))
                    .flex_none()
                    .text_color(if selected {
                        colors().accent
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
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(title),
            )
            .when_some(shortcut, |tab, shortcut| tab.child(tab_shortcut(shortcut)))
            .child(
                div()
                    .id("close-review-tab")
                    .size(px(20.0))
                    .flex_none()
                    .rounded(px(5.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .text_color(colors().subtle)
                    .hover(|button| button.bg(colors().hover).text_color(colors().foreground))
                    .tooltip(|_, cx| sidebar_tooltip("Cerrar revisión · ⌘W", cx))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, window, cx| {
                        cx.stop_propagation();
                        this.close_review(window, cx);
                    }))
                    .child(svg().path("chrome-icons/close.svg").size(px(12.0))),
            )
            .into_any_element()
    }

    /// Terminal and review side by side, split by a draggable divider.
    pub(super) fn review_split(
        &mut self,
        terminal: AnyElement,
        review: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ratio = self.settings.review_split;
        div()
            .id("review-split")
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .flex()
            .on_drag_move(cx.listener(Self::on_review_split_move))
            .child(
                div()
                    .h_full()
                    .flex_none()
                    .w(relative(ratio))
                    .min_w(px(240.0))
                    .flex()
                    .child(terminal),
            )
            .child(
                div()
                    .id("review-split-divider")
                    .h_full()
                    .w(px(5.0))
                    .flex_none()
                    .cursor_ew_resize()
                    .flex()
                    .justify_center()
                    .hover(|divider| divider.bg(surface_tint(colors().hover, colors().panel)))
                    .on_drag(ReviewSplitDrag, |_, _, _, cx| {
                        cx.new(|_| ReviewSplitDragView)
                    })
                    .child(div().w(px(1.0)).h_full().bg(colors().border_subtle)),
            )
            .child(
                div()
                    .h_full()
                    .flex_1()
                    .min_w(px(280.0))
                    .flex()
                    .child(review),
            )
            .into_any_element()
    }

    fn on_review_split_move(
        &mut self,
        event: &DragMoveEvent<ReviewSplitDrag>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let x: f32 = event.event.position.x.into();
        let left: f32 = event.bounds.left().into();
        let width: f32 = event.bounds.size.width.into();
        if width <= 1.0 {
            return;
        }
        let ratio = ((x - left) / width).clamp(MIN_REVIEW_SPLIT, MAX_REVIEW_SPLIT);
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
