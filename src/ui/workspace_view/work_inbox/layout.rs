//! Native translation of MonoCode InboxView / InboxFiltersMenu (c576783).

mod checks;
mod code;
mod commits;
mod item_detail;
mod local;
mod merge;
mod repository;

use super::*;
use crate::domain::inbox::relative_time;
use crate::domain::work_items::{
    CheckRollup, InboxPreferences, InboxTime, PullSummary, toggle_value,
};
use crate::infrastructure::clock::unix_now;
use crate::ui::menu::{MenuRow, menu_heading, menu_panel, menu_separator};
use crate::ui::theme::{surface, surface_tint};
use crate::ui::workspace_view::drag::SidebarResizeDragView;
use crate::ui::workspace_view::navigation::project_color;
use crate::ui::workspace_view::sidebar_tooltip;
use gpui::{ClickEvent, DragMoveEvent, MouseButton, Rgba, svg};

#[derive(Clone)]
struct InboxListResize;

impl WorkspaceView {
    pub(in crate::ui::workspace_view) fn inbox_content(
        &self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.work_inbox.activity {
            return self.inbox_activity_content(cx);
        }
        let repository = self.settings.inbox.repository_view;
        let width = if self.settings.inbox.list_width.is_finite() {
            self.settings.inbox.list_width.clamp(240.0, 420.0)
        } else {
            280.0
        };
        let mut sidebar = div()
            .id("inbox-list-pane")
            .relative()
            .w(px(width))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(colors().border_subtle)
            .child(self.inbox_source_tabs(cx));
        sidebar = if repository {
            sidebar
                .child(self.repository_toolbar(cx))
                .child(self.repository_tree(cx))
        } else {
            sidebar
                .child(self.inbox_filter_bar(cx))
                .child(self.inbox_task_list(cx))
        };
        let sidebar = sidebar.child(
            div()
                .id("resize-inbox-list")
                .absolute()
                .top_0()
                .bottom_0()
                .right_0()
                .w(px(5.0))
                .cursor_ew_resize()
                .hover(|handle| handle.bg(colors().border_subtle))
                .on_drag(InboxListResize, |_, _, _, cx| {
                    cx.new(|_| SidebarResizeDragView)
                })
                .on_click(cx.listener(|this, event: &ClickEvent, _, cx| {
                    if event.click_count() == 2 {
                        this.settings.inbox.list_width = 280.0;
                        this.persist_settings(cx);
                    }
                })),
        );
        let title = if repository {
            self.repository_project()
                .and_then(|id| self.snapshot.project(id))
                .map(|project| format!("Repository · {}", project.name))
                .unwrap_or_else(|| "Repository".into())
        } else {
            self.work_inbox
                .filter
                .project
                .and_then(|id| {
                    self.snapshot
                        .projects
                        .iter()
                        .find(|project| project.id == id)
                })
                .map(|project| format!("Review · {}", project.name))
                .unwrap_or_else(|| "Review".into())
        };
        div()
            .id("work-inbox")
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(surface(colors().background))
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<InboxListResize>, _, cx| {
                    let width: f32 = (event.event.position.x - event.bounds.left()).into();
                    this.settings.inbox.list_width = width.clamp(240.0, 420.0);
                    this.persist_settings(cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.persist_settings(cx)),
            )
            .child(
                div()
                    .h(px(40.0))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(colors().border_subtle)
                    .child(
                        svg()
                            .path(if repository {
                                "chrome-icons/git-branch.svg"
                            } else {
                                "chrome-icons/inbox.svg"
                            })
                            .size(px(14.0))
                            .text_color(colors().subtle),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .text_size(px(13.0))
                            .child(title),
                    )
                    .child(
                        icon_button(
                            "inbox-shortcuts",
                            "chrome-icons/keyboard.svg",
                            "Keyboard shortcuts (?)",
                        )
                        .on_click(cx.listener(|this, event, _, cx| {
                            this.open_inbox_menu(InboxMenu::Shortcuts, event, cx)
                        })),
                    )
                    .child(
                        quiet_button("inbox-activity", "Activity").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.work_inbox.activity = true;
                                this.work_inbox.menu = None;
                                this.sync_repository_polling(cx);
                                this.sync_terminal_surface_visibility(cx);
                                cx.notify();
                            },
                        )),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .child(sidebar)
                    .child(if repository {
                        self.repository_detail(cx)
                    } else {
                        self.inbox_detail_panel(cx)
                    }),
            )
            .into_any_element()
    }

    /// Repository · GitHub · Linear, plus the connection menu.
    fn inbox_source_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let repository = self.settings.inbox.repository_view;
        let source = self.settings.inbox.source;
        let tab = |id: SharedString, icon: AnyElement, label: &'static str, selected: bool| {
            div()
                .id(id)
                .h(px(24.0))
                .flex_1()
                .min_w(px(0.0))
                .rounded(px(5.0))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(6.0))
                .cursor_pointer()
                .text_size(px(12.0))
                .text_color(if selected {
                    colors().foreground
                } else {
                    colors().muted
                })
                .when(selected, |tab| {
                    tab.bg(surface_tint(colors().selection, colors().background))
                })
                .hover(|tab| tab.bg(surface_tint(colors().hover, colors().background)))
                .child(icon)
                .child(div().truncate().child(label))
        };
        div()
            .h(px(36.0))
            .px_2()
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .border_b_1()
            .border_color(colors().border_subtle)
            .child(
                tab(
                    "inbox-tab-repository".into(),
                    svg()
                        .path("chrome-icons/git-branch.svg")
                        .size(px(14.0))
                        .flex_none()
                        .text_color(colors().muted)
                        .into_any_element(),
                    "Repository",
                    repository,
                )
                .tooltip(|_, cx| sidebar_tooltip("Branches, stashes, and worktrees (G)", cx))
                .on_click(cx.listener(|this, _, _, cx| this.show_repository_view(true, cx))),
            )
            .children(
                [WorkSource::GitHub, WorkSource::Linear]
                    .into_iter()
                    .filter(|provider| {
                        *provider == WorkSource::GitHub
                            || self.work_inbox.feed(*provider).connected == Some(true)
                    })
                    .map(|provider| {
                        tab(
                            SharedString::from(format!("inbox-tab-{provider:?}")),
                            provider_icon(provider, 14.0).into_any_element(),
                            provider.label(),
                            !repository && source == provider,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.show_repository_view(false, cx);
                            if this.settings.inbox.source != provider {
                                this.select_work_source(provider, cx);
                            }
                        }))
                    }),
            )
            .child(
                icon_button(
                    "inbox-connections",
                    "chrome-icons/plus.svg",
                    "Add connection",
                )
                .on_click(cx.listener(|this, event, _, cx| {
                    this.open_inbox_menu(InboxMenu::Connections, event, cx)
                })),
            )
    }

    fn inbox_filter_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let source = self.settings.inbox.source;
        let feed = self.work_inbox.feed(source);
        div()
            .h(px(36.0))
            .flex_none()
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .border_b_1()
            .border_color(colors().border_subtle)
            .child(
                div()
                    .id("inbox-filter-input")
                    .flex_1()
                    .min_w(px(0.0))
                    .h(px(28.0))
                    .px_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_text()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.focus_inbox_search(window, cx);
                    }))
                    .child(
                        svg()
                            .path("chrome-icons/search.svg")
                            .size(px(12.0))
                            .flex_none()
                            .text_color(colors().subtle),
                    )
                    .child(
                        div()
                            .min_w(px(0.0))
                            .truncate()
                            .text_size(px(12.0))
                            .text_color(if self.work_inbox.search_editing {
                                colors().foreground
                            } else {
                                colors().muted
                            })
                            .child(if self.work_inbox.filter.query.is_empty() {
                                "Filter Review".into()
                            } else {
                                self.work_inbox.filter.query.clone()
                            }),
                    ),
            )
            .child(
                icon_button("inbox-filter-menu", "chrome-icons/filter.svg", "Filters")
                    .when(
                        self.settings.inbox.filters_active()
                            || self.work_inbox.filter.project.is_some(),
                        |button| button.bg(surface_tint(colors().selection, colors().background)),
                    )
                    .on_click(cx.listener(|this, event, _, cx| {
                        this.open_inbox_menu(InboxMenu::Filters, event, cx)
                    })),
            )
            .child(
                icon_button(
                    "inbox-read-all",
                    "chrome-icons/check-all.svg",
                    "Mark all as read",
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    for item in &this.work_inbox.feed(source).items {
                        this.settings.inbox.mark_seen(item);
                    }
                    this.persist_settings(cx);
                })),
            )
            .child(
                icon_button(
                    "inbox-refresh",
                    "chrome-icons/refresh.svg",
                    if feed.loading {
                        "Refreshing…"
                    } else {
                        "Refresh (R)"
                    },
                )
                .when(feed.loading, |button| button.opacity(0.4))
                .on_click(cx.listener(move |this, _, _, cx| this.refresh_inbox(cx))),
            )
    }

    fn inbox_task_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let feed = self.work_inbox.feed(self.settings.inbox.source);
        let groups = self.grouped_inbox_items();
        let mut list = div()
            .id("inbox-list-scroll")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .track_scroll(&self.work_inbox.list_scroll)
            .p(px(6.0))
            .flex()
            .flex_col()
            .gap(px(2.0));
        if let Some(error) = &feed.error {
            list = list.child(message(error, true));
        }
        if feed.loading && feed.items.is_empty() {
            list = list.child(message("Loading Review…", false));
        } else if groups.is_empty() {
            list = list.child(message(
                if feed.connected == Some(false) {
                    "Add a connection to get started."
                } else if self.settings.inbox.filters_active()
                    || self.work_inbox.filter.project.is_some()
                    || !self.work_inbox.filter.query.is_empty()
                {
                    "No tasks match these filters."
                } else {
                    "No issues or pull requests in your projects."
                },
                false,
            ));
        }
        // One group needs no heading: the list reads as before.
        let grouped = groups.len() > 1;
        for (relation, items) in groups {
            let collapsed = grouped && self.work_inbox.collapsed_groups.contains(&relation);
            if grouped {
                list = list.child(
                    div()
                        .id(SharedString::from(format!("inbox-group-{relation:?}")))
                        .h(px(26.0))
                        .px(px(6.0))
                        .mt(px(4.0))
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .rounded(px(5.0))
                        .cursor_pointer()
                        .hover(|row| row.bg(surface_tint(colors().hover, colors().background)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.work_inbox.collapsed_groups.remove(&relation) {
                                this.work_inbox.collapsed_groups.insert(relation);
                            }
                            this.sync_inbox_selection(cx);
                            cx.notify();
                        }))
                        .child(
                            svg()
                                .path(if collapsed {
                                    "chrome-icons/chevron-right.svg"
                                } else {
                                    "chrome-icons/chevron-down.svg"
                                })
                                .size(px(12.0))
                                .text_color(colors().subtle),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(10.5))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(colors().muted)
                                .child(relation.label().to_uppercase()),
                        )
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(colors().subtle)
                                .child(items.len().to_string()),
                        ),
                );
            }
            if collapsed {
                continue;
            }
            for item in items {
                list = list.child(self.inbox_task_card(item, cx));
            }
        }
        if feed.truncated {
            list = list.child(message(
                "The result limit was reached. Adjust filters to find other tasks.",
                false,
            ));
        }
        list
    }

    pub(super) fn refresh_inbox(&mut self, cx: &mut Context<Self>) {
        if self.settings.inbox.repository_view {
            if let Some(root) = self.repository_root() {
                self.refresh_local_repo(root, true, cx);
            }
            self.refresh_diff_views(cx);
            return;
        }
        let source = self.settings.inbox.source;
        self.refresh_work_source(source, true, cx);
        self.load_inbox_detail(true, cx);
        self.refresh_inbox_repos(true, cx);
    }

    pub(super) fn focus_inbox_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.work_inbox.search_editing = true;
        self.work_inbox.comment_editing = false;
        self.work_inbox.composer_editing = false;
        self.work_inbox.repository.stash_editing = false;
        if let Some(url) = self.work_inbox.selected.clone() {
            self.work_inbox
                .details
                .entry(url)
                .or_default()
                .review_draft
                .editing = false;
        }
        self.focus_handle.focus(window);
        cx.notify();
    }

    fn inbox_task_card(&self, item: &WorkItem, cx: &mut Context<Self>) -> AnyElement {
        let url = item.url.clone();
        let selected = self.work_inbox.selected.as_ref() == Some(&url);
        let (status_icon, color) = status_mark(item);
        let linked = self
            .settings
            .inbox
            .linked_sessions
            .get(&url)
            .is_some_and(|panes| panes.iter().any(|pane| self.terminals.contains_key(pane)));
        let (_, local) = self.local_pull(item);
        div()
            .id(SharedString::from(format!("inbox-item-{url}")))
            .px(px(10.0))
            .py_2()
            .rounded(px(6.0))
            .flex()
            .flex_col()
            .gap_1()
            .cursor_pointer()
            .when(selected, |row| {
                row.bg(surface_tint(colors().selection, colors().background))
            })
            .hover(|row| row.bg(surface_tint(colors().hover, colors().background)))
            .tooltip({
                let title = item.title.clone();
                move |_, cx| sidebar_tooltip(title.clone(), cx)
            })
            .on_click(cx.listener(move |this, _, _, cx| this.select_work_item(&url, cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.0))
                    .text_size(px(11.0))
                    .text_color(colors().subtle)
                    .child(provider_icon(item.source, 14.0))
                    .child(
                        svg()
                            .path(status_icon)
                            .size(px(12.0))
                            .flex_none()
                            .text_color(color),
                    )
                    .child(div().flex_1().min_w(px(0.0)).truncate().child(format!(
                        "{} · {}",
                        if item.kind == WorkKind::PullRequest {
                            "Pull request"
                        } else {
                            "Issue"
                        },
                        item.reference
                    )))
                    .when(linked, |row| {
                        row.child(
                            svg()
                                .path("chrome-icons/comment.svg")
                                .size(px(12.0))
                                .text_color(colors().accent),
                        )
                    })
                    .child(short_time(item.updated_at)),
            )
            .child(
                div()
                    .text_size(px(13.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .truncate()
                    .child(item.title.clone()),
            )
            .when_some(item.pull.as_ref(), |card, pull| {
                card.child(pull_facts(pull, local))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_size(px(11.0))
                    .text_color(colors().subtle)
                    .when_some(item.project_id, |row, id| {
                        row.child(
                            div()
                                .size(px(12.0))
                                .flex_none()
                                .rounded(px(3.0))
                                .bg(project_color(id)),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .child(item.repository.clone()),
                    )
                    .children(item.labels.iter().take(2).map(|label| label_chip(label))),
            )
            .into_any_element()
    }

    pub(super) fn open_inbox_menu(
        &mut self,
        menu: InboxMenu,
        event: &ClickEvent,
        cx: &mut Context<Self>,
    ) {
        self.work_inbox.menu = if self.work_inbox.menu == Some(menu) {
            None
        } else {
            Some(menu)
        };
        self.work_inbox.menu_position = (event.position().x.into(), event.position().y.into());
        self.work_inbox.search_editing = false;
        self.work_inbox.comment_editing = false;
        self.work_inbox.composer_editing = false;
        cx.notify();
    }

    pub(in crate::ui::workspace_view) fn inbox_popover(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.workspace_section != WorkspaceSection::Inbox {
            return None;
        }
        let menu = self.work_inbox.menu?;
        let mut content = div()
            .id("inbox-menu-scroll")
            .max_h(px(480.0))
            .overflow_y_scroll();
        match menu {
            InboxMenu::Filters => {
                let preferences = &self.settings.inbox;
                content = content
                    .child(
                        MenuRow::new("Assigned to me")
                            .checked(preferences.assigned_to_me)
                            .render("inbox-filter-assigned")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.settings.inbox.assigned_to_me =
                                    !this.settings.inbox.assigned_to_me;
                                this.inbox_filters_changed(cx);
                            })),
                    )
                    .child(menu_heading("STATUS"))
                    .child(
                        MenuRow::new("All statuses")
                            .checked(
                                WorkStatus::ALL
                                    .iter()
                                    .all(|status| preferences.status_selected(*status)),
                            )
                            .render("inbox-all-statuses")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.settings.inbox.status = None;
                                this.settings.inbox.statuses = WorkStatus::ALL.to_vec();
                                this.inbox_filters_changed(cx);
                            })),
                    );
                for status in [
                    WorkStatus::Open,
                    WorkStatus::Draft,
                    WorkStatus::Closed,
                    WorkStatus::Merged,
                ]
                .into_iter()
                .filter(|status| {
                    self.settings.inbox.source == WorkSource::GitHub
                        || matches!(status, WorkStatus::Open | WorkStatus::Closed)
                }) {
                    content = content.child(
                        MenuRow::new(status.label())
                            .checked(preferences.status_selected(status))
                            .render(SharedString::from(format!("filter-status-{status:?}")))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.settings.inbox.toggle_status(status);
                                this.inbox_filters_changed(cx);
                            })),
                    );
                }
                content = content.child(menu_heading("TIME"));
                for time in [
                    InboxTime::All,
                    InboxTime::Today,
                    InboxTime::Week,
                    InboxTime::Month,
                ] {
                    content = content.child(
                        MenuRow::new(time.label())
                            .checked(preferences.time == time)
                            .render(SharedString::from(format!("filter-time-{time:?}")))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.settings.inbox.time = time;
                                this.inbox_filters_changed(cx);
                            })),
                    );
                }
                if self.settings.inbox.source == WorkSource::GitHub {
                    content = content.child(menu_heading("TYPE"));
                    for (kind, label) in [
                        (WorkKind::Issue, "Issues"),
                        (WorkKind::PullRequest, "Pull requests"),
                    ] {
                        content = content.child(
                            MenuRow::new(label)
                                .checked(!preferences.hidden_kinds.contains(&kind))
                                .render(SharedString::from(format!("filter-kind-{kind:?}")))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    toggle_value(&mut this.settings.inbox.hidden_kinds, kind);
                                    this.inbox_filters_changed(cx);
                                })),
                        );
                    }
                    content = content.child(menu_heading("PROJECTS")).child(
                        MenuRow::new("All projects")
                            .checked(
                                self.work_inbox.filter.project.is_none()
                                    && preferences.hidden_projects.is_empty(),
                            )
                            .render("inbox-all-projects")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.work_inbox.filter.project = None;
                                this.settings.inbox.hidden_projects.clear();
                                this.inbox_filters_changed(cx);
                            })),
                    );
                    for project in self.snapshot.listed_projects() {
                        let id = project.id;
                        content = content.child(
                            MenuRow::new(project.name.clone())
                                .checked(self.inbox_project_selected(id))
                                .render(SharedString::from(format!("filter-project-{id}")))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_inbox_project(id, cx);
                                })),
                        );
                    }
                } else {
                    content = content.child(menu_heading("TEAMS / PROJECTS"));
                    let groups: std::collections::BTreeSet<_> = self
                        .work_inbox
                        .linear
                        .items
                        .iter()
                        .map(|item| item.group.clone())
                        .collect();
                    for group in groups {
                        content = content.child(
                            MenuRow::new(group.clone())
                                .checked(!preferences.hidden_groups.contains(&group))
                                .render(SharedString::from(format!("filter-group-{group}")))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    toggle_value(
                                        &mut this.settings.inbox.hidden_groups,
                                        group.clone(),
                                    );
                                    this.inbox_filters_changed(cx);
                                })),
                        );
                    }
                }
                content = content.child(menu_separator()).child(
                    MenuRow::new("Reset filters")
                        .render("inbox-clear-filters")
                        .on_click(cx.listener(|this, _, _, cx| {
                            let source = this.settings.inbox.source;
                            let width = this.settings.inbox.list_width;
                            let seen = std::mem::take(&mut this.settings.inbox.seen);
                            let linked_sessions =
                                std::mem::take(&mut this.settings.inbox.linked_sessions);
                            let repository_view = this.settings.inbox.repository_view;
                            this.settings.inbox = InboxPreferences {
                                source,
                                seen,
                                linked_sessions,
                                list_width: width,
                                repository_view,
                                ..Default::default()
                            };
                            this.work_inbox.filter = WorkFilter::default();
                            this.scope_inbox_to_active_project();
                            this.inbox_filters_changed(cx);
                        })),
                );
            }
            InboxMenu::Projects => {
                content = content.child(menu_heading("PROJECT"));
                for project in self
                    .snapshot
                    .projects
                    .iter()
                    .filter(|project| project.directory().is_some())
                {
                    let id = project.id;
                    content = content.child(
                        MenuRow::new(project.name.clone())
                            .checked(self.work_inbox.target_project == Some(id))
                            .render(SharedString::from(format!("task-project-{id}")))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.work_inbox.target_project = Some(id);
                                this.work_inbox.menu = None;
                                cx.notify();
                            })),
                    );
                }
            }
            InboxMenu::Agents => {
                content = content.child(menu_heading("AGENT"));
                for (index, (label, _)) in AGENTS.iter().enumerate() {
                    content = content.child(
                        MenuRow::new(*label)
                            .checked(self.work_inbox.agent == index)
                            .render(SharedString::from(format!("task-agent-{index}")))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.work_inbox.agent = index;
                                this.work_inbox.menu = None;
                                cx.notify();
                            })),
                    );
                }
            }
            InboxMenu::Connections => {
                content = content
                    .child(menu_heading("CONNECTIONS"))
                    .child(
                        MenuRow::new("GitHub · use gh session")
                            .icon("chrome-icons/github.svg")
                            .render("connect-github")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.select_work_source(WorkSource::GitHub, cx);
                                this.refresh_work_source(WorkSource::GitHub, true, cx);
                            })),
                    )
                    .child(
                        MenuRow::new("Linear · API key")
                            .icon("chrome-icons/linear.svg")
                            .render("connect-linear")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.work_inbox.menu = None;
                                this.open_settings(window, cx);
                                this.settings_page = crate::ui::workspace_view::SettingsPage::Inbox;
                                cx.notify();
                            })),
                    );
            }
            InboxMenu::Shortcuts => {
                content = content.child(menu_heading("KEYBOARD"));
                for (label, keys) in SHORTCUTS {
                    content = content.child(
                        MenuRow::new(*label)
                            .shortcut(keys)
                            .render(SharedString::from(format!("shortcut-{label}"))),
                    );
                }
            }
            InboxMenu::RepositoryProjects => {
                content = content.child(menu_heading("REPOSITORY"));
                let current = self.repository_project();
                for project in self
                    .snapshot
                    .listed_projects()
                    .filter(|project| project.directory().is_some())
                {
                    let id = project.id;
                    content = content.child(
                        MenuRow::new(project.name.clone())
                            .checked(current == Some(id))
                            .render(SharedString::from(format!("repository-project-{id}")))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select_repository_project(id, cx);
                            })),
                    );
                }
            }
            InboxMenu::PrActions => {
                let item = self.selected_work_item()?;
                let detail = self
                    .work_inbox
                    .details
                    .get(&item.url)
                    .and_then(|state| state.summary.data.as_ref());
                for action in merge::available_pr_actions(item, detail) {
                    let url = item.url.clone();
                    content = content.child(
                        MenuRow::new(action.label())
                            .render(SharedString::from(format!("pr-action-{action:?}")))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.propose_inbox_pr_action(&url, action);
                                this.work_inbox.menu = None;
                                cx.notify();
                            })),
                    );
                }
            }
        }
        let (x, y) = self.work_inbox.menu_position;
        let size = window.bounds().size;
        let w: f32 = size.width.into();
        let h: f32 = size.height.into();
        Some(
            div()
                .absolute()
                .inset_0()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.work_inbox.menu = None;
                        cx.notify();
                    }),
                )
                .child(
                    menu_panel()
                        .absolute()
                        .left(px(x.min(w - 244.0).max(8.0)))
                        .top(px((y + 8.0).min((h - 500.0).max(48.0))))
                        .w(px(232.0))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(content),
                )
                .into_any_element(),
        )
    }
}

