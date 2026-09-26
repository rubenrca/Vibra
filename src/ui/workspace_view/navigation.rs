//! Global navigation and the project-scoped workspace rail.

use gpui::{
    AnyElement, Context, Div, MouseButton, SharedString, Stateful, Window, div, prelude::*, px, svg,
};
use uuid::Uuid;

use crate::domain::workspace::SidebarEntry;
use crate::ui::theme::{colors, surface, surface_tint};

use super::chrome::SIDEBAR_ROW_INSET;

use super::{
    ContextMenuKind, PaletteMode, ProjectDrag, RightSidebarMode, WorkspaceSection, WorkspaceView,
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

/// One navigation row: 30 px, 15 px icon, 13 px label.
fn rail_row(id: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id.into())
        .h(px(30.0))
        .w_full()
        .flex_none()
        .px(px(8.0))
        .rounded(px(7.0))
        .flex()
        .items_center()
        .gap(px(10.0))
        .cursor_pointer()
        .text_size(px(13.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(colors().muted)
        .hover(|row| {
            row.bg(surface_tint(colors().hover, colors().sidebar))
                .text_color(colors().foreground)
        })
}

/// A shortcut drawn as a small keycap.
pub(super) fn keycap(label: &'static str) -> Div {
    div()
        .flex_none()
        .h(px(18.0))
        .px(px(5.0))
        .rounded(px(4.0))
        .flex()
        .items_center()
        .border_1()
        .border_color(colors().border_subtle)
        .font_family(crate::ui::theme::MONO_FONT)
        .text_size(px(10.0))
        .text_color(colors().subtle)
        .child(label)
}

/// Small uppercase section label with an action revealed on hover.
fn section_label(group: &'static str, label: &'static str) -> Div {
    div()
        .group(group)
        .h(px(28.0))
        .mt(px(10.0))
        .pl(px(8.0))
        .pr(px(4.0))
        .flex()
        .items_center()
        .child(
            div()
                .flex_1()
                .text_size(px(11.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
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
        self.leave_library_section(section);
        self.workspace_section = section;
        if section == WorkspaceSection::Workspace
            && let Some(session) = self.snapshot.selected_session()
        {
            self.inbox.mark_pane_read(session.id);
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
            RightSidebarMode::Info => {}
        }
        cx.notify();
    }

    pub(super) fn global_sidebar_content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let projects: Vec<_> = self
            .snapshot
            .sidebar_entries()
            .into_iter()
            .filter(|entry| matches!(entry, SidebarEntry::Project { .. }))
            .collect();
        let (pinned, projects): (Vec<_>, Vec<_>) = projects.into_iter().partition(|entry| {
            matches!(entry, SidebarEntry::Project { id, .. } if self.settings.pinned_project_ids.contains(id))
        });
        let nav_items = [
            (WorkspaceSection::Inbox, "Inbox", "chrome-icons/inbox.svg"),
            (WorkspaceSection::Notes, "Notes", "chrome-icons/notes.svg"),
            (
                WorkspaceSection::Automations,
                "Automations",
                "chrome-icons/automations.svg",
            ),
        ];
        let has_projects = !self.snapshot.projects.is_empty();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(SIDEBAR_ROW_INSET))
                    .pt(px(6.0))
                    .pb(px(4.0))
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .id("global-search")
                            .h(px(30.0))
                            .mb(px(8.0))
                            .px(px(8.0))
                            .rounded(px(7.0))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .cursor_pointer()
                            .bg(surface_tint(colors().elevated, colors().sidebar))
                            .text_size(px(13.0))
                            .text_color(colors().subtle)
                            .hover(|field| {
                                field
                                    .bg(surface_tint(colors().hover, colors().sidebar))
                                    .text_color(colors().muted)
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open_palette(PaletteMode::Commands, cx)
                            }))
                            .child(svg().path("chrome-icons/search.svg").size(px(14.0)))
                            .child(div().flex_1().child("Search"))
                            .child(keycap("⇧⌘P")),
                    )
                    .children(nav_items.into_iter().map(|(section, label, icon)| {
                        let selected = self.workspace_section == section;
                        rail_row(SharedString::from(format!("nav-{label}")))
                            .when(selected, |row| {
                                row.bg(surface_tint(colors().selection, colors().sidebar))
                                    .text_color(colors().foreground)
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_section(section, window, cx)
                            }))
                            .child(svg().path(icon).size(px(15.0)).flex_none().text_color(
                                if selected {
                                    colors().accent
                                } else {
                                    colors().subtle
                                },
                            ))
                            .child(div().flex_1().child(label))
                            .when(section == WorkspaceSection::Inbox, |row| {
                                row.children(self.inbox_unread_badge())
                            })
                    })),
            )
            .child(
                div()
                    .id("global-project-list")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .px(px(SIDEBAR_ROW_INSET))
                    .pb_2()
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                            this.open_context_menu(
                                ContextMenuKind::SidebarBackground,
                                event.position.x.into(),
                                event.position.y.into(),
                                cx,
                            );
                            cx.stop_propagation();
                        }),
                    )
                    .when(!pinned.is_empty(), |list| {
                        list.child(section_label("section-pinned", "PINNED"))
                            .children(
                                pinned
                                    .into_iter()
                                    .map(|entry| self.project_sidebar_header(entry, cx)),
                            )
                    })
                    .child(
                        section_label("section-projects", "PROJECTS").child(
                            div()
                                .id("add-global-project")
                                .size(px(22.0))
                                .rounded(px(5.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .text_color(colors().subtle)
                                .opacity(if has_projects { 0.0 } else { 1.0 })
                                .group_hover("section-projects", |button| button.opacity(1.0))
                                .hover(|button| {
                                    button.bg(colors().hover).text_color(colors().foreground)
                                })
                                .tooltip(|_, cx| sidebar_tooltip("Agregar proyecto · ⇧⌘O", cx))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.choose_project_folder(None, false, window, cx)
                                }))
                                .child(svg().path("chrome-icons/plus.svg").size(px(13.0))),
                        ),
                    )
                    .children(
                        projects
                            .into_iter()
                            .map(|entry| self.project_sidebar_header(entry, cx)),
                    )
                    .when(self.snapshot.projects.len() > 1, |list| {
                        list.child(
                            div()
                                .id("global-project-drop-end")
                                .h(px(28.0))
                                .w_full()
                                .can_drop(|value, _, _| {
                                    value.downcast_ref::<ProjectDrag>().is_some()
                                })
                                .drag_over::<ProjectDrag>(|style, _, _, _| {
                                    style.border_t_2().border_color(colors().accent)
                                })
                                .on_drop(cx.listener(|this, drag: &ProjectDrag, _, cx| {
                                    if this.snapshot.move_project(drag.project_id, None) {
                                        this.persist(cx);
                                    }
                                })),
                        )
                    })
                    .when(!has_projects, |list| {
                        list.child(
                            div()
                                .id("global-empty-projects")
                                .mt_1()
                                .p_3()
                                .rounded(px(8.0))
                                .border_1()
                                .border_dashed()
                                .border_color(colors().border_subtle)
                                .flex()
                                .flex_col()
                                .gap(px(8.0))
                                .cursor_pointer()
                                .hover(|card| card.bg(surface_tint(colors().hover, colors().sidebar)))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.choose_project_folder(None, false, window, cx)
                                }))
                                .child(
                                    div()
                                        .text_size(px(12.5))
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .text_color(colors().foreground)
                                        .child("Agrega tu primer proyecto"),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .line_height(px(17.0))
                                        .text_color(colors().subtle)
                                        .child("Elige una carpeta; sus sesiones, archivos y cambios quedan juntos."),
                                ),
                        )
                    }),
            )
            .child(
                div()
                    .px(px(SIDEBAR_ROW_INSET))
                    .py(px(8.0))
                    .border_t_1()
                    .border_color(colors().border_subtle)
                    .child(
                        rail_row("global-settings")
                            .on_click(cx.listener(|this, _, _, cx| this.open_settings(cx)))
                            .child(
                                svg()
                                    .path("chrome-icons/settings.svg")
                                    .size(px(15.0))
                                    .flex_none()
                                    .text_color(colors().subtle),
                            )
                            .child(div().flex_1().child("Settings"))
                            .child(keycap("⌘,")),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn global_section_content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        match self.workspace_section {
            WorkspaceSection::Inbox => self.inbox_content(cx),
            WorkspaceSection::Notes => self.notes_content(cx),
            WorkspaceSection::Automations => self.automations_content(cx),
            WorkspaceSection::Workspace => unreachable!("workspace renders its terminal canvas"),
        }
    }

    /// Ends in-progress editing when the user leaves Notes or Automations.
    pub(super) fn leave_library_section(&mut self, next: WorkspaceSection) {
        if next != WorkspaceSection::Notes {
            self.note_editing = false;
            if let Some(id) = self.selected_note_id
                && self.library.discard_blank_note(id)
            {
                self.selected_note_id = None;
            }
        }
        if next != WorkspaceSection::Automations {
            self.automation_form = None;
        }
    }
}

