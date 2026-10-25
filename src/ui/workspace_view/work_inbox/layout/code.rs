use super::*;
use crate::domain::work_items::{FileViewed, ReviewVerdict};

impl WorkspaceView {
    /// Scope and display mode of the Files tab, and the GitHub review.
    fn inbox_code_toolbar(&self, state: Option<&ItemState>, cx: &mut Context<Self>) -> AnyElement {
        let mode = state.map(|state| state.code_mode).unwrap_or_default();
        let scope = state.map(|state| state.code_scope).unwrap_or_default();
        let since = self
            .work_inbox
            .selected
            .as_deref()
            .and_then(|url| self.inbox_since_base(url));
        let pending = self.pending_review_comments(cx).len();
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
                segmented_control().children(
                    [
                        (CodeScope::All, "All changes".to_owned()),
                        (
                            CodeScope::SinceReview,
                            match &since {
                                Some(oid) => {
                                    format!("Since {}", oid.chars().take(7).collect::<String>())
                                }
                                None => "Since last review".to_owned(),
                            },
                        ),
                    ]
                    .into_iter()
                    .map(|(value, label)| {
                        let enabled = value == CodeScope::All || since.is_some();
                        segment(
                            SharedString::from(format!("inbox-code-scope-{value:?}")),
                            label,
                            scope == value,
                        )
                        .when(!enabled, |segment| segment.opacity(0.45))
                        .tooltip(move |_, cx| {
                            sidebar_tooltip(
                                match (value, enabled) {
                                    (CodeScope::All, _) => "Every change in the pull request",
                                    (_, true) => "Only what changed after your last review",
                                    (_, false) => "Available after your first review or a new push",
                                },
                                cx,
                            )
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if enabled {
                                this.set_inbox_code_scope(value, cx)
                            }
                        }))
                    }),
                ),
            )
            .child(
                segmented_control().children(
                    [
                        (CodeMode::Hunks, "Hunks"),
                        (CodeMode::FullFile, "Full file"),
                    ]
                    .into_iter()
                    .map(|(value, label)| {
                        segment(
                            SharedString::from(format!("inbox-code-mode-{label}")),
                            label,
                            mode == value,
                        )
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.set_inbox_code_mode(value, cx)),
                        )
                    }),
                ),
            )
            .child(div().flex_1())
            .child(
                super::item_detail::header_button(
                    "inbox-review-toggle",
                    Some("chrome-icons/github.svg"),
                    if pending > 0 {
                        format!("Review · {pending}")
                    } else {
                        "Review".to_owned()
                    },
                    false,
                )
                .tooltip(|_, cx| {
                    sidebar_tooltip("Comment, approve, or request changes on GitHub", cx)
                })
                .child(
                    svg()
                        .path("chrome-icons/chevron-down.svg")
                        .size(px(12.0))
                        .text_color(colors().subtle),
                )
                .on_click(cx.listener(|this, event, window, cx| {
                    this.open_inbox_menu(InboxMenu::Review, event, cx);
                    if this.work_inbox.menu == Some(InboxMenu::Review)
                        && let Some(url) = this.work_inbox.selected.clone()
                    {
                        let draft =
                            &mut this.work_inbox.details.entry(url).or_default().review_draft;
                        draft.open = true;
                        draft.editing = true;
                        this.focus_handle.focus(window);
                    }
                    cx.notify();
                })),
            )
            .into_any_element()
    }

    pub(super) fn inbox_diff_body(
        &self,
        _item: &WorkItem,
        state: Option<&ItemState>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut body = div()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .flex()
            .flex_col()
            .child(self.inbox_code_toolbar(state, cx));
        let since = state.is_some_and(|state| state.code_scope == CodeScope::SinceReview);
        let load = state.map(|state| if since { &state.since } else { &state.diff });
        let notices: Vec<AnyElement> = [
            state.and_then(|state| state.viewed_error.clone()),
            load.and_then(|load| load.error.clone()),
            load.and_then(|load| load.data.as_ref())
                .filter(|diff| diff.truncated)
                .map(|_| "The file list is truncated. View the rest on GitHub.".to_owned()),
        ]
        .into_iter()
        .flatten()
        .map(|text| message(&text, true).into_any_element())
        .collect();
        if !notices.is_empty() {
            body = body.child(div().flex_none().px_2().py_1().children(notices));
        }
        let view = state.and_then(|state| {
            if since {
                state.since_review.clone()
            } else {
                state.review.clone()
            }
        });
        let diff = load.and_then(|load| load.data.as_ref());
        match (view, diff) {
            (Some(view), Some(diff)) => {
                body = body.child(
                    div()
                        .flex_1()
                        .min_h(px(0.0))
                        .flex()
                        .child(self.inbox_file_list(state, diff, &view, cx))
                        .child(div().flex_1().min_w(px(0.0)).h_full().child(view)),
                );
            }
            _ if load.is_some_and(|load| load.error.is_some()) => {}
            _ => {
                body = body.child(div().p_3().child(message(
                    if since {
                        "Loading changes since your last review…"
                    } else {
                        "Loading pull request files…"
                    },
                    false,
                )))
            }
        }
        body.into_any_element()
    }

    /// Changed files with their size, conversations, and "Viewed" marks.
    fn inbox_file_list(
        &self,
        state: Option<&ItemState>,
        diff: &WorkDiff,
        view: &gpui::Entity<crate::ui::diff_view::DiffView>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let detail = state.and_then(|state| state.summary.data.as_ref());
        let viewed_state = |path: &str| {
            state
                .and_then(|state| state.viewed_pending.get(path).copied())
                .map(|viewed| {
                    if viewed {
                        FileViewed::Viewed
                    } else {
                        FileViewed::Unviewed
                    }
                })
                .or_else(|| detail.and_then(|detail| detail.viewed.get(path).copied()))
        };
        let viewed = diff
            .files
            .iter()
            .filter(|file| viewed_state(&file.path) == Some(FileViewed::Viewed))
            .count();
        let total = diff.files.len();
        let current = view.read(cx).current_path();
        let progress = if total == 0 {
            0.0
        } else {
            viewed as f32 / total as f32
        };
        div()
            .id("inbox-code-files")
            .w(px(240.0))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(colors().border_subtle)
            .bg(surface_tint(colors().panel, colors().background))
            .child(
                div()
                    .h(px(32.0))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(colors().border_subtle)
                    .text_size(px(11.0))
                    .text_color(colors().muted)
                    .child(div().flex_none().child(format!("{viewed}/{total} viewed")))
                    .child(
                        div()
                            .flex_1()
                            .h(px(3.0))
                            .rounded_full()
                            .bg(colors().border_subtle)
                            .child(
                                div()
                                    .h_full()
                                    .w(gpui::relative(progress))
                                    .rounded_full()
                                    .bg(colors().success),
                            ),
                    ),
            )
            .child(
                div()
                    .id("inbox-code-files-scroll")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .py_1()
                    .children(diff.files.iter().map(|file| {
                        let path = file.path.clone();
                        let (name, parent) = match path.rsplit_once('/') {
                            Some((parent, name)) => (name.to_owned(), parent.to_owned()),
                            None => (path.clone(), String::new()),
                        };
                        let file_state = viewed_state(&file.path);
                        let is_viewed = file_state == Some(FileViewed::Viewed);
                        let threads = detail.map_or(0, |detail| {
                            detail
                                .threads
                                .iter()
                                .filter(|thread| thread.path == file.path && !thread.resolved)
                                .count()
                        });
                        let selected = current.as_deref() == Some(file.path.as_str());
                        let reveal = view.clone();
                        let toggle = view.clone();
                        let toggle_path = path.clone();
                        div()
                            .id(SharedString::from(format!("inbox-code-file-{path}")))
                            .h(px(26.0))
                            .px_2()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .cursor_pointer()
                            .when(selected, |row| {
                                row.bg(surface_tint(colors().selection, colors().background))
                            })
                            .hover(|row| row.bg(surface_tint(colors().hover, colors().background)))
                            .tooltip({
                                let path = path.clone();
                                move |_, cx| sidebar_tooltip(path.clone(), cx)
                            })
                            .on_click(cx.listener(move |_, _, _, cx| {
                                reveal.update(cx, |view, cx| view.reveal_path(&path, cx));
                            }))
                            .child(
                                div()
                                    .id(SharedString::from(format!(
                                        "inbox-code-viewed-{toggle_path}"
                                    )))
                                    .size(px(14.0))
                                    .flex_none()
                                    .rounded(px(3.0))
                                    .border_1()
                                    .border_color(match file_state {
                                        Some(FileViewed::Viewed) => colors().success,
                                        Some(FileViewed::Dismissed) => colors().warning,
                                        _ => colors().subtle,
                                    })
                                    .when(is_viewed, |box_| box_.bg(colors().success))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .tooltip(move |_, cx| {
                                        sidebar_tooltip(
                                            if is_viewed {
                                                "Mark as not viewed (V)"
                                            } else {
                                                "Mark as viewed (V)"
                                            },
                                            cx,
                                        )
                                    })
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.stop_propagation();
                                        toggle.update(cx, |view, cx| {
                                            view.toggle_viewed(&toggle_path, cx)
                                        });
                                    }))
                                    .when(is_viewed, |box_| {
                                        box_.child(
                                            svg()
                                                .path("chrome-icons/check.svg")
                                                .size(px(10.0))
                                                .text_color(colors().background),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .min_w(px(0.0))
                                    .flex_1()
                                    .flex()
                                    .items_baseline()
                                    .gap(px(5.0))
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .min_w(px(0.0))
                                            .truncate()
                                            .text_size(px(12.0))
                                            .text_color(if is_viewed {
                                                colors().subtle
                                            } else {
                                                colors().foreground
                                            })
                                            .child(name),
                                    )
                                    .when(!parent.is_empty(), |row| {
                                        row.child(
                                            div()
                                                .min_w(px(0.0))
                                                .flex_shrink()
                                                .truncate()
                                                .text_size(px(10.5))
                                                .text_color(colors().subtle)
                                                .child(parent),
                                        )
                                    }),
                            )
                            .when(threads > 0, |row| {
                                row.child(
                                    div()
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .gap(px(2.0))
                                        .text_size(px(10.0))
                                        .text_color(colors().accent)
                                        .child(
                                            svg()
                                                .path("chrome-icons/comment.svg")
                                                .size(px(10.0))
                                                .text_color(colors().accent),
                                        )
                                        .child(threads.to_string()),
                                )
                            })
                            .child(line_counts(file.additions, file.deletions))
                    })),
            )
    }

    /// The GitHub review being written: summary, verdict, and inline comments.
    pub(super) fn inbox_review_popover(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let url = self.work_inbox.selected.as_ref()?;
        let draft = &self.work_inbox.details.get(url)?.review_draft;
        if !draft.open {
            return None;
        }
        let comments = self.pending_review_comments(cx).len();
        Some(
            div()
                .p(px(10.0))
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child("Finish your review"),
                )
                .child(
                    div()
                        .id("inbox-review-body")
                        .min_h(px(84.0))
                        .max_h(px(200.0))
                        .overflow_y_scroll()
                        .p_2()
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(if draft.editing {
                            colors().accent
                        } else {
                            colors().border_subtle
                        })
                        .bg(surface_tint(colors().elevated, colors().background))
                        .text_size(px(13.0))
                        .text_color(if draft.body.is_empty() {
                            colors().subtle
                        } else {
                            colors().foreground
                        })
                        .cursor_text()
                        .on_click(cx.listener(|this, _, window, cx| {
                            if let Some(url) = this.work_inbox.selected.clone() {
                                this.work_inbox
                                    .details
                                    .entry(url)
                                    .or_default()
                                    .review_draft
                                    .editing = true;
                                this.work_inbox.search_editing = false;
                                this.work_inbox.comment_editing = false;
                                this.work_inbox.composer_editing = false;
                                this.focus_handle.focus(window);
                            }
                            cx.notify();
                        }))
                        .child(if draft.body.is_empty() {
                            "Leave a summary (optional)".to_owned()
                        } else {
                            draft.body.clone()
                        }),
                )
                .child(div().flex().child(segmented_control().children(
                    ReviewVerdict::ALL.into_iter().map(|verdict| {
                        segment(
                            SharedString::from(format!("inbox-review-verdict-{verdict:?}")),
                            verdict.label(),
                            draft.verdict == verdict,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(url) = this.work_inbox.selected.clone() {
                                this.work_inbox
                                    .details
                                    .entry(url)
                                    .or_default()
                                    .review_draft
                                    .verdict = verdict;
                            }
                            cx.notify();
                        }))
                    }),
                )))
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(colors().muted)
                        .child(match comments {
                            0 => {
                                "No line comments. Use + on a line in Files to add one.".to_owned()
                            }
                            1 => "1 line comment will be posted with it.".to_owned(),
                            count => format!("{count} line comments will be posted with it."),
                        }),
                )
                .when_some(draft.error.as_ref(), |panel, error| {
                    panel.child(message(error, true))
                })
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            quiet_button("inbox-review-close", "Cancel").on_click(cx.listener(
                                |this, _, _, cx| {
                                    if this.dismiss_inbox_menu() {
                                        cx.notify();
                                    }
                                },
                            )),
                        )
                        .child(
                            super::item_detail::header_button(
                                "inbox-review-submit",
                                None,
                                if draft.busy {
                                    "Submitting…"
                                } else {
                                    "Submit review · ⌘↩"
                                },
                                true,
                            )
                            .when(draft.busy, |button| button.opacity(0.6))
                            .on_click(cx.listener(|this, _, _, cx| this.submit_inbox_review(cx))),
                        ),
                )
                .into_any_element(),
        )
    }
}

pub(super) fn segmented_control() -> gpui::Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .p(px(2.0))
        .gap(px(2.0))
        .rounded(px(7.0))
        .bg(surface_tint(colors().elevated, colors().background))
}

pub(super) fn segment(
    id: SharedString,
    label: impl Into<SharedString>,
    selected: bool,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(24.0))
        .px(px(9.0))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(8.0))
        .rounded(px(5.0))
        .text_size(px(12.0))
        .cursor_pointer()
        .text_color(if selected {
            colors().foreground
        } else {
            colors().subtle
        })
        .when(selected, |button| {
            button.bg(surface_tint(colors().selection, colors().background))
        })
        .hover(|button| button.text_color(colors().foreground))
        .child(label.into())
}
