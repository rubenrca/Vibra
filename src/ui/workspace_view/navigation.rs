//! Global navigation and the project-scoped workspace rail.

use gpui::{
    AnyElement, Context, Div, MouseButton, SharedString, Stateful, Window, div, prelude::*, px, svg,
};
use uuid::Uuid;

use crate::ui::theme::{colors, frame_hover, frame_selected, surface_tint};

use super::chrome::SIDEBAR_ROW_INSET;

use super::{
    ContextMenuKind, PaletteMode, RightSidebarMode, WorkspaceSection, WorkspaceView,
    sidebar_tooltip,
};

pub(super) fn project_color(id: Uuid) -> gpui::Rgba {
    let palette = [
        colors().accent,
        colors().success,
        colors().warning,
        colors().folder,
        colors().git_deleted,
    ];
    palette[id.as_bytes()[0] as usize % palette.len()]
}

/// One navigation row: 32 px, 15 px icon, 13 px label.
pub(super) fn rail_row(id: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id.into())
        .h(px(32.0))
        .w_full()
        .flex_none()
        .px(px(8.0))
        .rounded(px(super::SIDEBAR_ROW_RADIUS))
        .flex()
        .items_center()
        .gap(px(10.0))
        .cursor_pointer()
        .text_size(px(13.0))
        .font_weight(gpui::FontWeight::NORMAL)
        .text_color(colors().muted)
        .hover(|row| row.bg(frame_hover()).text_color(colors().foreground))
}

