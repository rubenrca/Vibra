use super::*;
use crate::domain::work_items::WorkComment;
use crate::ui::markdown::{markdown, safe_link};

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
                .child(div().text_size(px(13.0)).child("Select a task from Review"))
                .into_any_element();
        };
        let state = self.work_inbox.details.get(&item.url);
        let summary = state.and_then(|state| state.summary.data.as_ref());
        let (status, color) = status_mark(item);
        let url = item.url.clone();
        let mut header = div()
            .flex_none()
            .min_w(px(0.0))
            .px(px(32.0))
            .pt(px(24.0))
            .pb(px(20.0))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .border_b_1()
            .border_color(colors().border_subtle)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.0))
                    .text_size(px(13.0))
                    .text_color(colors().subtle)
                    .child(provider_icon(item.source, 16.0))
                    .child(if item.kind == WorkKind::PullRequest {
                        "Pull request"
                    } else {
                        "Issue"
                    })
                    .child(item.reference.clone())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .ml_1()
                            .text_color(color)
                            .child(svg().path(status).size(px(14.0)).text_color(color))
                            .child(item.state_label.clone()),
                    )
                    .child(
                        div()
                            .min_w(px(0.0))
                            .truncate()
                            .child(item.repository.clone()),
                    ),
            )
            .child(
                div()
                    .text_size(px(22.0))
                    .line_height(px(29.0))
                    .text_color(colors().foreground)
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(item.title.clone()),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.0))
                    .text_size(px(13.0))
                    .text_color(colors().subtle)
                    .when(!item.author.is_empty(), |row| {
                        row.child(person(&item.author)).child("·")
                    })
                    .when(item.assignees.is_empty(), |row| row.child("Unassigned"))
                    .children(item.assignees.iter().map(|name| person(name)))
                    .when(item.created_at > 0, |row| {
                        row.child("·").child(format!(
                            "Created {}",
                            relative_time(unix_now(), item.created_at)
                        ))
                    })
                    .child("·")
                    .child(format!(
                        "Updated {}",
                        relative_time(unix_now(), item.updated_at)
                    ))
                    .when_some(
                        summary.filter(|detail| !detail.head_ref.is_empty()),
                        |row, detail| {
                            let branch = detail.head_ref.clone();
                            row.child("·")
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .min_w(px(0.0))
                                        .child(
                                            svg()
                                                .path("chrome-icons/git-branch.svg")
                                                .size(px(14.0))
                                                .flex_none()
                                                .text_color(colors().subtle),
                                        )
                                        .child(div().min_w(px(0.0)).truncate().child(format!(
                                            "{} ← {}",
                                            detail.base_ref, detail.head_ref
                                        )))
                                        .child(
                                            icon_button(
                                                "copy-inbox-branch",
                                                "chrome-icons/copy.svg",
                                                "Copy branch name",
                                            )
                                            .on_click(
                                                move |_, _, cx| {
                                                    cx.write_to_clipboard(
                                                        ClipboardItem::new_string(branch.clone()),
                                                    )
                                                },
                                            ),
                                        ),
                                )
                                .when(!review_label(&detail.review_decision).is_empty(), |row| {
                                    row.child("·").child(review_label(&detail.review_decision))
                                })
                        },
                    ),
            );
        if let Some(panes) = self.settings.inbox.linked_sessions.get(&item.url) {
            let sessions: Vec<_> = panes
                .iter()
                .filter_map(|pane| {
                    self.snapshot
                        .terminal_sessions()
                        .find(|session| session.id == *pane)
                })
                .collect();
            if !sessions.is_empty() {
                header = header.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .text_size(px(11.0))
                        .text_color(colors().subtle)
                        .child("Related sessions")
                        .children(sessions.into_iter().map(|session| {
                            let pane = session.id;
                            quiet_button(
                                SharedString::from(format!("inbox-related-{pane}")),
                                session
                                    .agent_task_title
                                    .clone()
                                    .unwrap_or_else(|| session.title.clone()),
                            )
                            .on_click(cx.listener(
                                move |this, _, window, cx| this.open_pane(pane, window, cx),
                            ))
                        })),
                );
            }
        }
        header = header.child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .when(item.kind == WorkKind::Issue, |row| {
                    row.child(
                        section_button("inbox-send-to-agent", "Send to agent", true)
                            .on_click(cx.listener(|this, _, _, cx| this.open_inbox_composer(cx))),
                    )
                })
                .when(
                    item.kind == WorkKind::PullRequest && item.status != WorkStatus::Merged,
                    |row| {
                        row.child(
                            section_button(
                                "inbox-pr-actions",
                                if item.status == WorkStatus::Open {
                                    "Merge ▾"
                                } else {
                                    "Actions ▾"
                                },
                                true,
                            )
                            .on_click(cx.listener(
                                |this, event, _, cx| {
                                    this.open_inbox_menu(InboxMenu::PrActions, event, cx)
                                },
                            )),
                        )
                    },
                )
                .child(
                    detail_action("inbox-ask", "chrome-icons/comment.svg", "Ask", true).on_click(
                        cx.listener(|this, _, window, cx| this.open_inbox_discussion(window, cx)),
                    ),
                )
                .child(
                    detail_action(
                        "inbox-external",
                        "chrome-icons/open-external.svg",
                        if item.kind == WorkKind::PullRequest {
                            "Review on GitHub".to_owned()
                        } else {
                            format!("Open in {}", item.source.label())
                        },
                        false,
                    )
                    .on_click(move |_, _, cx| cx.open_url(&url)),
                ),
        );
        if let Some(PrConfirmation {
            url: key, action, ..
        }) = &self.work_inbox.confirmation
            && key == &item.url
        {
            let busy = state.is_some_and(|state| state.mutation_busy);
            header = header.child(
                div()
                    .p_3()
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(colors().border_subtle)
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_size(px(12.0)).child(format!(
                        "{} · {} {}",
                        action.label(),
                        item.repository,
                        item.reference
                    )))
                    .child(div().text_size(px(11.0)).text_color(colors().muted).child(
                        if matches!(
                            action,
                            PrAction::Merge | PrAction::Squash | PrAction::Rebase
                        ) {
                            "This action merges changes into the remote repository's base branch."
                        } else {
                            "This action updates the pull request status on GitHub."
                        },
                    ))
                    .when_some(
                        state.and_then(|state| state.mutation_error.as_ref()),
                        |row, error| row.child(message(error, true)),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                section_button(
                                    "inbox-confirm-action",
                                    if busy { "Applying…" } else { "Confirm" },
                                    true,
                                )
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.confirm_inbox_pr_action(cx)),
                                ),
                            )
                            .child(quiet_button("inbox-cancel-action", "Cancel").on_click(
                                cx.listener(move |this, _, _, cx| {
                                    if !busy {
                                        this.work_inbox.confirmation = None;
                                        cx.notify();
                                    }
                                }),
                            )),
                    ),
            );
        }
        if item.kind == WorkKind::PullRequest {
            let checks = state.and_then(|state| state.checks.data.as_ref());
            header = header.pb_0().child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(20.0))
                    .mt(px(4.0))
                    .h(px(44.0))
                    .children(
                        [
                            (DetailTab::Summary, "Summary"),
                            (DetailTab::Code, "Code"),
                            (DetailTab::Checks, "Checks"),
                        ]
                        .into_iter()
                        .map(|(tab, label)| {
                            let selected = self.work_inbox.detail_tab == tab;
                            div()
                                .id(SharedString::from(format!("inbox-detail-{label}")))
                                .h_full()
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .cursor_pointer()
                                .text_size(px(13.0))
                                .text_color(if selected {
                                    colors().foreground
                                } else {
                                    colors().subtle
                                })
                                .hover(|tab| tab.text_color(colors().foreground))
                                .border_b_2()
                                .border_color(if selected {
                                    colors().foreground
                                } else {
                                    gpui::rgba(0x00000000)
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.select_inbox_detail_tab(tab, cx)
                                }))
                                .child(label)
                                .when(tab == DetailTab::Checks, |tab| {
                                    tab.when_some(
                                        checks.filter(|checks| !checks.is_empty()),
                                        |tab, checks| {
                                            let failed = checks.iter().any(|check| check.failed());
                                            let passed = checks.iter().all(|check| check.passed());
                                            tab.child(checks::check_status_icon(
                                                failed, passed, 15.0,
                                            ))
                                        },
                                    )
                                })
                        }),
                    )
                    .child(div().flex_1())
                    .when(self.work_inbox.detail_tab == DetailTab::Code, |row| {
                        row.child(self.inbox_code_modes(state, cx))
                    }),
            );
        }
        let code =
            item.kind == WorkKind::PullRequest && self.work_inbox.detail_tab == DetailTab::Code;
        let mut body = div()
            .id(SharedString::from(format!(
                "inbox-detail-scroll-{}-{}",
                item.url, self.work_inbox.detail_tab as u8
            )))
            .flex_1()
            .min_h(px(0.0))
            .when(!code, |body| body.overflow_y_scroll())
            .when(code, |body| body.overflow_hidden())
            .px(px(32.0))
            .py(px(24.0))
            .flex()
            .flex_col()
            .gap_5();
        if !item.labels.is_empty()
            && (item.kind != WorkKind::PullRequest
                || self.work_inbox.detail_tab == DetailTab::Summary)
        {
            body = body.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .children(item.labels.iter().map(|label| label_chip(label))),
            );
        }
        body = body.child(match self.work_inbox.detail_tab {
            DetailTab::Code if item.kind == WorkKind::PullRequest => {
                self.inbox_diff_body(item, state, cx)
            }
            DetailTab::Checks if item.kind == WorkKind::PullRequest => {
                self.inbox_checks_body(item, state, cx)
            }
            _ => self.inbox_summary_body(item, state, cx),
        });
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

    fn inbox_summary_body(
        &self,
        item: &WorkItem,
        state: Option<&ItemState>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let detail = state.and_then(|state| state.summary.data.as_ref());
        let mut content = div().flex().flex_col().gap_5();
        if let Some(error) = state.and_then(|state| state.summary.error.as_ref()) {
            content = content.child(message(error, true));
        }
        let body = detail
            .map(|detail| detail.body.as_str())
            .unwrap_or(&item.body);
        content = content.child(crate::ui::markdown::markdown_prose(
            &format!("inbox-body-{}", item.url),
            if body.trim().is_empty() {
                "No description."
            } else {
                body
            },
        ));
        if state.is_some_and(|state| state.summary.loading) {
            content = content.child(message("Refreshing conversation…", false));
        }
        if let Some(detail) = detail {
            if !detail.comments.is_empty() {
                content = content.child(
                    div()
                        .pt_5()
                        .border_t_1()
                        .border_color(colors().border_subtle)
                        .text_size(px(12.0))
                        .text_color(colors().muted)
                        .child(format!("{} comments", detail.comments.len())),
                );
            }
            for comment in &detail.comments {
                content = content.child(self.inbox_comment_card(&item.url, comment, cx));
            }
            if detail.truncated {
                content = content.child(message(
                    concat!(
                        "Showing recent comments. ",
                        "The full conversation is available on the source service.",
                    ),
                    false,
                ));
            }
        }
        let draft = state.map(|state| state.draft.clone()).unwrap_or_default();
        let busy = state.is_some_and(|state| state.posting);
        content = content.child(
            div()
                .pt_5()
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
                                .child(format!("Reply to {author}"))
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
                        .min_h(px(76.0))
                        .max_h(px(180.0))
                        .overflow_y_scroll()
                        .p_3()
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(if self.work_inbox.comment_editing {
                            colors().accent
                        } else {
                            colors().border_subtle
                        })
                        .bg(surface_tint(colors().elevated, colors().background))
                        .text_size(px(13.0))
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
                            draft
                        }),
                )
                .when_some(
                    state.and_then(|state| state.post_error.as_ref()),
                    |form, error| form.child(message(error, true)),
                )
                .child(
                    div().flex().justify_end().child(
                        section_button(
                            "inbox-post-comment",
                            if busy {
                                "Posting…"
                            } else {
                                "Post comment · ⌘↩"
                            },
                            false,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.post_inbox_comment(cx))),
                    ),
                ),
        );
        content.into_any_element()
    }

    fn inbox_comment_card(
        &self,
        url: &str,
        comment: &WorkComment,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = url.to_owned();
        let reply = comment.reply_id.clone();
        let author = comment.author.clone();
        div()
            .p_3()
            .rounded(px(6.0))
            .border_1()
            .border_color(colors().border_subtle)
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.0))
                    .text_color(colors().muted)
                    .child(person(&comment.author))
                    .child(short_time(comment.at))
                    .when(!comment.context.is_empty(), |row| {
                        row.child(comment.context.clone())
                    })
                    .child(div().flex_1())
                    .when_some(reply, |row, reply| {
                        row.child(
                            quiet_button(
                                SharedString::from(format!("reply-{}", comment.id)),
                                "Reply",
                            )
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
                    }),
            )
            .child(markdown(&format!("comment-{}", comment.id), &comment.body))
            .when(safe_link(&comment.url), |card| {
                let link = comment.url.clone();
                card.child(
                    quiet_button(
                        SharedString::from(format!("comment-link-{}", comment.id)),
                        "View comment ↗",
                    )
                    .on_click(move |_, _, cx| cx.open_url(&link)),
                )
            })
            .children(comment.replies.iter().map(|reply| {
                div()
                    .pl_4()
                    .border_l_2()
                    .border_color(colors().border_subtle)
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(person(&reply.author))
                    .child(markdown(&format!("reply-body-{}", reply.id), &reply.body))
            }))
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
                        div()
                            .flex_1()
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
                panel.child(message(error, true))
            });
        if ask && let Some(pane) = self.visible_inbox_terminal() {
            panel = panel.child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .child(self.terminals[&pane].clone()),
            );
        } else {
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
            panel = panel.child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .p_3()
                            .rounded(px(7.0))
                            .border_1()
                            .border_color(colors().border_subtle)
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_size(px(12.0))
                                    .child(provider_icon(item.source, 16.0))
                                    .child(item.reference.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .child(item.title.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(colors().muted)
                                    .child(item.repository.clone()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(
                                section_button(
                                    "inbox-choose-project",
                                    format!("{project} ▾"),
                                    false,
                                )
                                .on_click(cx.listener(
                                    |this, event, _, cx| {
                                        this.open_inbox_menu(InboxMenu::Projects, event, cx)
                                    },
                                )),
                            )
                            .child(
                                section_button(
                                    "inbox-choose-agent",
                                    format!("{} ▾", AGENTS[self.work_inbox.agent].0),
                                    false,
                                )
                                .on_click(cx.listener(
                                    |this, event, _, cx| {
                                        this.open_inbox_menu(InboxMenu::Agents, event, cx)
                                    },
                                )),
                            ),
                    )
                    .when(!ask, |panel| {
                        panel.child(
                            div()
                                .id("inbox-composer-note")
                                .min_h(px(120.0))
                                .p_3()
                                .rounded(px(6.0))
                                .border_1()
                                .border_color(if self.work_inbox.composer_editing {
                                    colors().accent
                                } else {
                                    colors().border_subtle
                                })
                                .text_size(px(13.0))
                                .cursor_text()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.work_inbox.composer_editing = true;
                                    this.work_inbox.comment_editing = false;
                                    this.work_inbox.search_editing = false;
                                    this.focus_handle.focus(window);
                                    cx.notify();
                                }))
                                .child(if self.work_inbox.composer_note.is_empty() {
                                    "Add instructions for the agent…".into()
                                } else {
                                    self.work_inbox.composer_note.clone()
                                }),
                        )
                    })
                    .child(
                        div().flex().justify_end().child(
                            section_button(
                                "inbox-confirm-send",
                                if ask { "Open conversation" } else { "Send" },
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
        }
        panel.into_any_element()
    }
}

