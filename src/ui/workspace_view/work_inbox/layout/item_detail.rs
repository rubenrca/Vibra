use super::*;
use crate::domain::work_items::WorkComment;
use crate::ui::markdown::{markdown, safe_link};

/// Reading width of the conversation.
const PROSE_WIDTH: f32 = 780.0;

impl WorkspaceView {
    pub(super) fn inbox_detail_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(item) = self.selected_work_item() else {
            return div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .text_color(colors().subtle)
                .child(
                    svg()
                        .path("chrome-icons/inbox.svg")
                        .size(px(24.0))
                        .text_color(colors().subtle),
                )
                .child(div().text_size(px(13.0)).child(
                    if self.settings.inbox.tab == RepositoryTab::PullRequests {
                        "Select a pull request"
                    } else {
                        "Select an issue"
                    },
                ))
                .into_any_element();
        };
        let state = self.work_inbox.details.get(&item.url);
        let pull = item.kind == WorkKind::PullRequest;
        let tab = if pull {
            self.work_inbox.detail_tab
        } else {
            DetailTab::Summary
        };
        let mut header = div()
            .flex_none()
            .min_w(px(0.0))
            .px(px(20.0))
            .pt(px(14.0))
            .when(!pull, |header| header.pb(px(14.0)))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .border_b_1()
            .border_color(colors().border_subtle)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_start()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(240.0))
                            .text_size(px(17.0))
                            .line_height(px(23.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(colors().foreground)
                            .child(item.title.clone()),
                    )
                    .child(self.inbox_header_actions(item, state, cx)),
            )
            .child(self.inbox_meta_row(item, state, cx));
        if let Some(feedback) = self.inbox_local_feedback(&item.url) {
            header = header.child(feedback);
        }
        if pull {
            header = header.child(self.inbox_detail_tabs(state, cx));
        }
        let full_bleed = pull
            && (tab == DetailTab::Code
                || (tab == DetailTab::Commits
                    && state.is_some_and(|state| state.selected_commit.is_some())));
        let body = if full_bleed {
            div()
                .flex_1()
                .min_h(px(0.0))
                .flex()
                .flex_col()
                .child(if tab == DetailTab::Code {
                    self.inbox_diff_body(item, state, cx)
                } else {
                    self.inbox_commits_body(item, state, cx)
                })
                .into_any_element()
        } else {
            div()
                .id(SharedString::from(format!(
                    "inbox-detail-scroll-{}-{}",
                    item.url, tab as u8
                )))
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .px(px(20.0))
                .py(px(18.0))
                .child(div().w_full().max_w(px(PROSE_WIDTH)).child(match tab {
                    DetailTab::Checks => self.inbox_checks_body(item, state, cx),
                    DetailTab::Commits => self.inbox_commits_body(item, state, cx),
                    _ => self.inbox_summary_body(item, state, cx),
                }))
                .into_any_element()
        };
        let center = div()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .flex()
            .flex_col()
            .child(header)
            .child(body);
        div()
            .relative()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .flex()
            .child(center)
            .when(
                self.work_inbox.composer_open || self.work_inbox.discussion_open,
                |row| row.child(self.inbox_agent_panel(item, cx)),
            )
            .into_any_element()
    }

    /// Agent actions, the link to GitHub, and the pull request's state menu.
    fn inbox_header_actions(
        &self,
        item: &WorkItem,
        state: Option<&ItemState>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let pull = item.kind == WorkKind::PullRequest;
        let url = item.url.clone();
        let summary = state.and_then(|state| state.summary.data.as_ref());
        let actions = super::merge::available_pr_actions(item, summary);
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(
                header_button(
                    "inbox-send-to-agent",
                    Some("chrome-icons/send.svg"),
                    "Send to agent",
                    !pull,
                )
                .tooltip(|_, cx| sidebar_tooltip("Start an agent on this task (A)", cx))
                .on_click(cx.listener(|this, _, _, cx| this.open_inbox_composer(cx))),
            )
            .child(
                header_button("inbox-ask", Some("chrome-icons/comment.svg"), "Ask", false)
                    .tooltip(|_, cx| sidebar_tooltip("Ask an agent about it, read-only", cx))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.open_inbox_discussion(window, cx)),
                    ),
            )
            .child(
                icon_button(
                    "inbox-external",
                    "chrome-icons/open-external.svg",
                    "Open on GitHub (O)",
                )
                .on_click(move |_, _, cx| cx.open_url(&url)),
            )
            .when(pull && !actions.is_empty(), |row| {
                let open = item.status == WorkStatus::Open;
                let ready = item.pull_step() == Some(PullStep::ReadyToMerge);
                row.child(
                    header_button(
                        "inbox-pr-actions",
                        None,
                        if open { "Merge" } else { "Actions" },
                        open && ready,
                    )
                    .child(
                        svg()
                            .path("chrome-icons/chevron-down.svg")
                            .size(px(12.0))
                            .text_color(if open && ready {
                                colors().background
                            } else {
                                colors().subtle
                            }),
                    )
                    .on_click(cx.listener(|this, event, _, cx| {
                        this.open_inbox_menu(InboxMenu::PrActions, event, cx)
                    })),
                )
            })
    }

    /// State, number, people, branch, freshness, agents, and labels.
    fn inbox_meta_row(
        &self,
        item: &WorkItem,
        state: Option<&ItemState>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (icon, color) = status_mark(item);
        let (label, label_color) = match item.pull_step() {
            Some(step) => (step.label().to_owned(), step_color(step)),
            None => (item.state_label.clone(), color),
        };
        let sessions: Vec<_> = self
            .settings
            .inbox
            .linked_sessions
            .get(&item.url)
            .into_iter()
            .flatten()
            .filter_map(|pane| {
                self.snapshot
                    .terminal_sessions()
                    .find(|session| session.id == *pane)
            })
            .collect();
        let dot = || div().text_color(colors().subtle).child("·");
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.0))
            .text_size(px(12.0))
            .text_color(colors().subtle)
            .child(
                div()
                    .flex_none()
                    .h(px(22.0))
                    .px(px(7.0))
                    .rounded(px(11.0))
                    .flex()
                    .items_center()
                    .gap(px(5.0))
                    .bg(surface_tint(colors().elevated, colors().background))
                    .text_color(label_color)
                    .child(svg().path(icon).size(px(12.0)).text_color(color))
                    .child(label),
            )
            .child(div().flex_none().child(item.reference.clone()))
            .when(!item.author.is_empty(), |row| {
                row.child(dot()).child(person(&item.author))
            })
            .when(item.kind == WorkKind::Issue, |row| {
                row.child(dot()).map(|row| {
                    if item.assignees.is_empty() {
                        row.child("Unassigned")
                    } else {
                        row.children(item.assignees.iter().map(|name| person(name)))
                    }
                })
            })
            .when_some(item.pull.as_ref(), |row, pull| {
                let base = state
                    .and_then(|state| state.summary.data.as_ref())
                    .map(|detail| detail.base_ref.clone())
                    .filter(|base| !base.is_empty())
                    .unwrap_or_else(|| pull.base_ref.clone());
                row.child(dot())
                    .when(!base.is_empty(), |row| {
                        row.child(div().flex_none().child(base)).child(
                            svg()
                                .path("chrome-icons/chevron-left.svg")
                                .size(px(11.0))
                                .flex_none()
                                .text_color(colors().subtle),
                        )
                    })
                    .child(self.inbox_branch_pill(item, cx))
            })
            .child(dot())
            .child(
                div()
                    .flex_none()
                    .child(format!("Updated {}", short_time(item.updated_at))),
            )
            .children(sessions.into_iter().map(|session| {
                let pane = session.id;
                div()
                    .id(SharedString::from(format!("inbox-related-{pane}")))
                    .flex_none()
                    .max_w(px(200.0))
                    .h(px(22.0))
                    .px(px(7.0))
                    .rounded(px(11.0))
                    .flex()
                    .items_center()
                    .gap(px(5.0))
                    .bg(colors().selection)
                    .text_color(colors().accent)
                    .cursor_pointer()
                    .hover(|chip| chip.opacity(0.85))
                    .tooltip(|_, cx| sidebar_tooltip("Open the agent's terminal", cx))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.open_pane(pane, window, cx)),
                    )
                    .child(
                        svg()
                            .path("chrome-icons/terminal.svg")
                            .size(px(11.0))
                            .flex_none()
                            .text_color(colors().accent),
                    )
                    .child(
                        div().min_w(px(0.0)).truncate().child(
                            session
                                .agent_task_title
                                .clone()
                                .unwrap_or_else(|| session.title.clone()),
                        ),
                    )
            }))
            .children(item.labels.iter().take(3).map(|label| label_chip(label)))
    }

    fn inbox_detail_tabs(
        &self,
        state: Option<&ItemState>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let checks = state.and_then(|state| state.checks.data.as_ref());
        let count = |tab: DetailTab| match tab {
            DetailTab::Code => state
                .and_then(|state| state.diff.data.as_ref())
                .map(|diff| diff.files.len()),
            DetailTab::Commits => state
                .and_then(|state| state.commits.data.as_ref())
                .map(|commits| commits.total),
            _ => None,
        };
        div()
            .h(px(36.0))
            .flex()
            .items_center()
            .gap(px(18.0))
            .children(DetailTab::ALL.into_iter().map(|tab| {
                let selected = self.work_inbox.detail_tab == tab;
                let label = tab.label();
                div()
                    .id(SharedString::from(format!("inbox-detail-{label}")))
                    .h_full()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .cursor_pointer()
                    .text_size(px(12.5))
                    .text_color(if selected {
                        colors().foreground
                    } else {
                        colors().subtle
                    })
                    .hover(|tab| tab.text_color(colors().foreground))
                    .relative()
                    .when(selected, |tab| tab.child(tab_underline()))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.select_inbox_detail_tab(tab, cx)),
                    )
                    .child(label)
                    .when_some(count(tab), |tab, count| {
                        tab.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(colors().subtle)
                                .child(count.to_string()),
                        )
                    })
                    .when(tab == DetailTab::Checks, |tab| {
                        tab.when_some(checks.filter(|checks| !checks.is_empty()), |tab, checks| {
                            let failed = checks.iter().any(|check| check.failed());
                            let passed = checks.iter().all(|check| check.passed());
                            tab.child(checks::check_status_icon(failed, passed, 13.0))
                        })
                    })
            }))
    }

    fn inbox_summary_body(
        &self,
        item: &WorkItem,
        state: Option<&ItemState>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let detail = state.and_then(|state| state.summary.data.as_ref());
        let mut content = div().flex().flex_col().gap(px(20.0));
        if let Some(error) = state.and_then(|state| state.summary.error.as_ref()) {
            content = content.child(message(error, true));
        }
        if let Some(merge) = self.inbox_merge_box(item, state, cx) {
            content = content.child(merge);
        }
        let body = detail
            .map(|detail| detail.body.as_str())
            .unwrap_or(&item.body);
        content = content.child(if body.trim().is_empty() {
            div()
                .text_size(px(13.0))
                .text_color(colors().subtle)
                .child("No description.")
                .into_any_element()
        } else {
            crate::ui::markdown::markdown_prose(&format!("inbox-body-{}", item.url), body)
        });
        if state.is_some_and(|state| state.summary.loading) {
            content = content.child(message("Refreshing conversation…", false));
        }
        if let Some(detail) = detail {
            if !detail.comments.is_empty() {
                let count = detail.comments.len();
                content = content.child(
                    div()
                        .pt(px(18.0))
                        .border_t_1()
                        .border_color(colors().border_subtle)
                        .text_size(px(11.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(colors().muted)
                        .child(format!(
                            "{count} COMMENT{}",
                            if count == 1 { "" } else { "S" }
                        )),
                );
            }
            for comment in &detail.comments {
                content = content.child(self.inbox_comment(&item.url, comment, cx));
            }
            if detail.truncated {
                content = content.child(message(
                    "Showing recent comments. The full conversation is on GitHub.",
                    false,
                ));
            }
        }
        content
            .child(self.inbox_comment_composer(state, cx))
            .into_any_element()
    }

    fn inbox_comment_composer(
        &self,
        state: Option<&ItemState>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let draft = state.map(|state| state.draft.clone()).unwrap_or_default();
        let busy = state.is_some_and(|state| state.posting);
        let editing = self.work_inbox.comment_editing;
        div()
            .pt(px(18.0))
            .border_t_1()
            .border_color(colors().border_subtle)
            .flex()
            .flex_col()
            .gap_2()
            .when_some(
                state.and_then(|state| state.reply.as_ref()),
                |form, (_, author)| {
                    form.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(px(12.0))
                            .text_color(colors().muted)
                            .child(format!("Replying to {author}"))
                            .child(quiet_button("inbox-cancel-reply", "Cancel").on_click(
                                cx.listener(|this, _, _, cx| {
                                    if let Some(url) = this.work_inbox.selected.clone() {
                                        this.work_inbox.details.entry(url).or_default().reply =
                                            None;
                                    }
                                    cx.notify();
                                }),
                            )),
                    )
                },
            )
            .child(
                div()
                    .id("inbox-comment-input")
                    .min_h(px(72.0))
                    .max_h(px(180.0))
                    .overflow_y_scroll()
                    .p_3()
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(if editing {
                        colors().accent
                    } else {
                        colors().border_subtle
                    })
                    .bg(surface_tint(colors().elevated, colors().background))
                    .text_size(px(13.0))
                    .text_color(if draft.is_empty() {
                        colors().subtle
                    } else {
                        colors().foreground
                    })
                    .cursor_text()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if !busy {
                            this.work_inbox.comment_editing = true;
                            this.work_inbox.search_editing = false;
                            this.work_inbox.composer_editing = false;
                            this.focus_handle.focus(window);
                            cx.notify();
                        }
                    }))
                    .child(if draft.is_empty() {
                        "Write a comment…".into()
                    } else {
                        draft.clone()
                    }),
            )
            .when_some(
                state.and_then(|state| state.post_error.as_ref()),
                |form, error| form.child(message(error, true)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.0))
                            .text_color(colors().subtle)
                            .child("Markdown · ⌘↩ to post"),
                    )
                    .child(
                        header_button(
                            "inbox-post-comment",
                            None,
                            if busy { "Posting…" } else { "Comment" },
                            !draft.trim().is_empty() && !busy,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.post_inbox_comment(cx))),
                    ),
            )
    }

    /// One comment of the timeline with its replies.
    fn inbox_comment(
        &self,
        url: &str,
        comment: &WorkComment,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = url.to_owned();
        let reply = comment.reply_id.clone();
        let author = comment.author.clone();
        let link = safe_link(&comment.url).then(|| comment.url.clone());
        div()
            .flex()
            .gap(px(10.0))
            .child(avatar(&comment.author, 24.0))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .h(px(24.0))
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(px(12.0))
                            .text_color(colors().subtle)
                            .child(
                                div()
                                    .flex_none()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(colors().foreground)
                                    .child(comment.author.clone()),
                            )
                            .child(div().flex_none().child(short_time(comment.at)))
                            .when(!comment.context.is_empty(), |row| {
                                row.child(
                                    div()
                                        .min_w(px(0.0))
                                        .truncate()
                                        .font_family(crate::ui::theme::MONO_FONT)
                                        .text_size(px(11.0))
                                        .child(comment.context.clone()),
                                )
                            })
                            .child(div().flex_1())
                            .when_some(reply, |row, reply| {
                                row.child(
                                    quiet_button(
                                        SharedString::from(format!("reply-{}", comment.id)),
                                        "Reply",
                                    )
                                    .h(px(22.0))
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            this.work_inbox
                                                .details
                                                .entry(key.clone())
                                                .or_default()
                                                .reply = Some((reply.clone(), author.clone()));
                                            this.work_inbox.comment_editing = true;
                                            this.work_inbox.search_editing = false;
                                            this.work_inbox.composer_editing = false;
                                            this.focus_handle.focus(window);
                                            cx.notify();
                                        },
                                    )),
                                )
                            })
                            .when_some(link, |row, link| {
                                row.child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "comment-link-{}",
                                            comment.id
                                        )))
                                        .size(px(22.0))
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded(px(5.0))
                                        .cursor_pointer()
                                        .hover(|button| {
                                            button.bg(surface_tint(
                                                colors().hover,
                                                colors().background,
                                            ))
                                        })
                                        .tooltip(|_, cx| sidebar_tooltip("View on GitHub", cx))
                                        .on_click(move |_, _, cx| cx.open_url(&link))
                                        .child(
                                            svg()
                                                .path("chrome-icons/open-external.svg")
                                                .size(px(12.0))
                                                .text_color(colors().subtle),
                                        ),
                                )
                            }),
                    )
                    .child(markdown(&format!("comment-{}", comment.id), &comment.body))
                    .children(comment.replies.iter().map(|reply| {
                        div()
                            .mt_1()
                            .pl_3()
                            .border_l_2()
                            .border_color(colors().border_subtle)
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_size(px(12.0))
                                    .text_color(colors().subtle)
                                    .child(avatar(&reply.author, 18.0))
                                    .child(
                                        div()
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .text_color(colors().foreground)
                                            .child(reply.author.clone()),
                                    )
                                    .child(short_time(reply.at)),
                            )
                            .child(markdown(&format!("reply-body-{}", reply.id), &reply.body))
                    })),
            )
            .into_any_element()
    }

    fn inbox_agent_panel(&self, item: &WorkItem, cx: &mut Context<Self>) -> AnyElement {
        let ask = self.work_inbox.discussion_open;
        let mut panel = div()
            .w(px(440.0))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(colors().border_subtle)
            .bg(surface(colors().background))
            .when(self.settings.window_width < 1200.0, |panel| {
                panel.absolute().inset_0().w_full()
            })
            .child(
                div()
                    .h(px(44.0))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(colors().border_subtle)
                    .child(
                        svg()
                            .path(if ask {
                                "chrome-icons/comment.svg"
                            } else {
                                "chrome-icons/send.svg"
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
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(format!(
                                "{} · {}",
                                if ask { "Ask" } else { "Send to agent" },
                                item.reference
                            )),
                    )
                    .child(
                        icon_button("inbox-close-agent", "chrome-icons/close.svg", "Close panel")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.work_inbox.discussion_open = false;
                                this.work_inbox.composer_open = false;
                                this.work_inbox.composer_editing = false;
                                this.sync_terminal_surface_visibility(cx);
                                this.focus_handle.focus(window);
                                cx.notify();
                            })),
                    ),
            )
            .when_some(self.work_inbox.action_error.as_ref(), |panel, error| {
                panel.child(div().px_3().pt_2().child(message(error, true)))
            });
        if ask && let Some(pane) = self.visible_inbox_terminal() {
            return panel
                .child(
                    div()
                        .flex_1()
                        .min_h(px(0.0))
                        .child(self.terminals[&pane].clone()),
                )
                .into_any_element();
        }
        let project = self
            .work_inbox
            .target_project
            .and_then(|id| {
                self.snapshot
                    .projects
                    .iter()
                    .find(|project| project.id == id)
            })
            .map(|project| project.name.as_str())
            .unwrap_or("Choose project");
        let editing = self.work_inbox.composer_editing;
        let note = self.work_inbox.composer_note.clone();
        panel = panel.child(
            div()
                .id("inbox-agent-form")
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .p_4()
                .flex()
                .flex_col()
                .gap(px(14.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_size(px(11.5))
                                .text_color(colors().subtle)
                                .child(github_icon(12.0))
                                .child(format!("{} {}", item.repository, item.reference)),
                        )
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .child(item.title.clone()),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .child(field_label("Run in"))
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_2()
                                .child(
                                    picker_button("inbox-choose-project", project.to_owned())
                                        .on_click(cx.listener(|this, event, _, cx| {
                                            this.open_inbox_menu(InboxMenu::Projects, event, cx)
                                        })),
                                )
                                .child(
                                    picker_button(
                                        "inbox-choose-agent",
                                        AGENTS[self.work_inbox.agent].0.to_owned(),
                                    )
                                    .on_click(cx.listener(
                                        |this, event, _, cx| {
                                            this.open_inbox_menu(InboxMenu::Agents, event, cx)
                                        },
                                    )),
                                ),
                        ),
                )
                .when(!ask, |form| {
                    form.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(field_label("Instructions"))
                            .child(
                                div()
                                    .id("inbox-composer-note")
                                    .min_h(px(140.0))
                                    .p_3()
                                    .rounded(px(8.0))
                                    .border_1()
                                    .border_color(if editing {
                                        colors().accent
                                    } else {
                                        colors().border_subtle
                                    })
                                    .bg(surface_tint(colors().elevated, colors().background))
                                    .text_size(px(13.0))
                                    .text_color(if note.is_empty() {
                                        colors().subtle
                                    } else {
                                        colors().foreground
                                    })
                                    .cursor_text()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.work_inbox.composer_editing = true;
                                        this.work_inbox.comment_editing = false;
                                        this.work_inbox.search_editing = false;
                                        this.focus_handle.focus(window);
                                        cx.notify();
                                    }))
                                    .child(if note.is_empty() {
                                        "Optional. Line comments from Files land here.".into()
                                    } else {
                                        note
                                    }),
                            ),
                    )
                })
                .when(ask, |form| {
                    form.child(div().text_size(px(12.0)).text_color(colors().subtle).child(
                        concat!(
                            "Opens a conversation with the agent about this task. ",
                            "It reads GitHub and does not change your files.",
                        ),
                    ))
                })
                .child(
                    div().flex().justify_end().child(
                        header_button(
                            "inbox-confirm-send",
                            Some(if ask {
                                "chrome-icons/comment.svg"
                            } else {
                                "chrome-icons/send.svg"
                            }),
                            if ask {
                                "Open conversation"
                            } else {
                                "Send · ⌘↩"
                            },
                            true,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                if ask {
                                    this.open_inbox_discussion(window, cx);
                                } else {
                                    this.start_work_item(window, cx);
                                }
                            },
                        )),
                    ),
                ),
        );
        panel.into_any_element()
    }
}