/// System-font keycaps keep macOS modifier symbols legible and evenly spaced.
pub(super) fn keycap(label: &'static str) -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(3.0))
        .font_family(".SystemUIFont")
        .text_size(px(11.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(colors().muted)
        .children(label.chars().map(|key| {
            div()
                .w(px(16.0))
                .h(px(18.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(3.0))
                .border_1()
                .border_color(gpui::Hsla::from(colors().foreground).opacity(0.32))
                .bg(gpui::Hsla::from(colors().foreground).opacity(0.04))
                .child(key.to_string())
        }))
}

/// Sidebar section heading with room for a trailing action.
/// A section's title. It is the only separator in the sidebar, so it keeps
/// clear room above it.
pub(super) fn section_label(label: &'static str) -> Div {
    div()
        .h(px(28.0))
        .mt(px(14.0))
        .pl(px(8.0))
        .pr(px(4.0))
        .flex()
        .items_center()
        .child(
            div()
                .flex_1()
                .text_size(px(12.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(colors().subtle)
                .child(label),
        )
}

impl WorkspaceView {
    pub(super) fn select_section(
        &mut self,
        section: WorkspaceSection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.record_navigation(cx);
        let entering_inbox =
            section == WorkspaceSection::Inbox && self.workspace_section != WorkspaceSection::Inbox;
        if section == WorkspaceSection::Settings && self.workspace_section != section {
            self.theme_query.clear();
            crate::ui::theme::refresh_user_themes();
        }
        self.usage.open = false;
        self.workspace_section = section;
        if section == WorkspaceSection::Inbox {
            // The tab belongs to the space Repository was opened from.
            if let Some(project_id) = self.snapshot.selected_project_id {
                self.repository_tabs.insert(project_id);
            }
            if entering_inbox {
                self.scope_inbox_to_active_project();
            }
            self.ensure_inbox_loaded(cx);
            self.sync_inbox_selection(cx);
            if self.settings.inbox.tab == crate::domain::work_items::RepositoryTab::History {
                self.open_history(false, cx);
            }
        }
        self.context_menu = None;
        self.ide_menu_open = false;
        self.pending_focus_session = None;
        self.sync_terminal_surface_visibility(cx);
        self.sync_git_panel_visibility(cx);
        self.sync_files_watcher(cx);
        self.focus_selected_terminal(window, cx);
        cx.notify();
    }

    pub(super) fn set_workspace_mode(&mut self, mode: RightSidebarMode, cx: &mut Context<Self>) {
        self.workspace_section = WorkspaceSection::Workspace;
        self.right_sidebar_mode = mode;
        self.set_right_sidebar_visible(true, true, cx);
        self.sync_terminal_surface_visibility(cx);
        self.sync_git_panel_visibility(cx);
        self.sync_files_watcher(cx);
        match mode {
            RightSidebarMode::Files => self.refresh_project_files(cx),
            RightSidebarMode::Diff => {
                self.sync_diff_root(cx);
                self.diff_view.update(cx, |diff, cx| diff.refresh_now(cx));
            }
        }
        cx.notify();
    }

    pub(super) fn global_sidebar_content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let (pinned, projects): (Vec<_>, Vec<_>) = self
            .snapshot
            .projects
            .iter()
            .map(|project| (project.id, project.name.clone()))
            .partition(|(id, _)| self.settings.pinned_project_ids.contains(id));
        let nav_items = [(
            WorkspaceSection::Inbox,
            "Repository",
            "chrome-icons/repository.svg",
        )];
        let space_count = self.snapshot.projects.len();
        div()
            .size_full()
            .flex()
            .flex_col()
            // No dividers: each section's title, with room above it,
            // separates it from the one before.
            .child(
                div()
                    .px(px(SIDEBAR_ROW_INSET))
                    .pt(px(10.0))
                    .pb(px(4.0))
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(
                        div()
                            .id("global-search")
                            .group("global-search")
                            .h(px(32.0))
                            .mb(px(8.0))
                            .px(px(8.0))
                            .rounded(px(8.0))
                            .border_1()
                            .border_color(colors().border_subtle)
                            .bg(surface_tint(colors().panel, colors().sidebar))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .cursor_pointer()
                            .text_size(px(13.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(colors().subtle)
                            .hover(|field| field.bg(frame_hover()).text_color(colors().muted))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open_palette(PaletteMode::Commands, cx)
                            }))
                            .child(
                                svg()
                                    .path("chrome-icons/search.svg")
                                    .size(px(16.0))
                                    .flex_none()
                                    .text_color(colors().muted)
                                    .group_hover("global-search", |icon| {
                                        icon.text_color(colors().foreground)
                                    }),
                            )
                            .child(div().flex_1().min_w(px(0.0)).truncate().child("Search"))
                            .child(keycap("⇧⌘P")),
                    )
                    .children(nav_items.into_iter().map(|(section, label, icon)| {
                        let selected = self.workspace_section == section;
                        rail_row(SharedString::from(format!("nav-{label}")))
                            .when(selected, |row| {
                                row.bg(frame_selected())
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(colors().foreground)
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_section(section, window, cx)
                            }))
                            .child(svg().path(icon).size(px(15.0)).flex_none().text_color(
                                if selected {
                                    colors().foreground
                                } else {
                                    colors().subtle
                                },
                            ))
                            .child(div().flex_1().min_w(px(0.0)).truncate().child(label))
                    })),
            )
            // Spaces above, Agents below; dragging the edge above Agents'
            // title resizes it.
            .child(
                div()
                    .id("sidebar-body")
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .flex_col()
                    .on_drag_move(cx.listener(Self::resize_agents_panel))
                    .child(
                        div()
                            .id("global-project-list")
                            .map(|list| self.project_list_drop_area(list, cx))
                            .flex_1()
                            .min_h(px(0.0))
                            .overflow_y_scroll()
                            .px(px(SIDEBAR_ROW_INSET))
                            .pb_2()
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                                    let y = event.position.y.into();
                                    this.open_sidebar_context_menu(
                                        ContextMenuKind::SidebarBackground,
                                        y,
                                        y,
                                        cx,
                                    );
                                    cx.stop_propagation();
                                }),
                            )
                            .when(!pinned.is_empty(), |list| {
                                list.child(section_label("Pinned"))
                                    .children(self.project_rows(pinned, cx))
                            })
                            .child(
                                section_label("Spaces").child(
                                    div()
                                        .id("add-global-project")
                                        .group("add-global-project")
                                        .size(px(22.0))
                                        .flex_none()
                                        .rounded(px(5.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .hover(|button| button.bg(frame_hover()))
                                        .tooltip(|_, cx| sidebar_tooltip("New space · ⌘N", cx))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.new_space(window, cx)
                                        }))
                                        .child(
                                            svg()
                                                .path("chrome-icons/plus.svg")
                                                .size(px(15.0))
                                                .flex_none()
                                                .text_color(colors().muted)
                                                .group_hover("add-global-project", |icon| {
                                                    icon.text_color(colors().foreground)
                                                }),
                                        ),
                                ),
                            )
                            .children(self.project_rows(projects, cx))
                            .when(space_count > 1, |list| {
                                list.child(self.project_list_end_target(cx))
                            })
                            .when(space_count == 0, |list| {
                                list.child(
                                    div()
                                        .id("global-empty-projects")
                                        .mt_1()
                                        .px_3()
                                        .py_2()
                                        .rounded(px(8.0))
                                        .border_1()
                                        .border_dashed()
                                        .border_color(colors().border_subtle)
                                        .flex()
                                        .flex_col()
                                        .cursor_pointer()
                                        .hover(|card| card.bg(frame_hover()))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.new_space(window, cx)
                                        }))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .text_size(px(12.5))
                                                        .font_weight(gpui::FontWeight::MEDIUM)
                                                        .text_color(colors().foreground)
                                                        .child("Open a space"),
                                                )
                                                .child(keycap("⌘N")),
                                        ),
                                )
                            }),
                    )
                    .child(self.agents_section(cx)),
            )
            .into_any_element()
    }

    pub(super) fn global_section_content(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self.workspace_section {
            WorkspaceSection::Inbox => self.inbox_content(cx),
            WorkspaceSection::Settings => self.settings_content(window, cx),
            WorkspaceSection::Workspace => unreachable!("workspace renders its terminal canvas"),
        }
    }
}
