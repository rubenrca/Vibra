//! Commits tab: the pull request's history like a Git log, and one commit's
//! diff in the shared review.

use super::*;
use crate::domain::work_items::{CheckRollup, WorkCommit};
use crate::infrastructure::clock::local_utc_offset;

impl WorkspaceView {
    pub(super) fn inbox_commits_body(
        &self,
        item: &WorkItem,
        state: Option<&ItemState>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(oid) = state.and_then(|state| state.selected_commit.clone()) {
            return self.inbox_commit_review(item, state, &oid, cx);
        }
        let mut body = div().flex().flex_col();
        if let Some(error) = state.and_then(|state| state.commits.error.as_ref()) {
            body = body.child(message(error, true));
        }
        let Some(commits) = state.and_then(|state| state.commits.data.as_ref()) else {
            return body
                .child(message("Loading commits…", false))
                .into_any_element();
        };
        if commits.commits.is_empty() {
            return body
                .child(message("This pull request has no commits.", false))
                .into_any_element();
        }
        // Commits after the head the user last reviewed are new to them.
        let reviewed = self.inbox_since_base(&item.url);
        let new_from = reviewed.as_ref().and_then(|oid| {
            commits
                .commits
                .iter()
                .position(|commit| commit.oid == *oid)
                .map(|index| index + 1)
        });
        body = body.child(
            div()
                .pb(px(14.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(colors().foreground)
                        .child("Commits"),
                )
                .child(count_badge(commits.total, false))
                .when_some(new_from, |row, from| {
                    let count = commits.commits.len().saturating_sub(from);
                    row.when(count > 0, |row| {
                        row.child(
                            div()
                                .text_size(px(12.0))
                                .text_color(colors().accent)
                                .child(format!("{count} new since your last review")),
                        )
                    })
                }),
        );
        // Newest first, like a Git log, in one card per day.
        let now = unix_now();
        let today = local_date(now);
        let mut days: Vec<(chrono::NaiveDate, Vec<(usize, &WorkCommit)>)> = Vec::new();
        for (index, commit) in commits.commits.iter().enumerate().rev() {
            let day = local_date(commit.at);
            match days.last_mut() {
                Some((last, list)) if *last == day => list.push((index, commit)),
                _ => days.push((day, vec![(index, commit)])),
            }
        }
        let count = days.len();
        for (position, (day, list)) in days.into_iter().enumerate() {
            let rows = list.into_iter().enumerate().map(|(row, (index, commit))| {
                let new = new_from.is_some_and(|from| index >= from);
                self.inbox_commit_row(commit, row == 0, new, now, cx)
            });
            body = body.child(commit_day(
                day_label(day, today),
                rows.collect(),
                position + 1 == count,
            ));
        }
        if commits.total > commits.commits.len() {
            body = body.child(message(
                "Showing the latest 100 commits. The full history is on GitHub.",
                false,
            ));
        }
        body.into_any_element()
    }

    /// One commit in its day's card: subject, then author and time; checks
    /// and the short hash on the right.
    fn inbox_commit_row(
        &self,
        commit: &WorkCommit,
        first: bool,
        new: bool,
        now: u64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let oid = commit.oid.clone();
        div()
            .id(SharedString::from(format!("inbox-commit-{}", commit.oid)))
            .px(px(14.0))
            .py(px(10.0))
            .flex()
            .items_center()
            .gap(px(12.0))
            .cursor_pointer()
            .when(!first, |row| {
                row.border_t_1().border_color(colors().border_subtle)
            })
            .hover(|row| row.bg(card_hover()))
            .tooltip({
                let headline = commit.headline.clone();
                move |_, cx| sidebar_tooltip(headline.clone(), cx)
            })
            .on_click(
                cx.listener(move |this, _, _, cx| this.select_inbox_commit(Some(oid.clone()), cx)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .min_w(px(0.0))
                                    .truncate()
                                    .text_size(px(13.0))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(colors().foreground)
                                    .child(commit.headline.clone()),
                            )
                            .when(new, |row| {
                                row.child(
                                    div()
                                        .flex_none()
                                        .h(px(18.0))
                                        .px(px(6.0))
                                        .rounded_full()
                                        .flex()
                                        .items_center()
                                        .bg(tinted(colors().accent, 0.14))
                                        .text_size(px(10.5))
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .text_color(colors().accent)
                                        .child("New"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
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
                            .child(
                                div()
                                    .min_w(px(0.0))
                                    .truncate()
                                    .child(format!("committed {}", relative_time(now, commit.at))),
                            ),
                    ),
            )
            .when(commit.checks != CheckRollup::None, |row| {
                row.child(checks::check_status_icon(
                    commit.checks == CheckRollup::Failing,
                    commit.checks == CheckRollup::Passing,
                    14.0,
                ))
            })
            .child(
                div()
                    .flex_none()
                    .h(px(22.0))
                    .px(px(7.0))
                    .rounded(px(6.0))
                    .flex()
                    .items_center()
                    .bg(foreground(0.05))
                    .font_family(crate::ui::theme::MONO_FONT)
                    .text_size(px(11.5))
                    .text_color(colors().muted)
                    .child(commit.short_oid.clone()),
            )
            .into_any_element()
    }

    fn inbox_commit_review(
        &self,
        item: &WorkItem,
        state: Option<&ItemState>,
        oid: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let commit = state
            .and_then(|state| state.commits.data.as_ref())
            .and_then(|commits| commits.commits.iter().find(|commit| commit.oid == oid));
        let diff = state.and_then(|state| state.commit_diffs.get(oid));
        let url = format!("{}/commits/{oid}", item.url);
        let review = state.and_then(|state| state.commit_review.clone());
        let summary = review
            .as_ref()
            .map(|review| review.read(cx).change_summary())
            .filter(|(files, _, _)| *files > 0);
        // Back beside the subject; who, when, and the size under it; the
        // review's controls and GitHub on the right.
        let mut body = div().flex_1().min_h(px(0.0)).flex().flex_col().child(
            div()
                .flex_none()
                .px(px(16.0))
                .py(px(12.0))
                .flex()
                .items_center()
                .gap(px(12.0))
                .border_b_1()
                .border_color(colors().border_subtle)
                .child(
                    back_button("inbox-commits-back", "All commits")
                        .on_click(cx.listener(|this, _, _, cx| this.select_inbox_commit(None, cx))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(
                            div()
                                .min_w(px(0.0))
                                .truncate()
                                .text_size(px(13.5))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(colors().foreground)
                                .child(
                                    commit
                                        .map(|commit| commit.headline.clone())
                                        .unwrap_or_default(),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .text_size(px(12.0))
                                .text_color(colors().subtle)
                                .when_some(commit, |row, commit| {
                                    row.child(
                                        div()
                                            .flex_none()
                                            .text_color(colors().muted)
                                            .child(commit.author.clone()),
                                    )
                                    .child(
                                        div().flex_none().child(format!(
                                            "committed {}",
                                            relative_time(unix_now(), commit.at)
                                        )),
                                    )
                                })
                                .child(hash_chip(
                                    "inbox-commit-sha",
                                    oid.chars().take(7).collect::<String>(),
                                    oid.to_owned(),
                                ))
                                .when_some(summary, |row, (files, additions, deletions)| {
                                    row.child(div().flex_none().child(format!(
                                        "{files} file{}",
                                        if files == 1 { "" } else { "s" }
                                    )))
                                    .child(line_counts(additions as u64, deletions as u64))
                                }),
                        ),
                )
                .when_some(review, |row, review| {
                    row.child(self.review_controls(&review, cx))
                })
                .child(
                    icon_button(
                        "inbox-commit-external",
                        "chrome-icons/open-external.svg",
                        "Open commit on GitHub",
                    )
                    .on_click(move |_, _, cx| cx.open_url(&url)),
                ),
        );
        if let Some(error) = diff.and_then(|diff| diff.error.as_ref()) {
            body = body.child(div().p_2().child(message(error, true)));
        }
        match state.and_then(|state| state.commit_review.clone()) {
            Some(view) if diff.is_some_and(|diff| diff.data.is_some()) => {
                body = body.child(div().flex_1().min_h(px(0.0)).child(view));
            }
            _ if diff.is_some_and(|diff| diff.error.is_some()) => {}
            _ => body = body.child(div().p_3().child(message("Loading commit files…", false))),
        }
        body.into_any_element()
    }
}

/// A day of commits: a node on a quiet timeline, the day, and its card.
fn commit_day(label: String, rows: Vec<AnyElement>, last: bool) -> impl IntoElement {
    div()
        .flex()
        .gap(px(12.0))
        .child(
            div()
                .w(px(16.0))
                .flex_none()
                .flex()
                .flex_col()
                .items_center()
                .child(
                    div().h(px(22.0)).flex().items_center().child(
                        svg()
                            .path("chrome-icons/git-commit.svg")
                            .size(px(14.0))
                            .text_color(colors().subtle),
                    ),
                )
                .when(!last, |lane| {
                    lane.child(
                        div()
                            .flex_1()
                            .w(px(1.0))
                            .mt(px(4.0))
                            .bg(colors().border_subtle),
                    )
                }),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .pb(px(if last { 0.0 } else { 20.0 }))
                .child(
                    div()
                        .h(px(22.0))
                        .flex()
                        .items_center()
                        .text_size(px(12.5))
                        .text_color(colors().muted)
                        .child(label),
                )
                .child(
                    div()
                        .rounded(px(10.0))
                        .border_1()
                        .border_color(colors().border_subtle)
                        .overflow_hidden()
                        .flex()
                        .flex_col()
                        .children(rows),
                ),
        )
}

/// The calendar day of `at` in the system time zone.
fn local_date(at: u64) -> chrono::NaiveDate {
    let seconds = at.min(i64::MAX as u64) as i64;
    let local = seconds.saturating_add(i64::from(local_utc_offset(seconds)));
    chrono::DateTime::from_timestamp(local, 0)
        .map(|time| time.date_naive())
        .unwrap_or_default()
}

/// "Today", "Yesterday", then the date; the year only when it is not this one.
fn day_label(day: chrono::NaiveDate, today: chrono::NaiveDate) -> String {
    use chrono::Datelike;
    if day == today {
        return "Today".to_owned();
    }
    if today.pred_opt() == Some(day) {
        return "Yesterday".to_owned();
    }
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let month = MONTHS[day.month0() as usize];
    if day.year() == today.year() {
        format!("{month} {}", day.day())
    } else {
        format!("{month} {}, {}", day.day(), day.year())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_days_read_relative_to_today() {
        let date = |y, m, d| chrono::NaiveDate::from_ymd_opt(y, m, d).unwrap();
        let today = date(2026, 11, 6);
        assert_eq!(day_label(today, today), "Today");
        assert_eq!(day_label(date(2026, 11, 5), today), "Yesterday");
        assert_eq!(day_label(date(2026, 10, 30), today), "Oct 30");
        assert_eq!(day_label(date(2025, 12, 31), today), "Dec 31, 2025");
    }
}
