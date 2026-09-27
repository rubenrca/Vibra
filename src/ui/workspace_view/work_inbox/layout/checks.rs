use super::code::{segment, segmented_control};
use super::*;
use crate::ui::markdown::safe_link;
use crate::ui::theme::MONO_FONT;

impl WorkspaceView {
    pub(super) fn inbox_checks_body(
        &self,
        item: &WorkItem,
        state: Option<&ItemState>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let checks = state.and_then(|state| state.checks.data.as_ref());
        let totals = CheckTotals::from_checks(checks.map(Vec::as_slice).unwrap_or_default());
        let loading = state.is_none_or(|state| state.checks.loading);
        let mut body = div().flex().flex_col().gap(px(24.0)).child(
            div()
                .flex()
                .items_start()
                .gap_3()
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .px(px(8.0))
                        .child(
                            div()
                                .text_size(px(20.0))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(if checks.is_none() {
                                    if loading {
                                        "Loading checks…"
                                    } else {
                                        "Checks unavailable"
                                    }
                                } else {
                                    totals.title()
                                }),
                        )
                        .when(checks.is_some(), |header| {
                            header.child(
                                div()
                                    .text_size(px(13.0))
                                    .text_color(colors().subtle)
                                    .child(totals.summary()),
                            )
                        }),
                )
                .child(
                    icon_button(
                        "inbox-refresh-checks",
                        "chrome-icons/refresh.svg",
                        "Refresh checks",
                    )
                    .when(loading, |button| button.opacity(0.45))
                    .on_click(cx.listener(|this, _, _, cx| this.load_inbox_detail(true, cx))),
                ),
        );
        if let Some(error) = state.and_then(|state| state.checks.error.as_ref()) {
            body = body.child(message(error, true));
        }
        let Some(checks) = checks else {
            return body.into_any_element();
        };
        let attention_only = state.is_some_and(|state| state.checks_attention_only);
        body = body.child(
            div().flex().child(
                segmented_control().children(
                    [
                        (true, "Needs attention", totals.failed + totals.pending),
                        (false, "All checks", checks.len()),
                    ]
                    .into_iter()
                    .map(|(filter, label, count)| {
                        segment(
                            SharedString::from(format!("inbox-check-filter-{filter}")),
                            label,
                            attention_only == filter,
                        )
                        .h(px(28.0))
                        .child(div().text_color(colors().subtle).child(count.to_string()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(url) = this.work_inbox.selected.clone() {
                                this.work_inbox
                                    .details
                                    .entry(url)
                                    .or_default()
                                    .checks_attention_only = filter;
                                cx.notify();
                            }
                        }))
                    }),
                ),
            ),
        );
        if attention_only && totals.failed + totals.pending == 0 {
            body = body.child(message("No checks need attention.", false));
        }
        for (group, title, count) in [
            (0, "Needs attention", totals.failed),
            (1, "In progress", totals.pending),
            (2, "Passed", totals.passed),
        ] {
            if count == 0 || (attention_only && group == 2) {
                continue;
            }
            let mut section = div().flex().flex_col().gap(px(6.0)).child(
                div()
                    .px(px(8.0))
                    .pb(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(9.0))
                    .text_size(px(13.0))
                    .text_color(colors().subtle)
                    .child(title)
                    .child(div().text_size(px(11.0)).child(count.to_string())),
            );
            for (index, check) in checks
                .iter()
                .enumerate()
                .filter(|(_, check)| check_group(check) == group)
            {
                section = section.child(self.inbox_check_row(item, state, check, index, cx));
            }
            body = body.child(section);
        }
        body.into_any_element()
    }

    fn inbox_check_row(
        &self,
        item: &WorkItem,
        state: Option<&ItemState>,
        check: &WorkCheck,
        index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let color = check_color(check.failed(), check.passed());
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
        let mut card = div().flex_none().flex().flex_col().rounded(px(6.0)).child(
            div()
                .id(SharedString::from(format!("inbox-check-{index}")))
                .min_h(px(44.0))
                .px(px(8.0))
                .py(px(8.0))
                .flex()
                .items_center()
                .gap(px(12.0))
                .rounded(px(6.0))
                .hover(|row| row.bg(surface_tint(colors().hover, colors().background)))
                .child(check_status_icon(check.failed(), check.passed(), 18.0))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .text_size(px(14.0))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .child(check.name.clone()),
                        )
                        .when(!check.workflow.is_empty(), |row| {
                            row.child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(colors().subtle)
                                    .child(check.workflow.clone()),
                            )
                        }),
                )
                .child(
                    div()
                        .w(px(76.0))
                        .flex_none()
                        .text_size(px(12.0))
                        .text_color(color)
                        .child(check_label(check)),
                )
                .child(
                    div()
                        .w(px(62.0))
                        .flex_none()
                        .text_right()
                        .text_size(px(11.0))
                        .text_color(colors().subtle)
                        .child(
                            check
                                .duration_seconds
                                .map(duration_label)
                                .unwrap_or_default(),
                        ),
                )
                .when(can_fix, |row| {
                    row.child(
                        quiet_button(SharedString::from(format!("check-fix-{index}")), "Fix")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.prepare_inbox_check_fix(&fix_check, cx)
                            })),
                    )
                })
                .child(div().w(px(24.0)).flex_none().when(has_job, |row| {
                    row.child(
                        check_icon_button(
                            format!("check-log-{index}"),
                            if expanded {
                                "chrome-icons/chevron-down.svg"
                            } else {
                                "chrome-icons/chevron-right.svg"
                            },
                            if expanded { "Hide log" } else { "Show log" },
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_inbox_check(log_check.clone(), cx)
                        })),
                    )
                }))
                .child(div().w(px(24.0)).flex_none().when(safe_link(&url), |row| {
                    row.child(
                        check_icon_button(
                            format!("check-link-{index}"),
                            "chrome-icons/open-external.svg",
                            "Open check",
                        )
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                    )
                })),
        );
        if expanded {
            let log = state.and_then(|state| state.check_logs.get(&key));
            let mut content = div()
                .ml(px(38.0))
                .mb_2()
                .rounded(px(6.0))
                .overflow_hidden()
                .border_1()
                .border_color(colors().border_subtle)
                .bg(surface_tint(colors().panel, colors().background));
            if let Some(error) = log.and_then(|log| log.error.as_ref()) {
                content = content.child(message(error, true));
            }
            if let Some(text) = log.and_then(|log| log.data.as_ref()) {
                let rows: std::sync::Arc<Vec<String>> =
                    std::sync::Arc::new(text.lines().map(str::to_owned).collect());
                let count = rows.len();
                content = content.child(
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
                content = content.child(message("Loading log…", false));
            }
            card = card.child(content);
        }
        card.into_any_element()
    }
}

