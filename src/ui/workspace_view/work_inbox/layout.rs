//! Repository: the section's tabs, the pull request and issue list beside
//! the selected task, and its menus.

mod checks;
mod code;
mod commits;
mod history;
mod item_detail;
mod local;
mod merge;

use super::*;
use crate::domain::inbox::relative_time;
use crate::domain::work_items::{InboxPreferences, InboxTime, PullStep, RepositoryTab};
use crate::infrastructure::clock::unix_now;
use crate::ui::menu::{MENU_WIDTH, MenuRow, menu_heading, menu_panel, menu_separator};
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
        let body = if self.settings.inbox.tab == RepositoryTab::History {
            self.history_view(cx)
        } else {
            self.inbox_list_and_detail(cx)
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
            .child(self.repository_tabs(cx))
            .child(body)
            .into_any_element()
    }

    /// The pull request or issue list beside the selected task.
    fn inbox_list_and_detail(&self, cx: &mut Context<Self>) -> AnyElement {
        let width = if self.settings.inbox.list_width.is_finite() {
            self.settings.inbox.list_width.clamp(240.0, 420.0)
        } else {
            280.0
        };
        let list = div()
            .id("inbox-list-pane")
            .relative()
            .w(px(width))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(colors().border_subtle)
            .child(self.inbox_list_header(cx))
            .child(self.inbox_task_list(cx))
            .child(
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
        div()
            .id("inbox-list-and-detail")
            .flex_1()
            .min_h(px(0.0))
            .flex()
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
            .child(list)
            .child(self.inbox_detail_panel(cx))
            .into_any_element()
    }

    /// Search, filters, and refresh.
    fn inbox_list_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let feed = &self.work_inbox.github;
        let editing = self.work_inbox.search_editing;
        let query = self.work_inbox.filter.query.clone();
        div()
            .h(px(44.0))
            .flex_none()
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .id("inbox-filter-input")
                    .flex_1()
                    .min_w(px(0.0))
                    .h(px(28.0))
                    .pl_2()
                    .pr_1()
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(if editing {
                        colors().accent
                    } else {
                        colors().border_subtle
                    })
                    .bg(surface_tint(colors().elevated, colors().background))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
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
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .text_size(px(12.0))
                            .text_color(if query.is_empty() {
                                colors().subtle
                            } else {
                                colors().foreground
                            })
                            .child(if !query.is_empty() {
                                query.clone()
                            } else if self.settings.inbox.tab == RepositoryTab::PullRequests {
                                "Search pull requests".to_owned()
                            } else {
                                "Search issues".to_owned()
                            }),
                    )
                    .when(query.is_empty() && !editing, |field| {
                        field.child(key_hint("/"))
                    })
                    .when(!query.is_empty(), |field| {
                        field.child(
                            div()
                                .id("inbox-clear-search")
                                .size(px(18.0))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(4.0))
                                .cursor_pointer()
                                .hover(|button| {
                                    button.bg(surface_tint(colors().hover, colors().background))
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.work_inbox.filter.query.clear();
                                    this.sync_inbox_selection(cx);
                                    cx.notify();
                                }))
                                .child(
                                    svg()
                                        .path("chrome-icons/close.svg")
                                        .size(px(10.0))
                                        .text_color(colors().muted),
                                ),
                        )
                    }),
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

    /// History · Pull requests · Issues, with Activity and the shortcuts.
    fn repository_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.settings.inbox.tab;
        div()
            .h(px(38.0))
            .flex_none()
            .pl_4()
            .pr_2()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(colors().border_subtle)
            .child(div().h_full().min_w(px(0.0)).flex().gap(px(18.0)).children(
                RepositoryTab::ALL.into_iter().map(|tab| {
                    let selected = current == tab;
                    let count = tab.kind().map(|kind| self.inbox_items_of(kind).count());
                    div()
                        .id(SharedString::from(format!("repository-tab-{tab:?}")))
                        .h_full()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .relative()
                        .when(selected, |tab| tab.child(tab_underline()))
                        .cursor_pointer()
                        .text_size(px(12.5))
                        .text_color(if selected {
                            colors().foreground
                        } else {
                            colors().subtle
                        })
                        .hover(|tab| tab.text_color(colors().foreground))
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.select_repository_tab(tab, cx)),
                        )
                        .child(tab.label())
                        .when_some(count.filter(|count| *count > 0), |tab, count| {
                            tab.child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(colors().subtle)
                                    .child(count.to_string()),
                            )
                        })
                }),
            ))
            .child(div().flex_1().min_w(px(8.0)))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .child(
                        icon_button(
                            "inbox-activity",
                            "chrome-icons/activity.svg",
                            "Agent activity",
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.work_inbox.activity = true;
                            this.dismiss_inbox_menu();
                            this.sync_terminal_surface_visibility(cx);
                            cx.notify();
                        })),
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
                    ),
            )
    }

    /// Switch tabs. A list returns to the task last selected there.
    pub(super) fn select_repository_tab(&mut self, tab: RepositoryTab, cx: &mut Context<Self>) {
        let previous = self.settings.inbox.tab;
        if previous == tab {
            return;
        }
        if let (Some(kind), Some(url)) = (previous.kind(), self.work_inbox.selected.clone()) {
            self.work_inbox.tab_selection.insert(kind, url);
        }
        self.dismiss_inbox_menu();
        self.settings.inbox.tab = tab;
        match tab.kind() {
            None => self.open_history(false, cx),
            Some(kind) => {
                if let Some(url) = self.work_inbox.tab_selection.get(&kind).cloned()
                    && self
                        .ordered_inbox_items()
                        .iter()
                        .any(|item| item.url == url)
                {
                    self.select_work_item(&url, cx);
                } else {
                    self.sync_inbox_selection(cx);
                }
            }
        }
        self.sync_terminal_surface_visibility(cx);
        self.persist_settings(cx);
        cx.notify();
    }

    fn inbox_task_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let feed = &self.work_inbox.github;
        let kind = self
            .settings
            .inbox
            .tab
            .kind()
            .unwrap_or(WorkKind::PullRequest);
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
            .gap(px(1.0));
        if let Some(error) = &feed.error {
            list = list.child(message(error, true));
        }
        if feed.loading && feed.items.is_empty() {
            list = list.child(message("Loading…", false));
        } else if groups.is_empty() {
            let filtered = self.settings.inbox.filters_active()
                || self.work_inbox.filter.project.is_some()
                || !self.work_inbox.filter.query.is_empty();
            list = list.child(list_empty_state(match (kind, filtered) {
                (WorkKind::PullRequest, true) => "No pull requests match these filters.",
                (WorkKind::PullRequest, false) => "No pull requests in your spaces.",
                (WorkKind::Issue, true) => "No issues match these filters.",
                (WorkKind::Issue, false) => "No issues in your spaces.",
            }));
        }
        // Tasks of several projects carry their project's color.
        let projects = self.work_inbox.filter.project.is_none() && self.snapshot.projects.len() > 1;
        // One group needs no heading: the list reads as before.
        let grouped = groups.len() > 1;
        for (relation, items) in groups {
            let collapsed = grouped && self.work_inbox.collapsed_groups.contains(&relation);
            if grouped {
                list = list.child(
                    group_heading(relation.label(kind), items.len(), collapsed).on_click(
                        cx.listener(move |this, _, _, cx| {
                            if !this.work_inbox.collapsed_groups.remove(&relation) {
                                this.work_inbox.collapsed_groups.insert(relation);
                            }
                            this.sync_inbox_selection(cx);
                            cx.notify();
                        }),
                    ),
                );
            }
            if collapsed {
                continue;
            }
            for item in items {
                list = list.child(self.inbox_task_row(item, projects, cx));
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
        if self.settings.inbox.tab == RepositoryTab::History {
            // The feed still matters: it gives commits their pull request.
            self.open_history(true, cx);
            self.refresh_work_items(true, cx);
            return;
        }
        self.refresh_work_items(true, cx);
        self.load_inbox_detail(true, cx);
        self.refresh_inbox_repos(true, cx);
    }

    pub(super) fn focus_inbox_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.work_inbox.search_editing = true;
        self.work_inbox.comment_editing = false;
        self.work_inbox.composer_editing = false;
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

    /// Two lines: status and title, then what the task waits for.
    fn inbox_task_row(
        &self,
        item: &WorkItem,
        projects: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use super::local::LocalPull;
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
        let local = match local {
            LocalPull::Branch { current: true, .. } => Some((
                "chrome-icons/git-branch.svg",
                "Checked out",
                colors().accent,
            )),
            LocalPull::Branch {
                worktree: Some(_), ..
            } => Some((
                "chrome-icons/worktree.svg",
                "In a worktree",
                colors().accent,
            )),
            LocalPull::Branch { .. } => Some((
                "chrome-icons/git-branch.svg",
                "Local branch",
                colors().subtle,
            )),
            _ => None,
        };
        let row_id = SharedString::from(format!("inbox-item-{url}"));
        div()
            .id(row_id)
            .px(px(8.0))
            .py(px(7.0))
            .rounded(px(6.0))
            .flex()
            .items_start()
            .gap(px(8.0))
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
                    .pt(px(2.0))
                    .flex_none()
                    .child(svg().path(status_icon).size(px(14.0)).text_color(color)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .truncate()
                                    .text_size(px(13.0))
                                    .text_color(colors().foreground)
                                    .child(item.title.clone()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(11.0))
                                    .text_color(colors().subtle)
                                    .child(short_time(item.updated_at)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .text_size(px(11.5))
                            .text_color(colors().subtle)
                            .when(projects, |row| {
                                row.when_some(item.project_id, |row, id| {
                                    row.child(
                                        div()
                                            .size(px(8.0))
                                            .flex_none()
                                            .rounded(px(2.0))
                                            .bg(project_color(id)),
                                    )
                                })
                            })
                            .child(div().flex_none().child(item.reference.clone()))
                            .when_some(item.pull_step(), |row, step| {
                                row.child(
                                    div()
                                        .min_w(px(0.0))
                                        .truncate()
                                        .text_color(step_color(step))
                                        .child(step.label()),
                                )
                            })
                            .when(item.pull.is_none(), |row| {
                                row.children(
                                    item.labels.iter().take(2).map(|label| label_chip(label)),
                                )
                            })
                            .child(div().flex_1())
                            .when_some(item.pull.as_ref(), |row, pull| {
                                row.child(line_counts(pull.additions, pull.deletions))
                            })
                            .when_some(local, |row, (icon, label, color)| {
                                row.child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "inbox-item-local-{}",
                                            item.url
                                        )))
                                        .flex_none()
                                        .tooltip(move |_, cx| sidebar_tooltip(label, cx))
                                        .child(svg().path(icon).size(px(11.0)).text_color(color)),
                                )
                            })
                            .when(linked, |row| {
                                row.child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "inbox-item-linked-{}",
                                            item.url
                                        )))
                                        .flex_none()
                                        .tooltip(|_, cx| sidebar_tooltip("Agent session open", cx))
                                        .child(
                                            svg()
                                                .path("chrome-icons/terminal.svg")
                                                .size(px(11.0))
                                                .text_color(colors().accent),
                                        ),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn open_inbox_menu(
        &mut self,
        menu: InboxMenu,
        event: &ClickEvent,
        cx: &mut Context<Self>,
    ) {
        let reopen = self.work_inbox.menu == Some(menu);
        self.dismiss_inbox_menu();
        if !reopen {
            self.work_inbox.menu = Some(menu);
            self.work_inbox.menu_position = (event.position().x.into(), event.position().y.into());
        }
        self.work_inbox.search_editing = false;
        self.work_inbox.comment_editing = false;
        self.work_inbox.composer_editing = false;
        cx.notify();
    }

    /// Close the open popover. A confirmation that is running and a review
    /// being sent stay; an unsent review keeps its text for later.
    pub(super) fn dismiss_inbox_menu(&mut self) -> bool {
        let Some(menu) = self.work_inbox.menu.take() else {
            return false;
        };
        let state = self
            .work_inbox
            .selected
            .clone()
            .and_then(|url| self.work_inbox.details.get_mut(&url));
        match menu {
            InboxMenu::Confirm if state.as_ref().is_some_and(|state| state.mutation_busy) => {
                self.work_inbox.menu = Some(menu);
                return false;
            }
            InboxMenu::Confirm => self.work_inbox.confirmation = None,
            InboxMenu::Review => {
                if let Some(state) = state {
                    if state.review_draft.busy {
                        self.work_inbox.menu = Some(menu);
                        return false;
                    }
                    state.review_draft.open = false;
                    state.review_draft.editing = false;
                }
            }
            _ => {}
        }
        true
    }

    /// Ask before running a pull request action, beside the control that
    /// proposed it.
    pub(super) fn confirm_pr_action_at(
        &mut self,
        url: &str,
        action: PrAction,
        position: (f32, f32),
        cx: &mut Context<Self>,
    ) {
        self.dismiss_inbox_menu();
        self.propose_inbox_pr_action(url, action);
        if self.work_inbox.confirmation.is_some() {
            self.work_inbox.menu = Some(InboxMenu::Confirm);
            self.work_inbox.menu_position = position;
        }
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
        let mut width = MENU_WIDTH;
        let mut content = div()
            .id("inbox-menu-scroll")
            .max_h(px(480.0))
            .overflow_y_scroll();
        match menu {
            InboxMenu::Filters => content = content.children(self.inbox_filter_rows(cx)),
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
            InboxMenu::PrActions => {
                let item = self.selected_work_item()?;
                let detail = self
                    .work_inbox
                    .details
                    .get(&item.url)
                    .and_then(|state| state.summary.data.as_ref());
                let actions = merge::available_pr_actions(item, detail);
                let mut previous_group = None;
                for action in actions {
                    let group = action_group(action);
                    if previous_group.is_some_and(|previous| previous != group) {
                        content = content.child(menu_separator());
                    }
                    previous_group = Some(group);
                    let url = item.url.clone();
                    let mut row = MenuRow::new(action.label());
                    if matches!(action, PrAction::Close | PrAction::DeleteBranch) {
                        row = row.danger();
                    }
                    content = content.child(
                        row.render(SharedString::from(format!("pr-action-{action:?}")))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let position = this.work_inbox.menu_position;
                                this.confirm_pr_action_at(&url, action, position, cx);
                            })),
                    );
                }
            }
            InboxMenu::Branch => {
                let item = self.selected_work_item()?;
                content = content.children(self.inbox_branch_menu(item, cx));
            }
            InboxMenu::Review => {
                width = 360.0;
                content = content.child(self.inbox_review_popover(cx)?);
            }
            InboxMenu::Confirm => {
                width = 320.0;
                content = content.child(self.inbox_confirmation(cx)?);
            }
            InboxMenu::HistoryProjects => {
                content = content.child(menu_heading("PROJECT"));
                let current = self.history_project();
                for project in self
                    .snapshot
                    .projects
                    .iter()
                    .filter(|project| project.directory().is_some())
                {
                    let id = project.id;
                    content = content.child(
                        MenuRow::new(project.name.clone())
                            .checked(current == Some(id))
                            .render(SharedString::from(format!("history-project-{id}")))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select_history_project(id, cx);
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
                        if this.dismiss_inbox_menu() {
                            cx.notify();
                        }
                    }),
                )
                .child(
                    menu_panel()
                        .absolute()
                        .left(px(x.min(w - width - 12.0).max(8.0)))
                        .top(px((y + 8.0).min((h - 500.0).max(48.0))))
                        .w(px(width))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(content),
                )
                .into_any_element(),
        )
    }

    fn inbox_filter_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let preferences = &self.settings.inbox;
        let mut rows = vec![
            MenuRow::new("Assigned to me")
                .checked(preferences.assigned_to_me)
                .render("inbox-filter-assigned")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.settings.inbox.assigned_to_me = !this.settings.inbox.assigned_to_me;
                    this.inbox_filters_changed(cx);
                }))
                .into_any_element(),
            menu_heading("STATUS").into_any_element(),
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
                }))
                .into_any_element(),
        ];
        for status in [
            WorkStatus::Open,
            WorkStatus::Draft,
            WorkStatus::Closed,
            WorkStatus::Merged,
        ] {
            rows.push(
                MenuRow::new(status.label())
                    .checked(preferences.status_selected(status))
                    .render(SharedString::from(format!("filter-status-{status:?}")))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings.inbox.toggle_status(status);
                        this.inbox_filters_changed(cx);
                    }))
                    .into_any_element(),
            );
        }
        rows.push(menu_heading("UPDATED").into_any_element());
        for time in [
            InboxTime::All,
            InboxTime::Today,
            InboxTime::Week,
            InboxTime::Month,
        ] {
            rows.push(
                MenuRow::new(time.label())
                    .checked(preferences.time == time)
                    .render(SharedString::from(format!("filter-time-{time:?}")))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings.inbox.time = time;
                        this.inbox_filters_changed(cx);
                    }))
                    .into_any_element(),
            );
        }
        rows.push(menu_heading("PROJECTS").into_any_element());
        rows.push(
            MenuRow::new("All spaces")
                .checked(
                    self.work_inbox.filter.project.is_none()
                        && preferences.hidden_projects.is_empty(),
                )
                .render("inbox-all-projects")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.work_inbox.filter.project = None;
                    this.settings.inbox.hidden_projects.clear();
                    this.inbox_filters_changed(cx);
                }))
                .into_any_element(),
        );
        for project in self.snapshot.projects.iter() {
            let id = project.id;
            rows.push(
                MenuRow::new(project.name.clone())
                    .checked(self.inbox_project_selected(id))
                    .render(SharedString::from(format!("filter-project-{id}")))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_inbox_project(id, cx);
                    }))
                    .into_any_element(),
            );
        }
        rows.push(menu_separator().into_any_element());
        rows.push(
            MenuRow::new("Reset filters")
                .render("inbox-clear-filters")
                .on_click(cx.listener(|this, _, _, cx| {
                    let preferences = std::mem::take(&mut this.settings.inbox);
                    this.settings.inbox = InboxPreferences {
                        seen: preferences.seen,
                        linked_sessions: preferences.linked_sessions,
                        list_width: preferences.list_width,
                        tab: preferences.tab,
                        ..Default::default()
                    };
                    this.work_inbox.filter = WorkFilter::default();
                    this.scope_inbox_to_active_project();
                    this.inbox_filters_changed(cx);
                }))
                .into_any_element(),
        );
        rows
    }

    /// The tasks of one tab that pass the filters, for its count.
    fn inbox_items_of(&self, kind: WorkKind) -> impl Iterator<Item = &WorkItem> {
        self.work_inbox.github.items.iter().filter(move |item| {
            item.kind == kind && self.work_inbox.filter.matches(item, &self.settings.inbox)
        })
    }
}

