//! Commit history table and graph rendering.

use super::{
    DiffView, GRAPH_LANE_WIDTH, HISTORY_AUTHOR_WIDTH, HISTORY_DATE_WIDTH, HISTORY_HEADER_HEIGHT,
    HISTORY_PAGE, HISTORY_ROW_HEIGHT, HISTORY_SHA_WIDTH,
};
use crate::ports::git::GitCommit;
use crate::ui::git_graph::GitGraphRow;
use crate::ui::theme::{MONO_FONT, colors, surface_tint};
use gpui::{
    Context, Div, IntoElement, PathBuilder, Rgba, SharedString, Stateful, canvas, div, point,
    prelude::*, px, uniform_list,
};

impl DiffView {
    pub(super) fn history_graph_width(graph: &[GitGraphRow]) -> f32 {
        let lanes = graph.iter().map(|row| row.lane_count).max().unwrap_or(1);
        (lanes as f32 * GRAPH_LANE_WIDTH).clamp(18.0, 48.0)
    }

    pub(super) fn commit_controls(
        &self,
        commit: &GitCommit,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex_none()
            .w_full()
            .px_3()
            .py_2()
            .flex()
            .flex_col()
            .gap_2()
            .border_b_1()
            .border_color(colors().border_subtle)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .id("git-back-to-history")
                            .px_2()
                            .py_1()
                            .rounded(px(5.0))
                            .text_size(px(11.0))
                            .text_color(colors().muted)
                            .cursor_pointer()
                            .hover(|button| button.bg(colors().hover))
                            .child(if self.return_to_worktree {
                                "← Back to changes"
                            } else {
                                "← Back to history"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.return_to_worktree {
                                    this.set_review_expanded(false, cx);
                                } else {
                                    this.back_to_history(cx);
                                }
                            })),
                    )
                    .when(self.commit_error.is_some(), |view| {
                        view.child(
                            div()
                                .id("git-retry-commit")
                                .px_2()
                                .py_1()
                                .rounded(px(5.0))
                                .text_size(px(11.0))
                                .text_color(colors().accent)
                                .cursor_pointer()
                                .hover(|button| button.bg(colors().hover))
                                .child("Retry")
                                .on_click(cx.listener(|this, _, _, cx| this.refresh_commit(cx))),
                        )
                    }),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(colors().foreground)
                    .child(commit.subject.clone()),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(colors().subtle)
                    .child(format!(
                        "{} · {}",
                        commit.author,
                        format_short_date(&commit.date)
                    )),
            )
            .when(commit.parents.len() > 1, |view| {
                view.child(
                    div()
                        .text_size(px(10.0))
                        .text_color(colors().subtle)
                        .child("Merge commit · compared with first parent"),
                )
            })
    }

    pub(super) fn history_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let history = self.history.clone();
        let graph = self.history_graph.clone();
        let count = history.as_ref().map_or(0, |history| history.commits.len());
        let truncated = self
            .history
            .as_ref()
            .is_some_and(|history| history.truncated);
        let head = self.history.as_ref().map(|history| history.head.clone());
        let graph_width = Self::history_graph_width(graph.as_slice());
        let list_count = count + usize::from(truncated);

        div()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .flex()
            .flex_col()
            .child(Self::history_table_header(graph_width))
            .child(
                uniform_list(
                    "git-history",
                    list_count,
                    cx.processor(move |_this, range: std::ops::Range<usize>, _window, cx| {
                        let Some(history) = history.as_ref() else {
                            return Vec::new();
                        };
                        range
                            .filter_map(|index| {
                                if index == count {
                                    return truncated.then(|| {
                                        div()
                                            .h(px(HISTORY_ROW_HEIGHT))
                                            .w_full()
                                            .flex_none()
                                            .px_3()
                                            .flex()
                                            .items_center()
                                            .text_size(px(10.5))
                                            .text_color(colors().subtle)
                                            .child(format!(
                                                "Showing the {HISTORY_PAGE} most recent"
                                            ))
                                            .into_any_element()
                                    });
                                }
                                let commit = history.commits.get(index)?;
                                let selected = commit.clone();
                                let row = graph.get(index);
                                Some(
                                    Self::history_row(
                                        commit,
                                        row,
                                        graph_width,
                                        head.as_deref().is_some_and(|head| commit.sha == head),
                                    )
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.select_commit(selected.clone(), cx);
                                    }))
                                    .into_any_element(),
                                )
                            })
                            .collect()
                    }),
                )
                .flex_1()
                .min_h(px(0.0))
                .w_full(),
            )
    }

    fn history_table_header(graph_width: f32) -> Div {
        div()
            .h(px(HISTORY_HEADER_HEIGHT))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .px_2()
            .border_b_1()
            .border_color(colors().border_subtle)
            .child(div().w(px(graph_width)).h_full().flex_none())
            .child(Self::history_flex_cell(
                "Commit",
                true,
                10.5,
                colors().subtle,
            ))
            .child(Self::history_fixed_cell(
                "Author",
                HISTORY_AUTHOR_WIDTH,
                10.5,
                colors().subtle,
                false,
            ))
            .child(Self::history_fixed_cell(
                "Date",
                HISTORY_DATE_WIDTH,
                10.5,
                colors().subtle,
                false,
            ))
            .child(Self::history_fixed_cell(
                "SHA",
                HISTORY_SHA_WIDTH,
                10.5,
                colors().subtle,
                true,
            ))
    }

    fn history_flex_cell(
        text: impl Into<SharedString>,
        strong: bool,
        size: f32,
        color: Rgba,
    ) -> Div {
        div()
            .min_w(px(0.0))
            .flex_1()
            .overflow_hidden()
            .px_2()
            .child(
                div()
                    .w_full()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(size))
                    .font_weight(if strong {
                        gpui::FontWeight::MEDIUM
                    } else {
                        gpui::FontWeight::NORMAL
                    })
                    .text_color(color)
                    .child(text.into()),
            )
    }

    fn history_fixed_cell(
        text: impl Into<SharedString>,
        width: f32,
        size: f32,
        color: Rgba,
        mono: bool,
    ) -> Div {
        let mut cell = div()
            .w(px(width))
            .flex_none()
            .overflow_hidden()
            .pr_2()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_size(px(size))
            .text_color(color)
            .child(text.into());
        if mono {
            cell = cell.font_family(MONO_FONT);
        }
        cell
    }

    fn history_row(
        commit: &GitCommit,
        graph: Option<&GitGraphRow>,
        graph_width: f32,
        is_head: bool,
    ) -> Stateful<Div> {
        div()
            .id(SharedString::from(format!("git-commit-{}", commit.sha)))
            .h(px(HISTORY_ROW_HEIGHT))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .px_2()
            .overflow_hidden()
            .border_b_1()
            .border_color(colors().border_subtle)
            .hover(|row| row.bg(surface_tint(colors().hover, colors().panel)))
            .child(Self::graph_column(
                graph,
                graph_width,
                is_head,
                HISTORY_ROW_HEIGHT,
            ))
            .child(Self::history_flex_cell(
                commit.subject.clone(),
                is_head,
                12.0,
                colors().foreground,
            ))
            .child(Self::history_fixed_cell(
                commit.author.clone(),
                HISTORY_AUTHOR_WIDTH,
                11.0,
                colors().muted,
                false,
            ))
            .child(Self::history_fixed_cell(
                format_short_date(&commit.date),
                HISTORY_DATE_WIDTH,
                11.0,
                colors().subtle,
                false,
            ))
            .child(Self::history_fixed_cell(
                commit.short_sha.clone(),
                HISTORY_SHA_WIDTH,
                11.0,
                colors().subtle,
                true,
            ))
    }

    pub(super) fn graph_column(
        graph: Option<&GitGraphRow>,
        width: f32,
        is_head: bool,
        row_height: f32,
    ) -> Div {
        let Some(graph) = graph else {
            return div().w(px(width)).h_full().flex_none();
        };
        let lane = graph.lane;
        let through = graph
            .through
            .iter()
            .map(|rail| (rail.lane, lane_color(rail.color)))
            .collect::<Vec<_>>();
        let first_parent_edge = graph
            .first_parent_edge
            .map(|rail| (rail.lane, lane_color(graph.color)));
        let edges = graph
            .edges
            .iter()
            .map(|rail| (rail.lane, lane_color(rail.color)))
            .collect::<Vec<_>>();
        let active_color = lane_color(graph.color);
        let continues = graph.continues;
        let mid = row_height / 2.0;
        let lane_x = lane as f32 * GRAPH_LANE_WIDTH + 6.0;
        let node_size = if is_head { 10.0 } else { 6.0 };

        div()
            .w(px(width))
            .h_full()
            .flex_none()
            .relative()
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let top = bounds.top();
                        let bottom = bounds.bottom();
                        let middle = top + px(mid);

                        for (rail, color) in through {
                            let x = bounds.left() + px(rail as f32 * GRAPH_LANE_WIDTH + 6.0);
                            let mut path = PathBuilder::stroke(px(1.0));
                            path.move_to(point(x, top));
                            path.line_to(point(x, bottom));
                            if let Ok(path) = path.build() {
                                window.paint_path(path, color);
                            }
                        }

                        // Incoming half of the active rail always reaches the node.
                        let active_x = bounds.left() + px(lane_x);
                        let mut active = PathBuilder::stroke(px(1.0));
                        active.move_to(point(active_x, top));
                        active.line_to(point(active_x, middle));
                        if continues {
                            active.line_to(point(active_x, bottom));
                        }
                        if let Ok(path) = active.build() {
                            window.paint_path(path, active_color);
                        }

                        // When this lane rejoins an existing first-parent lane, bend the
                        // source-colored rail into it and stop the straight rail at the node.
                        if let Some((target, color)) = first_parent_edge {
                            let target_x =
                                bounds.left() + px(target as f32 * GRAPH_LANE_WIDTH + 6.0);
                            let mut path = PathBuilder::stroke(px(1.0));
                            path.move_to(point(active_x, middle));
                            path.cubic_bezier_to(
                                point(target_x, bottom),
                                point(active_x, middle + px(mid * 0.55)),
                                point(target_x, bottom - px(mid * 0.55)),
                            );
                            if let Ok(path) = path.build() {
                                window.paint_path(path, color);
                            }
                        }

                        // Secondary parents peel away with a smooth S-curve instead of
                        // the previous right-angle connector.
                        for (target, color) in edges {
                            let target_x =
                                bounds.left() + px(target as f32 * GRAPH_LANE_WIDTH + 6.0);
                            let mut path = PathBuilder::stroke(px(1.0));
                            path.move_to(point(active_x, middle));
                            path.cubic_bezier_to(
                                point(target_x, bottom),
                                point(active_x, middle + px(mid * 0.55)),
                                point(target_x, bottom - px(mid * 0.55)),
                            );
                            if let Ok(path) = path.build() {
                                window.paint_path(path, color);
                            }
                        }
                    },
                )
                .absolute()
                .inset_0(),
            )
            .child(
                div()
                    .absolute()
                    .left(px(lane_x - node_size / 2.0))
                    .top(px(mid - node_size / 2.0))
                    .size(px(node_size))
                    .rounded_full()
                    .when(is_head, |node| node.border_1().border_color(active_color))
                    .bg(if is_head {
                        colors().panel
                    } else {
                        active_color
                    }),
            )
    }
}

fn format_short_date(iso: &str) -> String {
    let mut parts = iso.split('-');
    let Some(year) = parts.next() else {
        return iso.to_owned();
    };
    let Some(month) = parts.next().and_then(|month| month.parse::<usize>().ok()) else {
        return iso.to_owned();
    };
    let Some(day) = parts.next() else {
        return iso.to_owned();
    };
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let month = months.get(month.saturating_sub(1)).copied().unwrap_or("?");
    let day = day.trim_start_matches('0');
    format!("{day} {month} {year}")
}

fn lane_color(lane: usize) -> Rgba {
    const LANES: [fn() -> Rgba; 6] = [
        || colors().accent,
        || gpui::rgba(0xd66aa0ff),
        || colors().success,
        || colors().warning,
        || colors().git_added,
        || colors().danger,
    ];
    LANES[lane % LANES.len()]()
}