fn avatar(name: &str, size: f32) -> gpui::Div {
    div()
        .size(px(size))
        .flex_none()
        .rounded_full()
        .bg(surface_tint(colors().selection, colors().background))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px((size * 0.45).round()))
        .text_color(colors().foreground)
        .child(
            name.chars()
                .next()
                .unwrap_or('?')
                .to_uppercase()
                .to_string(),
        )
}

fn person(name: &str) -> gpui::Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(5.0))
        .child(avatar(name, 16.0))
        .child(name.to_owned())
}

fn field_label(label: &'static str) -> impl IntoElement {
    div()
        .text_size(px(11.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(colors().muted)
        .child(label)
}

fn picker_button(id: &'static str, label: String) -> gpui::Stateful<gpui::Div> {
    header_button(id, None, label, false).child(
        svg()
            .path("chrome-icons/chevron-down.svg")
            .size(px(12.0))
            .text_color(colors().subtle),
    )
}

/// The one button style of the detail: filled for the main action, outlined
/// for the rest.
pub(super) fn header_button(
    id: impl Into<SharedString>,
    icon: Option<&'static str>,
    label: impl Into<SharedString>,
    primary: bool,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id.into())
        .h(px(28.0))
        .px(px(10.0))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(6.0))
        .rounded(px(6.0))
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
        .when_some(icon, |button, icon| {
            button.child(
                svg()
                    .path(icon)
                    .size(px(13.0))
                    .flex_none()
                    .text_color(if primary {
                        colors().background
                    } else {
                        colors().subtle
                    }),
            )
        })
        .child(label.into())
}
