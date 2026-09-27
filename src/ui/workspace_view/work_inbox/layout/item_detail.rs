use super::*;
use crate::domain::work_items::WorkComment;
use crate::ui::markdown::{markdown, safe_link};
use crate::ui::theme::MONO_FONT;

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
                .child(
                    div()
                        .text_size(px(13.0))
                        .child("Selecciona una tarea del Inbox"),
                )
                .into_any_element();
        };
        let state = self.work_inbox.details.get(&item.url);
        let summary = state.and_then(|state| state.summary.data.as_ref());
        let (status, color) = status_mark(item);
        let url = item.url.clone();
        let mut header = div()
            .flex_none()
            .px(px(32.0))
            .pt_5()
            .pb_4()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .border_b_1()
            .border_color(colors().border_subtle)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.0))
                    .text_color(colors().subtle)
                    .child(provider_icon(item.source, 14.0))
                    .child(item.repository.clone())
                    .child("/")
                    .child(svg().path(status).size(px(14.0)).text_color(color))
                    .child(item.reference.clone())
                    .child(item.state_label.clone()),
            )
            .child(
                div()
                    .max_h(px(50.0))
                    .overflow_hidden()
                    .text_size(px(20.0))
                    .line_height(px(25.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(item.title.clone()),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.0))
                    .text_color(colors().subtle)
                    .when(!item.author.is_empty(), |row| {
                        row.child(person(&item.author))
                    })
                    .children(
                        item.assignees
                            .iter()
                            .filter(|name| *name != &item.author)
                            .map(|name| person(name)),
                    )
                    .when(item.assignees.is_empty(), |row| row.child("· Sin asignar"))
                    .when(item.created_at > 0, |row| {
                        row.child(format!(
                            "· Creado {}",
                            relative_time(unix_now(), item.created_at)
                        ))
                    })
                    .child(format!(
                        "· Actualizado {}",
                        relative_time(unix_now(), item.updated_at)
                    )),
            )
            .when_some(
                summary.filter(|detail| !detail.head_ref.is_empty()),
                |header, detail| {
                    let branch = detail.head_ref.clone();
                    header.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(px(12.0))
                            .text_color(colors().muted)
                            .child(
                                svg()
                                    .path("chrome-icons/git-branch.svg")
                                    .size(px(13.0))
                                    .text_color(colors().muted),
                            )
                            .child(format!("{} ← {}", detail.base_ref, detail.head_ref))
                            .child(quiet_button("copy-inbox-branch", "Copiar").on_click(
                                move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(branch.clone()))
                                },
                            ))
                            .child(review_label(&detail.review_decision)),
                    )
                },
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
                        .child("Sesiones relacionadas")
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
                        section_button("inbox-send-to-agent", "Enviar al agente", true)
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
                                    "Acciones ▾"
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
                .child(section_button("inbox-ask", "Ask", false).on_click(
                    cx.listener(|this, _, window, cx| this.open_inbox_discussion(window, cx)),
                ))
                .child(
                    quiet_button(
                        "inbox-external",
                        format!("Abrir en {} ↗", item.source.label()),
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
            header = header.child(div().p_3().rounded(px(6.0)).border_1().border_color(colors().border_subtle).flex().flex_col().gap_2()
                .child(div().text_size(px(12.0)).child(format!("{} · {} {}", action.label(), item.repository, item.reference)))
                .child(div().text_size(px(11.0)).text_color(colors().muted).child(
                    if matches!(action, PrAction::Merge | PrAction::Squash | PrAction::Rebase) {
                        "Esta acción integra los cambios en la rama base del repositorio remoto."
                    } else { "Esta acción actualiza el estado del pull request en GitHub." }))
                .when_some(state.and_then(|state| state.mutation_error.as_ref()), |row, error| row.child(message(error, true)))
                .child(div().flex().gap_2()
                    .child(section_button("inbox-confirm-action", if busy { "Aplicando…" } else { "Confirmar" }, true)
                        .on_click(cx.listener(|this, _, _, cx| this.confirm_inbox_pr_action(cx))))
                    .child(quiet_button("inbox-cancel-action", "Cancelar")
                        .on_click(cx.listener(move |this, _, _, cx| { if !busy { this.work_inbox.confirmation = None; cx.notify(); } })))));
        }
        if item.kind == WorkKind::PullRequest {
            header = header.pb_0().child(
                div().flex().gap_4().h(px(36.0)).children(
                    [
                        (DetailTab::Summary, "Summary"),
                        (DetailTab::Code, "Code"),
                        (DetailTab::Checks, "Checks"),
                    ]
                    .into_iter()
                    .map(|(tab, label)| {
                        let selected = self.work_inbox.detail_tab == tab;
                        let failures = state
                            .and_then(|state| state.checks.data.as_ref())
                            .map(|checks| checks.iter().filter(|check| check.failed()).count())
                            .unwrap_or_default();
                        div()
                            .id(SharedString::from(format!("inbox-detail-{label}")))
                            .h_full()
                            .flex()
                            .items_center()
                            .gap_1()
                            .cursor_pointer()
                            .text_size(px(12.0))
                            .text_color(if selected {
                                colors().foreground
                            } else {
                                colors().muted
                            })
                            .when(selected, |tab| {
                                tab.border_b_2().border_color(colors().foreground)
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select_inbox_detail_tab(tab, cx)
                            }))
                            .child(label)
                            .when(tab == DetailTab::Checks && failures > 0, |tab| {
                                tab.child(
                                    div()
                                        .text_color(colors().danger)
                                        .child(format!("⊗ {failures}")),
                                )
                            })
                    }),
                ),
            );
        }
        let mut body = div()
            .id(SharedString::from(format!(
                "inbox-detail-scroll-{}",
                item.url
            )))
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .px(px(32.0))
            .py_5()
            .flex()
            .flex_col()
            .gap_5();
        if !item.labels.is_empty() {
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
        content = content.child(markdown(
            &format!("inbox-body-{}", item.url),
            if body.trim().is_empty() {
                "Sin descripción."
            } else {
                body
            },
        ));
        if state.is_some_and(|state| state.summary.loading) {
            content = content.child(message("Actualizando conversación…", false));
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
                        .child(format!("{} comentarios", detail.comments.len())),
                );
            }
            for comment in &detail.comments {
                content = content.child(self.inbox_comment_card(&item.url, comment, cx));
            }
            if detail.truncated {
                content = content.child(message("Se muestran los comentarios recientes. La conversación completa está en el servicio de origen.", false));
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
                                .child(format!("Responder a {author}"))
                                .child(quiet_button("inbox-cancel-reply", "Cancelar").on_click(
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
                            "Escribe un comentario…".into()
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
                                "Publicando…"
                            } else {
                                "Publicar comentario · ⌘↩"
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
                                "Responder",
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
                        "Ver comentario ↗",
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

    fn inbox_diff_body(
        &self,
        item: &WorkItem,
        state: Option<&ItemState>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(diff) = state.and_then(|state| state.diff.data.as_ref()) else {
            return message(
                state
                    .and_then(|state| state.diff.error.as_deref())
                    .unwrap_or("Cargando archivos del PR…"),
                state.is_some_and(|state| state.diff.error.is_some()),
            )
            .into_any_element();
        };
        let mut body = div().flex().flex_col().gap_3().child(
            div()
                .text_size(px(12.0))
                .text_color(colors().muted)
                .child(format!(
                    "{} archivos · +{} −{}",
                    diff.files.len(),
                    diff.files.iter().map(|f| f.additions).sum::<u64>(),
                    diff.files.iter().map(|f| f.deletions).sum::<u64>()
                )),
        );
        for file in &diff.files {
            let key = item.url.clone();
            let path = file.path.clone();
            let collapsed = state.is_some_and(|state| state.collapsed_files.contains(&path));
            let mut card = div()
                .rounded(px(6.0))
                .border_1()
                .border_color(colors().border_subtle)
                .overflow_hidden()
                .flex()
                .flex_col()
                .child(
                    div()
                        .id(SharedString::from(format!("pr-file-{}", file.path)))
                        .px_3()
                        .h(px(36.0))
                        .flex()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        .bg(surface_tint(colors().elevated, colors().background))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let files = &mut this
                                .work_inbox
                                .details
                                .entry(key.clone())
                                .or_default()
                                .collapsed_files;
                            if !files.remove(&path) {
                                files.insert(path.clone());
                            }
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
                                .min_w(px(0.0))
                                .truncate()
                                .font_family(MONO_FONT)
                                .text_size(px(12.0))
                                .child(
                                    file.previous_path
                                        .as_ref()
                                        .map(|old| format!("{old} → {}", file.path))
                                        .unwrap_or(file.path.clone()),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(colors().success)
                                .child(format!("+{}", file.additions)),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(colors().danger)
                                .child(format!("−{}", file.deletions)),
                        ),
                );
            if !collapsed {
                if file.patch.is_empty() {
                    card = card.child(message(
                        "Sin patch textual disponible (archivo binario o cambio demasiado grande).",
                        false,
                    ));
                } else {
                    let rows = std::sync::Arc::new(patch_rows(&file.patch));
                    let count = rows.len();
                    card = card.child(
                        gpui::uniform_list(
                            SharedString::from(format!("pr-diff-lines-{}", file.path)),
                            count,
                            move |range, _, _| {
                                range
                                    .map(|index| {
                                        let row = &rows[index];
                                        div()
                                            .h(px(19.0))
                                            .px_2()
                                            .flex()
                                            .gap_2()
                                            .font_family(MONO_FONT)
                                            .text_size(px(11.5))
                                            .whitespace_nowrap()
                                            .when(row.kind == '+', |line| {
                                                line.bg(colors().diff_added_bg)
                                            })
                                            .when(row.kind == '-', |line| {
                                                line.bg(colors().diff_deleted_bg)
                                            })
                                            .child(
                                                div()
                                                    .w(px(35.0))
                                                    .flex_none()
                                                    .text_right()
                                                    .text_color(colors().subtle)
                                                    .child(
                                                        row.old
                                                            .map(|n| n.to_string())
                                                            .unwrap_or_default(),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .w(px(35.0))
                                                    .flex_none()
                                                    .text_right()
                                                    .text_color(colors().subtle)
                                                    .child(
                                                        row.new
                                                            .map(|n| n.to_string())
                                                            .unwrap_or_default(),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .text_color(if row.kind == '@' {
                                                        colors().accent
                                                    } else {
                                                        colors().foreground
                                                    })
                                                    .child(row.text.clone()),
                                            )
                                    })
                                    .collect()
                            },
                        )
                        .h(px((count as f32 * 19.0).min(600.0)))
                        .min_w(px(0.0)),
                    );
                }
            }
            body = body.child(card);
        }
        if diff.truncated {
            body = body.child(message(
                "La lista de archivos está truncada. Revisa el resto en GitHub.",
                true,
            ));
        }
        body.into_any_element()
    }

    fn inbox_checks_body(
        &self,
        item: &WorkItem,
        state: Option<&ItemState>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut body = div().flex().flex_col().gap_2().child(
            div()
                .flex()
                .items_center()
                .child(div().flex_1().text_size(px(13.0)).child("Checks"))
                .child(
                    icon_button(
                        "inbox-refresh-checks",
                        "chrome-icons/refresh.svg",
                        "Actualizar checks",
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.load_inbox_detail(true, cx))),
                ),
        );
        if let Some(error) = state.and_then(|state| state.checks.error.as_ref()) {
            body = body.child(message(error, true));
        }
        let Some(checks) = state.and_then(|state| state.checks.data.as_ref()) else {
            return body
                .child(message("Cargando checks…", false))
                .into_any_element();
        };
        if checks.is_empty() {
            body = body.child(message("Este PR no tiene checks registrados.", false));
        }
        for (index, check) in checks.iter().enumerate() {
            let color = if check.failed() {
                colors().danger
            } else if check.passed() {
                colors().success
            } else {
                colors().warning
            };
            let url = check.url.clone();
            let key = format!("{}:{}", check.name, check.url);
            let expanded = state.is_some_and(|state| state.expanded_checks.contains(&key));
            let has_job = work_items::github_check_job(item, check).is_some();
            let can_fix = check.failed()
                && state.is_some_and(|state| {
                    !state.checks.loading
                        && state.checks.error.is_none()
                        && state.summary.data.as_ref().is_some_and(|detail| {
                            !detail.head_oid.is_empty() && detail.head_oid == check.head_oid
                        })
                });
            let log_check = check.clone();
            let fix_check = check.clone();
            let mut card = div()
                .rounded(px(6.0))
                .border_1()
                .border_color(colors().border_subtle)
                .flex()
                .flex_col()
                .child(
                    div()
                        .min_h(px(42.0))
                        .px_3()
                        .py_2()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            svg()
                                .path(if check.failed() {
                                    "chrome-icons/issue-canceled.svg"
                                } else if check.passed() {
                                    "chrome-icons/issue-closed.svg"
                                } else {
                                    "chrome-icons/issue-open.svg"
                                })
                                .size(px(16.0))
                                .text_color(color),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .text_size(px(12.0))
                                .child(check.name.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(color)
                                .child(check.state.clone()),
                        )
                        .when(has_job, |row| {
                            row.child(
                                quiet_button(
                                    SharedString::from(format!("check-log-{index}")),
                                    if expanded {
                                        "Ocultar registro"
                                    } else {
                                        "Registro"
                                    },
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.toggle_inbox_check(log_check.clone(), cx)
                                    },
                                )),
                            )
                        })
                        .when(can_fix, |row| {
                            row.child(
                                quiet_button(
                                    SharedString::from(format!("check-fix-{index}")),
                                    "Fix",
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.prepare_inbox_check_fix(&fix_check, cx)
                                    },
                                )),
                            )
                        })
                        .when(safe_link(&url), |row| {
                            row.child(
                                quiet_button(
                                    SharedString::from(format!("check-link-{index}")),
                                    "Ver ↗",
                                )
                                .on_click(move |_, _, cx| cx.open_url(&url)),
                            )
                        }),
                );
            if expanded {
                let log = state.and_then(|state| state.check_logs.get(&key));
                if let Some(error) = log.and_then(|log| log.error.as_ref()) {
                    card = card.child(message(error, true));
                }
                if let Some(text) = log.and_then(|log| log.data.as_ref()) {
                    let rows: std::sync::Arc<Vec<String>> =
                        std::sync::Arc::new(text.lines().map(str::to_owned).collect());
                    let count = rows.len();
                    card = card.child(
                        gpui::uniform_list(
                            SharedString::from(format!("check-log-lines-{index}")),
                            count,
                            move |range, _, _| {
                                range
                                    .map(|index| {
                                        div()
                                            .h(px(18.0))
                                            .px_3()
                                            .font_family(MONO_FONT)
                                            .text_size(px(11.0))
                                            .whitespace_nowrap()
                                            .child(rows[index].clone())
                                    })
                                    .collect()
                            },
                        )
                        .h(px((count as f32 * 18.0).clamp(36.0, 360.0)))
                        .min_w(px(0.0)),
                    );
                } else if log.is_none_or(|log| log.loading) {
                    card = card.child(message("Cargando registro…", false));
                }
            }
            body = body.child(card);
        }
        body.into_any_element()
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
                                if ask { "Ask" } else { "Enviar al agente" },
                                item.reference
                            )),
                    )
                    .child(
                        icon_button(
                            "inbox-close-agent",
                            "chrome-icons/close.svg",
                            "Cerrar panel",
                        )
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
                .unwrap_or("Elegir proyecto");
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
                                    "Añade instrucciones para el agente…".into()
                                } else {
                                    self.work_inbox.composer_note.clone()
                                }),
                        )
                    })
                    .child(
                        div().flex().justify_end().child(
                            section_button(
                                "inbox-confirm-send",
                                if ask { "Abrir conversación" } else { "Enviar" },
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
                .size(px(16.0))
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
        "APPROVED" => "Aprobado",
        "CHANGES_REQUESTED" => "Cambios solicitados",
        "REVIEW_REQUIRED" => "Revisión pendiente",
        _ => "",
    }
}

struct PatchRow {
    old: Option<u64>,
    new: Option<u64>,
    kind: char,
    text: String,
}
fn patch_rows(patch: &str) -> Vec<PatchRow> {
    let mut old = 0;
    let mut new = 0;
    patch
        .lines()
        .map(|text| {
            let kind = text.chars().next().unwrap_or(' ');
            if text.starts_with("@@") {
                let parts: Vec<_> = text.split_whitespace().collect();
                old = parts
                    .get(1)
                    .and_then(|part| part.trim_start_matches('-').split(',').next()?.parse().ok())
                    .unwrap_or_default();
                new = parts
                    .get(2)
                    .and_then(|part| part.trim_start_matches('+').split(',').next()?.parse().ok())
                    .unwrap_or_default();
                return PatchRow {
                    old: None,
                    new: None,
                    kind: '@',
                    text: text.into(),
                };
            }
            let (left, right) = match kind {
                '-' => {
                    let value = old;
                    old += 1;
                    (Some(value), None)
                }
                '+' => {
                    let value = new;
                    new += 1;
                    (None, Some(value))
                }
                ' ' => {
                    let value = (Some(old), Some(new));
                    old += 1;
                    new += 1;
                    value
                }
                _ => (None, None),
            };
            PatchRow {
                old: left,
                new: right,
                kind,
                text: text.into(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn patch_numbers_follow_hunks_and_do_not_count_eof_markers() {
        let rows = patch_rows("@@ -4,2 +8,2 @@\n same\n-old\n+new\n\\ No newline at end of file");
        assert_eq!((rows[1].old, rows[1].new), (Some(4), Some(8)));
        assert_eq!((rows[2].old, rows[2].new), (Some(5), None));
        assert_eq!((rows[3].old, rows[3].new), (None, Some(9)));
        assert_eq!((rows[4].old, rows[4].new), (None, None));
    }
}