/// Page chrome shared by Inbox, Notes and Automations.
pub(super) fn section_frame(title: &str, actions: Vec<AnyElement>, body: AnyElement) -> AnyElement {
    div()
        .id("global-section")
        .flex_1()
        .min_w(px(0.0))
        .h_full()
        .flex()
        .flex_col()
        .bg(surface(colors().background))
        .child(
            div()
                .h(px(58.0))
                .flex_none()
                .px_6()
                .flex()
                .items_center()
                .gap_2()
                .border_b_1()
                .border_color(colors().border_subtle)
                .child(
                    div()
                        .flex_1()
                        .text_size(px(16.0))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(title.to_owned()),
                )
                .children(actions),
        )
        .child(
            div()
                .id("global-section-body")
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .px_6()
                .py_4()
                .flex()
                .flex_col()
                .items_center()
                .child(body),
        )
        .into_any_element()
}

pub(super) fn section_button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    primary: bool,
) -> Stateful<Div> {
    let label = label.into();
    div()
        .id(id.into())
        .h(px(28.0))
        .flex_none()
        .px_3()
        .rounded(px(6.0))
        .flex()
        .items_center()
        .cursor_pointer()
        .text_size(px(12.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .when(primary, |button| {
            button
                .bg(colors().accent)
                .text_color(colors().background)
                .hover(|button| button.opacity(0.9))
        })
        .when(!primary, |button| {
            button
                .border_1()
                .border_color(colors().border_subtle)
                .text_color(colors().muted)
                .hover(|button| {
                    button
                        .bg(surface_tint(colors().hover, colors().background))
                        .text_color(colors().foreground)
                })
        })
        .child(label)
}

pub(super) fn section_heading(label: impl Into<SharedString>) -> Div {
    div()
        .h(px(30.0))
        .px_3()
        .flex()
        .items_center()
        .text_size(px(12.0))
        .text_color(colors().subtle)
        .child(label.into())
}

pub(super) fn section_empty_state(
    icon: &'static str,
    heading: &'static str,
    description: &'static str,
) -> AnyElement {
    div()
        .w_full()
        .py_6()
        .flex()
        .flex_col()
        .items_center()
        .gap_2()
        .text_center()
        .child(
            div()
                .size(px(40.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(10.0))
                .bg(surface_tint(colors().elevated, colors().background))
                .child(svg().path(icon).size(px(20.0)).text_color(colors().muted)),
        )
        .child(
            div()
                .text_size(px(14.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(heading),
        )
        .child(
            div()
                .max_w(px(420.0))
                .text_size(px(12.5))
                .line_height(px(19.0))
                .text_color(colors().muted)
                .child(description),
        )
        .into_any_element()
}
