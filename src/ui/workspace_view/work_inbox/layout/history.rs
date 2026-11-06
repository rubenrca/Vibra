//! Repository → History: the commit graph as a table, each commit's pull
//! request, and the opened commit's page in its place.

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
const FILES_WIDTH: f32 = 260.0;
/// A commit lists its files beside the diff only past this many; fewer read
/// fine as the cards alone.
const INDEX_MIN_FILES: usize = 6;

impl WorkspaceView {
    pub(super) fn history_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let history = &self.work_inbox.history;
        if let Some(commit) = history
            .selected
            .as_deref()
            .and_then(|sha| self.history_commit(sha))
        {
            return self.history_commit_page(commit, cx);
        }
        let mut view = div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .child(self.history_toolbar(cx));
        if self.history_root().is_none() {
            return view
                .child(history_empty(
                    "Open a space in a Git repository to see its history.",
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
        let graph_width = (lanes as f32 * GRAPH_LANE_WIDTH + 6.0).clamp(24.0, 240.0);
        let rows = log.history.commits.len() + usize::from(log.history.truncated);
        view.child(
            div()
                .flex_1()
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
        )
        .into_any_element()
    }

    /// Project, scope, size, and refresh.
    fn history_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let history = &self.work_inbox.history;
        let project = self.history_project();
        let name = project
            .and_then(|id| self.snapshot.project(id))
            .map(|project| project.name.clone())
            .unwrap_or_else(|| "Choose space".into());
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
        let locals = self.history_local_branches();
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
                    &commit_refs(&commit.refs, &log.history.branch, locals.as_ref()),
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
        refs: &[CommitRef],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let sha = commit.sha.clone();
        let marked = self.work_inbox.history.cursor.as_deref() == Some(sha.as_str());
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
        let extra_refs = refs.len().saturating_sub(2);
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
            .when(marked, |row| {
                row.bg(surface_tint(colors().selection, colors().background))
            })
            .hover(|row| row.bg(surface_tint(colors().hover, colors().background)))
            .on_click(
                cx.listener(move |this, _, _, cx| {
                    this.select_history_commit(Some(sha.clone()), cx)
                }),
            )
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
                        refs.iter()
                            .take(2)
                            .enumerate()
                            .map(|(index, name)| ref_chip(&commit.sha, index, name)),
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

    /// The open commit in place of the graph: one header with the way back,
    /// what the commit is, and how to read it; then its changes. Commits with
    /// many files list them beside the diff.
    fn history_commit_page(&self, commit: &GitCommit, cx: &mut Context<Self>) -> AnyElement {
        let history = &self.work_inbox.history;
        let files = history
            .review
            .as_ref()
            .map_or(0, |review| review.read(cx).change_summary().0);
        div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .child(self.history_commit_header(commit, cx))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .when(files > INDEX_MIN_FILES, |row| {
                        row.when_some(history.index.clone(), |row, index| {
                            row.child(
                                div()
                                    .w(px(FILES_WIDTH))
                                    .flex_none()
                                    .h_full()
                                    .border_r_1()
                                    .border_color(colors().border_subtle)
                                    .child(index),
                            )
                        })
                    })
                    .when_some(history.review.clone(), |row, review| {
                        row.child(div().flex_1().min_w(px(0.0)).h_full().child(review))
                    }),
            )
            .into_any_element()
    }

    /// Back, the subject, and the way to the newer and older commit, its pull
    /// request, and GitHub; under them who made it, when, its hash and refs,
    /// its size, and the review's controls.
    fn history_commit_header(
        &self,
        commit: &GitCommit,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let position = self.history_position();
        let newer = position.is_some_and(|(index, _)| index > 0);
        let older = position.is_some_and(|(index, total)| index + 1 < total);
        let pull = self
            .commit_pull(commit)
            .map(|pull| (pull.number, pull.merged));
        let url = self
            .history_github_repository()
            .map(|repository| format!("https://github.com/{repository}/commit/{}", commit.sha));
        let refs = self
            .work_inbox
            .history
            .log
            .data
            .as_ref()
            .map(|log| {
                commit_refs(
                    &commit.refs,
                    &log.history.branch,
                    self.history_local_branches().as_ref(),
                )
            })
            .unwrap_or_default();
        let review = self.work_inbox.history.review.clone();
        let summary = review
            .as_ref()
            .map(|review| review.read(cx).change_summary())
            .filter(|(files, _, _)| *files > 0);
        let dot = || div().flex_none().text_color(colors().subtle).child("·");
        div()
            .flex_none()
            .px(px(20.0))
            .pt(px(16.0))
            .pb(px(14.0))
            .flex()
            .items_start()
            .gap(px(8.0))
            .border_b_1()
            .border_color(colors().border_subtle)
            .child(
                back_button("history-back", "Back to history (Esc)")
                    .on_click(cx.listener(|this, _, _, cx| this.select_history_commit(None, cx))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap(px(12.0))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .pt(px(3.0))
                                    .text_size(px(17.0))
                                    .line_height(px(24.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(colors().foreground)
                                    .child(commit.subject.clone()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(
                                        segmented_control()
                                            .child(
                                                step_button(
                                                    "history-newer",
                                                    "chrome-icons/chevron-up.svg",
                                                    "Newer commit (K)",
                                                    newer,
                                                )
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    if newer {
                                                        this.step_history_commit(-1, cx)
                                                    }
                                                })),
                                            )
                                            .child(
                                                step_button(
                                                    "history-older",
                                                    "chrome-icons/chevron-down.svg",
                                                    "Older commit (J)",
                                                    older,
                                                )
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    if older {
                                                        this.step_history_commit(1, cx)
                                                    }
                                                })),
                                            ),
                                    )
                                    .when_some(position, |row, (index, total)| {
                                        row.child(
                                            div()
                                                .flex_none()
                                                .mr(px(4.0))
                                                .text_size(px(11.5))
                                                .text_color(colors().subtle)
                                                .child(format!("{} of {total}", index + 1)),
                                        )
                                    })
                                    .when_some(pull, |row, (number, merged)| {
                                        row.child(
                                            super::item_detail::header_button(
                                                "history-open-pull",
                                                Some(if merged {
                                                    "chrome-icons/git-merge.svg"
                                                } else {
                                                    "chrome-icons/git-pull-request.svg"
                                                }),
                                                format!("#{number}"),
                                                false,
                                            )
                                            .tooltip(move |_, cx| {
                                                sidebar_tooltip(
                                                    format!("Open pull request #{number}"),
                                                    cx,
                                                )
                                            })
                                            .on_click(
                                                cx.listener(move |this, _, _, cx| {
                                                    this.open_commit_pull(number, cx)
                                                }),
                                            ),
                                        )
                                    })
                                    .when_some(url, |row, url| {
                                        row.child(
                                            icon_button(
                                                "history-commit-external",
                                                "chrome-icons/open-external.svg",
                                                "Open on GitHub (O)",
                                            )
                                            .on_click(move |_, _, cx| cx.open_url(&url)),
                                        )
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.0))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(px(6.0))
                                    .text_size(px(12.0))
                                    .text_color(colors().subtle)
                                    .child(super::item_detail::avatar(&commit.author, 16.0))
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_color(colors().muted)
                                            .child(commit.author.clone()),
                                    )
                                    .child(dot())
                                    .child(div().flex_none().child(format_short_date(&commit.date)))
                                    .child(hash_chip(
                                        "history-copy-sha",
                                        commit.short_sha.clone(),
                                        commit.sha.clone(),
                                    ))
                                    .children(
                                        refs.iter()
                                            .enumerate()
                                            .map(|(index, name)| ref_chip("detail", index, name)),
                                    )
                                    .when_some(summary, |row, (files, additions, deletions)| {
                                        row.child(dot())
                                            .child(div().flex_none().child(format!(
                                                "{files} file{}",
                                                if files == 1 { "" } else { "s" }
                                            )))
                                            .child(line_counts(additions as u64, deletions as u64))
                                    })
                                    .when(commit.parents.len() > 1, |row| {
                                        row.child(dot()).child(
                                            div()
                                                .flex_none()
                                                .child("Merge commit · compared with first parent"),
                                        )
                                    }),
                            )
                            .when_some(review, |row, review| {
                                row.child(self.review_controls(&review, cx))
                            }),
                    ),
            )
    }
}