/// Review keyboard shortcuts, shown by `?` and the keyboard button.
pub(super) const SHORTCUTS: &[(&str, &str)] = &[
    ("History, Pull requests, Issues", "H / P / I"),
    ("Next task or commit", "J"),
    ("Previous task or commit", "K"),
    ("Conversation, Files, Commits, Checks", "1–4"),
    ("Next file", "]"),
    ("Previous file", "["),
    ("Mark file viewed", "V"),
    ("Check out", "C"),
    ("Open in worktree", "W"),
    ("Send to agent", "A"),
    ("Open on GitHub", "O"),
    ("Open the commit", "Enter"),
    ("Back to history", "Esc"),
    ("Refresh", "R"),
    ("Search", "/"),
    ("Shortcuts", "?"),
];

/// Separators in the pull request menu: merge methods, auto-merge, branch
/// upkeep, then state changes.
fn action_group(action: PrAction) -> u8 {
    match action {
        PrAction::Merge | PrAction::Squash | PrAction::Rebase => 0,
        PrAction::AutoMerge
        | PrAction::AutoSquash
        | PrAction::AutoRebase
        | PrAction::DisableAutoMerge => 1,
        PrAction::UpdateBranch => 2,
        _ => 3,
    }
}

fn icon_button(
    id: &'static str,
    path: &'static str,
    label: &'static str,
) -> gpui::Stateful<gpui::Div> {
    super::super::chrome::icon_button(id, path, 26.0, true)
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
        .hover(|button| {
            button
                .bg(surface_tint(colors().hover, colors().background))
                .text_color(colors().foreground)
        })
        .child(label.into())
}