/// Review keyboard shortcuts, shown by `?` and the keyboard button.
pub(super) const SHORTCUTS: &[(&str, &str)] = &[
    ("Next task", "J"),
    ("Previous task", "K"),
    ("Summary, Code, Commits, Checks", "1–4"),
    ("Next file", "]"),
    ("Previous file", "["),
    ("Mark file viewed", "V"),
    ("Check out", "C"),
    ("Open in worktree", "W"),
    ("Open on GitHub", "O"),
    ("Refresh", "R"),
    ("Filter", "/"),
    ("Repository view", "G"),
    ("Shortcuts", "?"),
];

fn icon_button(
    id: &'static str,
    path: &'static str,
    label: &'static str,
) -> gpui::Stateful<gpui::Div> {
    super::super::chrome::icon_button(id, path, 24.0, true)
        .cursor_pointer()
        .hover(|button| button.bg(surface_tint(colors().hover, colors().background)))
        .tooltip(move |_, cx| sidebar_tooltip(label, cx))
}

fn quiet_button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id.into())
        .h(px(28.0))
        .px_2()
        .rounded(px(5.0))
        .flex_none()
        .flex()
        .items_center()
        .gap_1()
        .text_size(px(12.0))
        .text_color(colors().muted)
        .cursor_pointer()
        .hover(|button| button.bg(surface_tint(colors().hover, colors().background)))
        .child(label.into())
}