/// One step of the newer and older pair, inside its track.
fn step_button(
    id: &'static str,
    icon: &'static str,
    label: &'static str,
    enabled: bool,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .size(px(24.0))
        .flex_none()
        .rounded(px(crate::ui::tab_style::TAB_RADIUS))
        .flex()
        .items_center()
        .justify_center()
        .tooltip(move |_, cx| sidebar_tooltip(label, cx))
        .when(enabled, |button| {
            button
                .cursor_pointer()
                .hover(|button| button.bg(card_hover()))
        })
        .when(!enabled, |button| button.opacity(0.35))
        .child(svg().path(icon).size(px(13.0)).text_color(colors().muted))
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum RefKind {
    Current,
    Local,
    Tag,
    Remote,
}

/// A branch or tag pointing at a commit, as the table shows it.
#[derive(Debug, PartialEq, Eq)]
struct CommitRef {
    label: String,
    kind: RefKind,
    full: String,
}

/// The checked-out branch first, then local branches, tags, and remote
/// branches. A remote copy at the same commit as its local branch is left
/// out, and `origin/` is implied. Without the local branch list, only
/// `origin/` and `upstream/` count as remote.
fn commit_refs(
    refs: &[String],
    current: &str,
    locals: Option<&std::collections::HashSet<String>>,
) -> Vec<CommitRef> {
    let mut chips: Vec<CommitRef> = refs
        .iter()
        .filter_map(|name| {
            let remote = name.split_once('/').filter(|(remote, _)| {
                locals.is_some_and(|locals| !locals.contains(name))
                    || (locals.is_none() && matches!(*remote, "origin" | "upstream"))
            });
            let (label, kind) = if name == current {
                (name.clone(), RefKind::Current)
            } else if let Some((remote, branch)) = remote {
                if refs.iter().any(|other| other == branch) {
                    return None;
                }
                let label = if remote == "origin" { branch } else { name };
                (label.to_owned(), RefKind::Remote)
            } else if locals.is_some_and(|locals| !locals.contains(name)) {
                (name.clone(), RefKind::Tag)
            } else {
                (name.clone(), RefKind::Local)
            };
            Some(CommitRef {
                label,
                kind,
                full: name.clone(),
            })
        })
        .collect();
    chips.sort_by_key(|chip| chip.kind);
    chips
}

fn ref_chip(owner: &str, index: usize, chip: &CommitRef) -> impl IntoElement {
    let tooltip = match chip.kind {
        RefKind::Current => format!("{} · checked out", chip.full),
        RefKind::Local => format!("{} · local branch", chip.full),
        RefKind::Tag => format!("{} · tag", chip.full),
        RefKind::Remote => format!("{} · remote branch", chip.full),
    };
    div()
        .id(SharedString::from(format!("history-ref-{owner}-{index}")))
        .flex_none()
        .max_w(px(180.0))
        .h(px(20.0))
        .px(px(7.0))
        .rounded_full()
        .flex()
        .items_center()
        .truncate()
        .text_size(px(10.5))
        .tooltip(move |_, cx| sidebar_tooltip(tooltip.clone(), cx))
        .map(|chip_div| match chip.kind {
            RefKind::Current => chip_div
                .bg(tinted(colors().accent, 0.16))
                .text_color(colors().accent)
                .font_weight(gpui::FontWeight::MEDIUM),
            RefKind::Local => chip_div
                .bg(foreground(0.06))
                .text_color(colors().foreground),
            RefKind::Tag => chip_div
                .bg(tinted(colors().warning, 0.14))
                .text_color(colors().warning),
            RefKind::Remote => chip_div
                .border_1()
                .border_color(colors().border_subtle)
                .text_color(colors().subtle),
        })
        .child(chip.label.clone())
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

#[cfg(test)]
mod tests {
    use super::{RefKind, commit_refs};

    #[test]
    fn refs_hide_synced_remotes_and_tell_tags_apart() {
        let refs = [
            "origin/main",
            "main",
            "v1.0",
            "origin/dependabot/cargo/serde",
            "upstream/main",
            "feature/x",
        ]
        .map(String::from);
        let locals = ["main", "feature/x"]
            .map(String::from)
            .into_iter()
            .collect();
        let chips = commit_refs(&refs, "main", Some(&locals));
        let summary: Vec<_> = chips
            .iter()
            .map(|chip| (chip.label.as_str(), chip.kind))
            .collect();
        assert_eq!(
            summary,
            [
                ("main", RefKind::Current),
                ("feature/x", RefKind::Local),
                ("v1.0", RefKind::Tag),
                ("dependabot/cargo/serde", RefKind::Remote),
            ],
            "origin/main and upstream/main sit on the same commit as main"
        );
        // Before the local branches load, only known remotes count as remote.
        let chips = commit_refs(&refs[3..], "main", None);
        let summary: Vec<_> = chips
            .iter()
            .map(|chip| (chip.label.as_str(), chip.kind))
            .collect();
        assert_eq!(
            summary,
            [
                ("feature/x", RefKind::Local),
                ("dependabot/cargo/serde", RefKind::Remote),
                ("upstream/main", RefKind::Remote),
            ]
        );
    }
}
