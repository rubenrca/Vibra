//! Repository → History: the commit graph as a table, each commit's pull
//! request, and the selected commit's changes below it.

use super::code::{segment, segmented_control};
use super::*;
use crate::ports::git::GitCommit;
use crate::ui::diff_view::{DiffView, GRAPH_LANE_WIDTH, format_short_date};
use crate::ui::git_graph::GitGraphRow;
use crate::ui::theme::MONO_FONT;

const ROW_HEIGHT: f32 = 30.0;
const PULL_WIDTH: f32 = 84.0;
const AUTHOR_WIDTH: f32 = 128.0;
const DATE_WIDTH: f32 = 96.0;
const SHA_WIDTH: f32 = 72.0;
/// Share of the height the graph keeps while a commit is open below it.
const TABLE_SHARE: f32 = 0.42;

impl WorkspaceView {
    pub(super) fn history_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let history = &self.work_inbox.history;
        let mut view = div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .child(self.history_toolbar(cx));
        if self.history_root().is_none() {
            return view
                .child(history_empty(
                    "Add a project with a Git repository to see its history.",
                ))
                .into_any_element();
        }
        if let Some(error) = &history.log.error {
            view = view.child(div().px_3().py_2().child(message(error, true)));
        }
        let Some(log) = history.log.data.as_ref() else {
            return view
                .when(history.log.error.is_none(), |view| {
                    view.child(history_empty("Reading history…"))
                })
                .into_any_element();
        };
        if log.history.commits.is_empty() {
            return view
                .child(history_empty("This repository has no commits yet."))
                .into_any_element();
        }
        let lanes = log
            .graph
            .iter()
            .map(|row| row.lane_count)
            .max()
            .unwrap_or(1);
        let graph_width = (lanes as f32 * GRAPH_LANE_WIDTH + 6.0).clamp(24.0, 180.0);
        let rows = log.history.commits.len() + usize::from(log.history.truncated);
        let open = history.selected.is_some();
        view = view.child(
            div()
                .when(open, |table| {
                    table.h(gpui::relative(TABLE_SHARE)).flex_none()
                })
                .when(!open, |table| table.flex_1())
                .min_h(px(0.0))
                .flex()
                .flex_col()
                .child(history_header(graph_width))
                .child(
                    gpui::uniform_list(
                        "repository-history",
                        rows,
                        cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                            this.history_rows(range, graph_width, cx)
                        }),
                    )
                    .track_scroll(history.scroll.clone())
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full(),
                ),
        );
        if let Some(sha) = history.selected.as_deref() {
            view = view.child(self.history_commit_detail(sha, cx));
        }
        view.into_any_element()
    }

    /// Project, scope, size, and refresh.
    fn history_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let history = &self.work_inbox.history;
        let project = self.history_project();
        let name = project
            .and_then(|id| self.snapshot.project(id))
            .map(|project| project.name.clone())
            .unwrap_or_else(|| "Choose project".into());
        let count = history.log.data.as_ref().map(|log| {
            let shown = log.history.commits.len();
            if log.history.truncated {
                format!("Latest {shown} of {} commits", log.history.total)
            } else {
                format!("{} commit{}", shown, if shown == 1 { "" } else { "s" })
            }
        });
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
                div()
                    .id("history-project")
                    .flex_none()
                    .max_w(px(240.0))
                    .h(px(26.0))
                    .px_2()
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(colors().border_subtle)
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .cursor_pointer()
                    .hover(|button| button.bg(surface_tint(colors().hover, colors().background)))
                    .tooltip(|_, cx| sidebar_tooltip("Repository to show", cx))
                    .on_click(cx.listener(|this, event, _, cx| {
                        this.open_inbox_menu(InboxMenu::HistoryProjects, event, cx)
                    }))
                    .when_some(project, |button, id| {
                        button.child(
                            div()
                                .size(px(10.0))
                                .flex_none()
                                .rounded(px(3.0))
                                .bg(project_color(id)),
                        )
                    })
                    .child(
                        div()
                            .min_w(px(0.0))
                            .truncate()
                            .text_size(px(12.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(name),
                    )
                    .child(
                        svg()
                            .path("chrome-icons/chevron-down.svg")
                            .size(px(11.0))
                            .flex_none()
                            .text_color(colors().subtle),
                    ),
            )
            .child(
                segmented_control().children(
                    [(false, "All branches"), (true, "Current branch")]
                        .into_iter()
                        .map(|(current_only, label)| {
                            segment(
                                SharedString::from(format!("history-scope-{current_only}")),
                                label,
                                history.current_only == current_only,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| this.set_history_scope(current_only, cx),
                            ))
                        }),
                ),
            )
            .child(div().flex_1())
            .when_some(count, |bar, count| {
                bar.child(
                    div()
                        .flex_none()
                        .text_size(px(11.5))
                        .text_color(colors().subtle)
                        .child(count),
                )
            })
            .child(
                icon_button(
                    "history-refresh",
                    "chrome-icons/refresh.svg",
                    if history.log.loading {
                        "Refreshing…"
                    } else {
                        "Refresh (R)"
                    },
                )
                .when(history.log.loading, |button| button.opacity(0.4))
                .on_click(cx.listener(|this, _, _, cx| this.refresh_inbox(cx))),
            )
    }

    fn history_rows(
        &self,
        range: std::ops::Range<usize>,
        graph_width: f32,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(log) = self.work_inbox.history.log.data.as_ref() else {
            return Vec::new();
        };
        let commits = &log.history.commits;
        range
            .filter_map(|index| {
                let Some(commit) = commits.get(index) else {
                    return log.history.truncated.then(|| {
                        div()
                            .h(px(ROW_HEIGHT))
                            .w_full()
                            .flex_none()
                            .px_3()
                            .flex()
                            .items_center()
                            .text_size(px(11.0))
                            .text_color(colors().subtle)
                            .child(format!("Showing the latest {} commits.", commits.len()))
                            .into_any_element()
                    });
                };
                Some(self.history_row(
                    commit,
                    log.graph.get(index),
                    graph_width,
                    commit.sha == log.history.head,
                    &log.history.branch,
                    cx,
                ))
            })
            .collect()
    }

    fn history_row(
        &self,
        commit: &GitCommit,
        graph: Option<&GitGraphRow>,
        graph_width: f32,
        is_head: bool,
        branch: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let sha = commit.sha.clone();
        let selected = self.work_inbox.history.selected.as_deref() == Some(sha.as_str());
        let pull = self.commit_pull(commit).map(|pull| {
            let (color, tooltip) = match pull.item {
                Some(item) if item.status != WorkStatus::Merged => (
                    item.pull_step().map_or(colors().muted, step_color),
                    format!(
                        "{} · {}",
                        item.title,
                        item.pull_step().map_or("Open", |step| step.label())
                    ),
                ),
                Some(item) => (merged_color(), format!("{} · Merged", item.title)),
                None => (
                    merged_color(),
                    format!("Merged pull request #{}", pull.number),
                ),
            };
            (pull.number, pull.merged, color, tooltip)
        });
        let extra_refs = commit.refs.len().saturating_sub(2);
        div()
            .id(SharedString::from(format!("history-commit-{}", commit.sha)))
            .h(px(ROW_HEIGHT))
            .w_full()
            .flex_none()
            .px_2()
            .flex()
            .items_center()
            .overflow_hidden()
            .cursor_pointer()
            .when(selected, |row| {
                row.bg(surface_tint(colors().selection, colors().background))
            })
            .hover(|row| row.bg(surface_tint(colors().hover, colors().background)))
            .on_click(cx.listener(move |this, _, _, cx| {
                let open = this.work_inbox.history.selected.as_deref() == Some(sha.as_str());
                this.select_history_commit((!open).then(|| sha.clone()), cx);
            }))
            .child(DiffView::graph_column(
                graph,
                graph_width,
                is_head,
                ROW_HEIGHT,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .overflow_hidden()
                    .children(
                        commit
                            .refs
                            .iter()
                            .take(2)
                            .map(|name| ref_chip(name, name == branch)),
                    )
                    .when(extra_refs > 0, |cell| {
                        cell.child(
                            div()
                                .flex_none()
                                .text_size(px(10.5))
                                .text_color(colors().subtle)
                                .child(format!("+{extra_refs}")),
                        )
                    })
                    .child(
                        div()
                            .min_w(px(0.0))
                            .truncate()
                            .text_size(px(12.5))
                            .when(is_head, |text| text.font_weight(gpui::FontWeight::MEDIUM))
                            .text_color(colors().foreground)
                            .child(commit.subject.clone()),
                    ),
            )
            .child(div().w(px(PULL_WIDTH)).flex_none().when_some(
                pull,
                |cell, (number, merged, color, tooltip)| {
                    cell.child(
                        div()
                            .id(SharedString::from(format!("history-pull-{}", commit.sha)))
                            .flex_none()
                            .h(px(20.0))
                            .px(px(5.0))
                            .rounded(px(4.0))
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .text_size(px(11.5))
                            .text_color(color)
                            .cursor_pointer()
                            .hover(|badge| {
                                badge.bg(surface_tint(colors().hover, colors().background))
                            })
                            .tooltip(move |_, cx| sidebar_tooltip(tooltip.clone(), cx))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.open_commit_pull(number, cx);
                            }))
                            .child(
                                svg()
                                    .path(if merged {
                                        "chrome-icons/git-merge.svg"
                                    } else {
                                        "chrome-icons/git-pull-request.svg"
                                    })
                                    .size(px(11.0))
                                    .text_color(color),
                            )
                            .child(format!("#{number}")),
                    )
                },
            ))
            .child(history_cell(
                commit.author.clone(),
                AUTHOR_WIDTH,
                colors().muted,
                false,
            ))
            .child(history_cell(
                format_short_date(&commit.date),
                DATE_WIDTH,
                colors().subtle,
                false,
            ))
            .child(history_cell(
                commit.short_sha.clone(),
                SHA_WIDTH,
                colors().subtle,
                true,
            ))
            .into_any_element()
    }

    /// The open commit: what it is, its pull request, and its changes.
    fn history_commit_detail(&self, sha: &str, cx: &mut Context<Self>) -> AnyElement {
        let Some(commit) = self.history_commit(sha) else {
            return div().into_any_element();
        };
        let pull = self
            .commit_pull(commit)
            .map(|pull| (pull.number, pull.merged));
        let repository = self.history_github_repository();
        let full_sha = commit.sha.clone();
        let history = &self.work_inbox.history;
        let dot = || div().text_color(colors().subtle).child("·");
        div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(colors().border_subtle)
            .child(
                div()
                    .flex_none()
                    .px_3()
                    .py(px(10.0))
                    .flex()
                    .items_center()
                    .gap_3()
                    .border_b_1()
                    .border_color(colors().border_subtle)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(14.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(commit.subject.clone()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(px(6.0))
                                    .text_size(px(12.0))
                                    .text_color(colors().subtle)
                                    .child(div().flex_none().child(commit.author.clone()))
                                    .child(dot())
                                    .child(div().flex_none().child(format_short_date(&commit.date)))
                                    .child(dot())
                                    .child(
                                        div()
                                            .id("history-copy-sha")
                                            .flex_none()
                                            .h(px(20.0))
                                            .px(px(5.0))
                                            .rounded(px(4.0))
                                            .flex()
                                            .items_center()
                                            .gap(px(4.0))
                                            .font_family(MONO_FONT)
                                            .text_size(px(11.5))
                                            .cursor_pointer()
                                            .hover(|chip| {
                                                chip.bg(surface_tint(
                                                    colors().hover,
                                                    colors().background,
                                                ))
                                            })
                                            .tooltip(|_, cx| sidebar_tooltip("Copy SHA", cx))
                                            .on_click(move |_, _, cx| {
                                                cx.write_to_clipboard(ClipboardItem::new_string(
                                                    full_sha.clone(),
                                                ))
                                            })
                                            .child(commit.short_sha.clone())
                                            .child(
                                                svg()
                                                    .path("chrome-icons/copy.svg")
                                                    .size(px(10.0))
                                                    .text_color(colors().subtle),
                                            ),
                                    )
                                    .children(
                                        commit
                                            .refs
                                            .iter()
                                            .map(|name| ref_chip(name, false).into_any_element()),
                                    ),
                            ),
                    )
                    .when_some(pull, |row, (number, merged)| {
                        row.child(
                            super::item_detail::header_button(
                                "history-open-pull",
                                Some(if merged {
                                    "chrome-icons/git-merge.svg"
                                } else {
                                    "chrome-icons/git-pull-request.svg"
                                }),
                                format!("Pull request #{number}"),
                                false,
                            )
                            .on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.open_commit_pull(number, cx)
                                }),
                            ),
                        )
                    })
                    .when_some(repository, |row, repository| {
                        let url = format!("https://github.com/{repository}/commit/{sha}");
                        row.child(
                            icon_button(
                                "history-commit-external",
                                "chrome-icons/open-external.svg",
                                "Open on GitHub (O)",
                            )
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                        )
                    })
                    .child(
                        icon_button(
                            "history-close-commit",
                            "chrome-icons/close.svg",
                            "Close (Esc)",
                        )
                        .on_click(
                            cx.listener(|this, _, _, cx| this.select_history_commit(None, cx)),
                        ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .when_some(history.index.clone(), |row, index| {
                        row.child(
                            div()
                                .w(px(260.0))
                                .flex_none()
                                .h_full()
                                .border_r_1()
                                .border_color(colors().border_subtle)
                                .bg(surface(colors().sidebar))
                                .child(index),
                        )
                    })
                    .when_some(history.review.clone(), |row, review| {
                        row.child(div().flex_1().min_w(px(0.0)).h_full().child(review))
                    }),
            )
            .into_any_element()
    }
}