fn github_icon(size: f32) -> impl IntoElement {
    svg()
        .path("chrome-icons/github.svg")
        .size(px(size))
        .flex_none()
        .text_color(colors().muted)
}

fn key_hint(key: &'static str) -> impl IntoElement {
    div()
        .flex_none()
        .min_w(px(16.0))
        .h(px(16.0))
        .px(px(4.0))
        .rounded(px(4.0))
        .border_1()
        .border_color(colors().border_subtle)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(10.0))
        .text_color(colors().subtle)
        .child(key)
}

/// The selected tab's line, drawn over the row's bottom edge so the label
/// stays centered with the buttons beside it.
fn tab_underline() -> impl IntoElement {
    div()
        .absolute()
        .left_0()
        .right_0()
        .bottom_0()
        .h(px(2.0))
        .bg(colors().foreground)
}

fn group_heading(label: &'static str, count: usize, collapsed: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(format!("inbox-group-{label}")))
        .h(px(26.0))
        .px(px(8.0))
        .mt(px(6.0))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.0))
        .rounded(px(5.0))
        .cursor_pointer()
        .hover(|row| row.bg(surface_tint(colors().hover, colors().background)))
        .child(
            div()
                .flex_1()
                .text_size(px(10.5))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(colors().muted)
                .child(label.to_uppercase()),
        )
        .child(
            div()
                .text_size(px(10.5))
                .text_color(colors().subtle)
                .child(count.to_string()),
        )
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
}