fn person(name: &str) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(px(5.0))
        .text_size(px(12.0))
        .text_color(colors().muted)
        .child(
            div()
                .size(px(18.0))
                .flex_none()
                .rounded_full()
                .bg(surface_tint(colors().selection, colors().background))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(9.0))
                .child(
                    name.chars()
                        .next()
                        .unwrap_or('?')
                        .to_uppercase()
                        .to_string(),
                ),
        )
        .child(name.to_owned())
}

fn review_label(value: &str) -> &'static str {
    match value {
        "APPROVED" => "Approved",
        "CHANGES_REQUESTED" => "Changes requested",
        "REVIEW_REQUIRED" => "Review required",
        _ => "",
    }
}

fn detail_action(
    id: &'static str,
    icon: &'static str,
    label: impl Into<SharedString>,
    bordered: bool,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(32.0))
        .px(px(12.0))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(7.0))
        .rounded(px(7.0))
        .text_size(px(13.0))
        .text_color(colors().muted)
        .cursor_pointer()
        .when(bordered, |button| {
            button.border_1().border_color(colors().border_subtle)
        })
        .hover(|button| {
            button
                .bg(surface_tint(colors().hover, colors().background))
                .text_color(colors().foreground)
        })
        .child(svg().path(icon).size(px(16.0)).text_color(colors().muted))
        .child(label.into())
}