fn history_header(graph_width: f32) -> impl IntoElement {
    let label = |text: &'static str, width: Option<f32>| {
        div()
            .when_some(width, |cell, width| cell.w(px(width)).flex_none())
            .when(width.is_none(), |cell| cell.flex_1().min_w(px(0.0)).px_2())
            .text_size(px(10.5))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(colors().subtle)
            .child(text)
    };
    div()
        .h(px(26.0))
        .flex_none()
        .px_2()
        .flex()
        .items_center()
        .border_b_1()
        .border_color(colors().border_subtle)
        .child(div().w(px(graph_width)).flex_none())
        .child(label("Commit", None))
        .child(label("Pull request", Some(PULL_WIDTH)))
        .child(label("Author", Some(AUTHOR_WIDTH)))
        .child(label("Date", Some(DATE_WIDTH)))
        .child(label("SHA", Some(SHA_WIDTH)))
}

fn history_cell(text: String, width: f32, color: Rgba, mono: bool) -> impl IntoElement {
    div()
        .w(px(width))
        .flex_none()
        .pr_2()
        .truncate()
        .text_size(px(11.5))
        .text_color(color)
        .when(mono, |cell| cell.font_family(MONO_FONT).text_size(px(11.0)))
        .child(text)
}

/// A branch or tag pointing at a commit; the checked-out branch stands out.
fn ref_chip(name: &str, current: bool) -> gpui::Div {
    div()
        .flex_none()
        .max_w(px(160.0))
        .h(px(18.0))
        .px(px(6.0))
        .rounded(px(4.0))
        .flex()
        .items_center()
        .truncate()
        .text_size(px(10.5))
        .when(current, |chip| {
            chip.bg(colors().accent)
                .text_color(colors().background)
                .font_weight(gpui::FontWeight::MEDIUM)
        })
        .when(!current, |chip| {
            chip.bg(surface_tint(colors().elevated, colors().background))
                .text_color(colors().muted)
        })
        .child(name.to_owned())
}

fn history_empty(text: &'static str) -> impl IntoElement {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .text_color(colors().subtle)
        .child(
            svg()
                .path("chrome-icons/git-commit.svg")
                .size(px(22.0))
                .text_color(colors().subtle),
        )
        .child(div().text_size(px(12.5)).child(text))
}