fn provider_icon(source: WorkSource, size: f32) -> impl IntoElement {
    svg()
        .path(match source {
            WorkSource::GitHub => "chrome-icons/github.svg",
            WorkSource::Linear => "chrome-icons/linear.svg",
        })
        .size(px(size))
        .flex_none()
        .text_color(colors().muted)
}

fn label_chip(label: &str) -> gpui::Div {
    div()
        .max_w(px(90.0))
        .truncate()
        .px(px(5.0))
        .py(px(1.0))
        .rounded(px(4.0))
        .bg(surface_tint(colors().elevated, colors().background))
        .text_size(px(10.0))
        .text_color(colors().muted)
        .child(label.to_owned())
}

/// One scannable line of a pull request card: size, CI, review, conflicts,
/// and where it stands locally.
fn pull_facts(pull: &PullSummary, local: super::local::LocalPull<'_>) -> impl IntoElement {
    use super::local::LocalPull;
    let chip = |label: &'static str, color: Rgba| {
        div()
            .flex_none()
            .px(px(5.0))
            .rounded(px(4.0))
            .border_1()
            .border_color(colors().border_subtle)
            .text_size(px(10.0))
            .text_color(color)
            .child(label)
    };
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .text_size(px(11.0))
        .text_color(colors().subtle)
        .child(
            div()
                .flex_none()
                .flex()
                .gap(px(4.0))
                .child(
                    div()
                        .text_color(colors().diff_added)
                        .child(format!("+{}", pull.additions)),
                )
                .child(
                    div()
                        .text_color(colors().diff_deleted)
                        .child(format!("−{}", pull.deletions)),
                ),
        )
        .when(pull.checks != CheckRollup::None, |row| {
            row.child(checks::check_status_icon(
                pull.checks == CheckRollup::Failing,
                pull.checks == CheckRollup::Passing,
                12.0,
            ))
        })
        .when(pull.review_decision == "APPROVED", |row| {
            row.child(chip("Approved", colors().success))
        })
        .when(pull.review_decision == "CHANGES_REQUESTED", |row| {
            row.child(chip("Changes requested", colors().danger))
        })
        .when(pull.mergeable == "CONFLICTING", |row| {
            row.child(chip("Conflicts", colors().warning))
        })
        .child(div().flex_1())
        .map(|row| match local {
            LocalPull::Branch { current: true, .. } => {
                row.child(chip("Checked out", colors().accent))
            }
            LocalPull::Branch {
                worktree: Some(_), ..
            } => row.child(chip("Worktree", colors().accent)),
            LocalPull::Branch { .. } => row.child(chip("Local", colors().muted)),
            _ => row,
        })
}

fn short_time(at: u64) -> String {
    let value = relative_time(unix_now(), at);
    value.strip_suffix(" ago").unwrap_or(&value).to_owned()
}

fn status_mark(item: &WorkItem) -> (&'static str, Rgba) {
    match item.status {
        WorkStatus::Merged => ("chrome-icons/git-merge.svg", gpui::rgb(0xa78bfa)),
        WorkStatus::Closed if item.completed => {
            ("chrome-icons/issue-closed.svg", gpui::rgb(0xa78bfa))
        }
        WorkStatus::Closed => ("chrome-icons/issue-canceled.svg", colors().danger),
        WorkStatus::Draft => ("chrome-icons/git-pull-request.svg", colors().subtle),
        WorkStatus::Open if item.kind == WorkKind::PullRequest => {
            ("chrome-icons/git-pull-request.svg", colors().success)
        }
        WorkStatus::Open => ("chrome-icons/issue-open.svg", colors().success),
    }
}