fn list_empty_state(text: &'static str) -> impl IntoElement {
    div()
        .pt(px(48.0))
        .px_4()
        .flex()
        .flex_col()
        .items_center()
        .gap_2()
        .text_color(colors().subtle)
        .child(
            svg()
                .path("chrome-icons/inbox.svg")
                .size(px(20.0))
                .text_color(colors().subtle),
        )
        .child(div().text_size(px(12.0)).text_center().child(text))
}

fn label_chip(label: &str) -> gpui::Div {
    div()
        .flex_none()
        .max_w(px(90.0))
        .truncate()
        .px(px(5.0))
        .rounded(px(4.0))
        .bg(surface_tint(colors().elevated, colors().background))
        .text_size(px(10.0))
        .text_color(colors().muted)
        .child(label.to_owned())
}

fn line_counts(additions: u64, deletions: u64) -> impl IntoElement {
    div()
        .flex_none()
        .flex()
        .gap(px(4.0))
        .text_size(px(11.0))
        .child(
            div()
                .text_color(colors().diff_added)
                .child(format!("+{additions}")),
        )
        .child(
            div()
                .text_color(colors().diff_deleted)
                .child(format!("−{deletions}")),
        )
}

fn short_time(at: u64) -> String {
    let value = relative_time(unix_now(), at);
    value.strip_suffix(" ago").unwrap_or(&value).to_owned()
}