pub(super) fn check_status_icon(failed: bool, passed: bool, size: f32) -> impl IntoElement {
    svg()
        .path(if failed {
            "chrome-icons/issue-canceled.svg"
        } else if passed {
            "chrome-icons/issue-closed.svg"
        } else {
            "chrome-icons/issue-open.svg"
        })
        .size(px(size))
        .flex_none()
        .text_color(check_color(failed, passed))
}

fn check_color(failed: bool, passed: bool) -> Rgba {
    if failed {
        colors().danger
    } else if passed {
        colors().success
    } else {
        colors().warning
    }
}

fn check_icon_button(
    id: String,
    icon: &'static str,
    label: &'static str,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(id))
        .size(px(24.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.0))
        .cursor_pointer()
        .tooltip(move |_, cx| sidebar_tooltip(label, cx))
        .hover(|button| button.bg(surface_tint(colors().hover, colors().background)))
        .child(svg().path(icon).size(px(14.0)).text_color(colors().subtle))
}

fn check_label(check: &WorkCheck) -> &'static str {
    match check.state.as_str() {
        "SUCCESS" => "Passed",
        "NEUTRAL" => "Neutral",
        "SKIPPED" => "Skipped",
        "CANCELLED" => "Cancelled",
        "STALE" => "Stale",
        "TIMED_OUT" => "Timed out",
        "ACTION_REQUIRED" => "Action required",
        "IN_PROGRESS" => "Running",
        _ if check.failed() => "Failed",
        _ => "Pending",
    }
}

fn duration_label(seconds: u64) -> String {
    if seconds >= 3600 {
        format!("{}h {}m", seconds / 3600, seconds % 3600 / 60)
    } else if seconds >= 60 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

fn check_group(check: &WorkCheck) -> usize {
    if check.failed() {
        0
    } else if check.passed() {
        2
    } else {
        1
    }
}

#[derive(Default)]
struct CheckTotals {
    failed: usize,
    pending: usize,
    passed: usize,
}

impl CheckTotals {
    fn from_checks(checks: &[WorkCheck]) -> Self {
        let mut totals = Self::default();
        for check in checks {
            match check_group(check) {
                0 => totals.failed += 1,
                2 => totals.passed += 1,
                _ => totals.pending += 1,
            }
        }
        totals
    }

    fn title(&self) -> &'static str {
        if self.failed > 0 {
            "Checks need attention"
        } else if self.pending > 0 {
            "Checks in progress"
        } else if self.passed > 0 {
            "Checks passed"
        } else {
            "No checks"
        }
    }

    fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.failed > 0 {
            parts.push(format!("{} need attention", self.failed));
        }
        if self.pending > 0 {
            parts.push(format!("{} pending", self.pending));
        }
        if self.passed > 0 {
            parts.push(format!("{} passed", self.passed));
        }
        if parts.is_empty() {
            "This pull request has no checks.".into()
        } else {
            format!("{}.", parts.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_do_not_report_success_for_pending_cancelled_or_stale_runs() {
        let checks: Vec<_> = [
            "SUCCESS",
            "SKIPPED",
            "NEUTRAL",
            "IN_PROGRESS",
            "CANCELLED",
            "STALE",
        ]
        .into_iter()
        .map(|state| WorkCheck {
            state: state.into(),
            ..Default::default()
        })
        .collect();
        let totals = CheckTotals::from_checks(&checks);
        assert_eq!((totals.failed, totals.pending, totals.passed), (2, 1, 3));
        assert_eq!(totals.title(), "Checks need attention");
        assert_eq!(totals.summary(), "2 need attention, 1 pending, 3 passed.");
        assert_eq!(
            CheckTotals::from_checks(&checks[..3]).title(),
            "Checks passed"
        );
        assert_eq!(
            CheckTotals::from_checks(&checks[3..4]).title(),
            "Checks in progress"
        );
        assert_eq!(CheckTotals::from_checks(&[]).title(), "No checks");
    }
}
