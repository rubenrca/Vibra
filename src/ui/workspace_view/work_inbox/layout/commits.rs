//! Commits tab: the pull request's history like a Git log, and one commit's
//! diff in the shared review.

use super::*;
use crate::domain::work_items::WorkCommit;

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
                .pb_2()
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(12.0))
                .text_color(colors().muted)
                .child(format!(
                    "{} commit{}",
                    commits.total,
                    if commits.total == 1 { "" } else { "s" }
                ))
                .when_some(new_from, |row, from| {
                    let count = commits.commits.len().saturating_sub(from);
                    row.when(count > 0, |row| {
                        row.child("·").child(
                            div()
                                .text_color(colors().accent)
                                .child(format!("{count} new since your last review")),
                        )
                    })
                }),
        );
        let last = commits.commits.len().saturating_sub(1);
        // Newest first, like a Git log.
        for (index, commit) in commits.commits.iter().enumerate().rev() {
            let new = new_from.is_some_and(|from| index >= from);
            body = body.child(self.inbox_commit_row(commit, index == last, index == 0, new, cx));
        }
        if commits.total > commits.commits.len() {
            body = body.child(message(
                "Showing the latest 100 commits. The full history is on GitHub.",
                false,
            ));
        }
        body.into_any_element()
    }

    fn inbox_commit_row(
        &self,
        commit: &WorkCommit,
        newest: bool,
        oldest: bool,
        new: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let oid = commit.oid.clone();
        let lane = colors().accent;
        div()
            .id(SharedString::from(format!("inbox-commit-{}", commit.oid)))
            .h(px(44.0))
            .flex()
            .items_center()
            .gap_3()
            .px_2()
            .rounded(px(6.0))
            .cursor_pointer()
            .hover(|row| row.bg(surface_tint(colors().hover, colors().background)))
            .on_click(
                cx.listener(move |this, _, _, cx| this.select_inbox_commit(Some(oid.clone()), cx)),
            )
            .child(
                div()
                    .w(px(14.0))
                    .h_full()
                    .flex_none()
                    .relative()
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .absolute()
                            .top(px(if newest { 22.0 } else { 0.0 }))
                            .bottom(px(if oldest { 22.0 } else { 0.0 }))
                            .w(px(2.0))
                            .bg(lane),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(17.0))
                            .size(px(10.0))
                            .rounded_full()
                            .border_2()
                            .border_color(lane)
                            .bg(surface(colors().background)),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
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
                                    .text_color(colors().foreground)
                                    .child(commit.headline.clone()),
                            )
                            .when(new, |row| {
                                row.child(
                                    div()
                                        .flex_none()
                                        .px(px(5.0))
                                        .rounded(px(4.0))
                                        .bg(colors().selection)
                                        .text_size(px(10.0))
                                        .text_color(colors().accent)
                                        .child("New"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(colors().subtle)
                            .child(format!(
                                "{} · {}",
                                commit.author,
                                relative_time(unix_now(), commit.at)
                            )),
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
                    .font_family(crate::ui::theme::MONO_FONT)
                    .text_size(px(11.0))
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
        let mut body = div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        quiet_button("inbox-commits-back", "← All commits").on_click(
                            cx.listener(|this, _, _, cx| this.select_inbox_commit(None, cx)),
                        ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .text_size(px(13.0))
                            .child(
                                commit
                                    .map(|commit| commit.headline.clone())
                                    .unwrap_or_default(),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_family(crate::ui::theme::MONO_FONT)
                            .text_size(px(11.0))
                            .text_color(colors().muted)
                            .child(oid.chars().take(7).collect::<String>()),
                    )
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
            body = body.child(message(error, true));
        }
        match state.and_then(|state| state.commit_review.clone()) {
            Some(view) if diff.is_some_and(|diff| diff.data.is_some()) => {
                body = body.child(div().flex_1().min_h(px(0.0)).child(view));
            }
            _ if diff.is_some_and(|diff| diff.error.is_some()) => {}
            _ => body = body.child(message("Loading commit files…", false)),
        }
        body.into_any_element()
    }
}