fn merged_color() -> Rgba {
    gpui::rgb(0xa78bfa)
}

fn step_color(step: PullStep) -> Rgba {
    match step {
        PullStep::Conflicts | PullStep::ChecksRunning => colors().warning,
        PullStep::ChecksFailing | PullStep::ChangesRequested => colors().danger,
        PullStep::ReviewRequested => colors().accent,
        PullStep::ReadyToMerge => colors().success,
        PullStep::Merged => merged_color(),
        PullStep::Draft | PullStep::AwaitingReview | PullStep::Closed => colors().subtle,
    }
}

fn status_mark(item: &WorkItem) -> (&'static str, Rgba) {
    match item.status {
        WorkStatus::Merged => ("chrome-icons/git-merge.svg", merged_color()),
        WorkStatus::Closed if item.completed => ("chrome-icons/issue-closed.svg", merged_color()),
        WorkStatus::Closed => ("chrome-icons/issue-canceled.svg", colors().danger),
        WorkStatus::Draft => ("chrome-icons/git-pull-request.svg", colors().subtle),
        WorkStatus::Open if item.kind == WorkKind::PullRequest => {
            ("chrome-icons/git-pull-request.svg", colors().success)
        }
        WorkStatus::Open => ("chrome-icons/issue-open.svg", colors().success),
    }
}
