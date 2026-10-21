//! Merge box: whether a pull request can merge, and what blocks it.

use super::*;
use crate::domain::work_items::WorkDetail;

/// One requirement row: met, failing, or still pending.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Requirement {
    Met,
    Failing,
    Pending,
}

impl WorkspaceView {
    pub(super) fn inbox_merge_box(
        &self,
        item: &WorkItem,
        state: Option<&ItemState>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if item.kind != WorkKind::PullRequest
            || !matches!(item.status, WorkStatus::Open | WorkStatus::Draft)
        {
            return None;
        }
        let detail = state.and_then(|state| state.summary.data.as_ref())?;
        let base = if detail.base_ref.is_empty() {
            "the base branch".to_owned()
        } else {
            detail.base_ref.clone()
        };
        let mut rows = Vec::new();
        rows.push(match detail.review_decision.as_str() {
            "APPROVED" => (Requirement::Met, "Approved".to_owned()),
            "CHANGES_REQUESTED" => (Requirement::Failing, "Changes requested".to_owned()),
            "REVIEW_REQUIRED" => (Requirement::Pending, "Review required".to_owned()),
            _ => (Requirement::Met, "No review required".to_owned()),
        });
        let checks = state.and_then(|state| state.checks.data.as_ref());
        rows.push(match checks {
            Some(checks) if checks.is_empty() => (Requirement::Met, "No checks".to_owned()),
            Some(checks) => {
                let failed = checks.iter().filter(|check| check.failed()).count();
                let pending = checks
                    .iter()
                    .filter(|check| !check.failed() && !check.passed())
                    .count();
                if failed > 0 {
                    (
                        Requirement::Failing,
                        format!(
                            "{failed} check{} failing",
                            if failed == 1 { "" } else { "s" }
                        ),
                    )
                } else if pending > 0 {
                    (
                        Requirement::Pending,
                        format!(
                            "{pending} check{} running",
                            if pending == 1 { "" } else { "s" }
                        ),
                    )
                } else {
                    (Requirement::Met, "All checks passed".to_owned())
                }
            }
            None => (Requirement::Pending, "Loading checks…".to_owned()),
        });
        rows.push(if detail.merge.conflicts() {
            (Requirement::Failing, format!("Conflicts with {base}"))
        } else if detail.merge.mergeable == "MERGEABLE" {
            (Requirement::Met, format!("No conflicts with {base}"))
        } else {
            (Requirement::Pending, "Checking for conflicts…".to_owned())
        });
        if detail.merge.behind() {
            rows.push((Requirement::Pending, format!("Behind {base}")));
        }
        let url = item.url.clone();
        let mut box_ = div()
            .p_3()
            .rounded(px(8.0))
            .border_1()
            .border_color(colors().border_subtle)
            .bg(surface_tint(colors().panel, colors().background))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(11.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(colors().muted)
                    .child("MERGE STATUS"),
            )
            .children(rows.into_iter().map(|(requirement, label)| {
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.5))
                    .child(checks::check_status_icon(
                        requirement == Requirement::Failing,
                        requirement == Requirement::Met,
                        14.0,
                    ))
                    .child(label)
            }));
        if let Some(method) = &detail.merge.auto_merge {
            box_ = box_.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.5))
                    .text_color(colors().accent)
                    .child(
                        svg()
                            .path("chrome-icons/git-merge.svg")
                            .size(px(14.0))
                            .text_color(colors().accent),
                    )
                    .child(format!(
                        "Auto-merge enabled · {}",
                        merge_method_label(method)
                    )),
            );
        }
        let buttons: Vec<PrAction> = [
            (detail.merge.behind() && detail.merge.can_update_branch)
                .then_some(PrAction::UpdateBranch),
            (detail.merge.auto_merge.is_some() && detail.merge.can_disable_auto_merge)
                .then_some(PrAction::DisableAutoMerge),
        ]
        .into_iter()
        .flatten()
        .collect();
        if !buttons.is_empty() {
            box_ = box_.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .children(buttons.into_iter().map(|action| {
                        let url = url.clone();
                        section_button(
                            SharedString::from(format!("inbox-merge-{action:?}")),
                            action.label(),
                            false,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.propose_inbox_pr_action(&url, action);
                            cx.notify();
                        }))
                    })),
            );
        }
        Some(box_.into_any_element())
    }
}

/// Warnings worth reading before confirming a merge.
pub(super) fn merge_warnings(
    action: PrAction,
    detail: Option<&WorkDetail>,
    checks: Option<&[WorkCheck]>,
) -> Vec<&'static str> {
    if !action.merges() {
        return Vec::new();
    }
    let mut warnings = Vec::new();
    if let Some(detail) = detail {
        if detail.merge.conflicts() {
            warnings.push("GitHub reports conflicts with the base branch.");
        }
        if detail.review_decision == "CHANGES_REQUESTED" {
            warnings.push("A reviewer requested changes.");
        }
        if detail.merge.behind() {
            warnings.push("The branch is behind its base.");
        }
    }
    if checks.is_some_and(|checks| checks.iter().any(WorkCheck::failed)) {
        warnings.push("Some checks are failing.");
    }
    warnings
}

/// The pull request actions the menu offers for its state and permissions.
pub(super) fn available_pr_actions(item: &WorkItem, detail: Option<&WorkDetail>) -> Vec<PrAction> {
    let merge = detail.map(|detail| &detail.merge);
    let mut actions = match item.status {
        WorkStatus::Merged => vec![],
        WorkStatus::Closed => vec![PrAction::Reopen],
        WorkStatus::Draft => vec![PrAction::Ready, PrAction::Close],
        WorkStatus::Open => vec![PrAction::Merge, PrAction::Squash, PrAction::Rebase],
    };
    if item.status == WorkStatus::Open
        && merge.is_some_and(|merge| merge.can_enable_auto_merge && merge.auto_merge.is_none())
    {
        actions.extend([
            PrAction::AutoMerge,
            PrAction::AutoSquash,
            PrAction::AutoRebase,
        ]);
    }
    if matches!(item.status, WorkStatus::Open | WorkStatus::Draft)
        && merge.is_some_and(|merge| merge.auto_merge.is_some() && merge.can_disable_auto_merge)
    {
        actions.push(PrAction::DisableAutoMerge);
    }
    if matches!(item.status, WorkStatus::Open | WorkStatus::Draft)
        && merge.is_some_and(|merge| merge.can_update_branch && merge.behind())
    {
        actions.push(PrAction::UpdateBranch);
    }
    if item.status == WorkStatus::Open {
        actions.extend([PrAction::Draft, PrAction::Close]);
    }
    if matches!(item.status, WorkStatus::Merged | WorkStatus::Closed)
        && merge.is_some_and(|merge| merge.can_delete_head && !merge.head_ref_id.is_empty())
    {
        actions.push(PrAction::DeleteBranch);
    }
    actions
}

fn merge_method_label(method: &str) -> &'static str {
    match method {
        "SQUASH" => "Squash",
        "REBASE" => "Rebase",
        _ => "Merge commit",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::work_items::{MergeState, pull_fixture};

    #[test]
    fn pr_actions_follow_state_and_permissions() {
        let mut item = pull_fixture();
        let mut detail = WorkDetail {
            merge: MergeState {
                can_enable_auto_merge: true,
                can_update_branch: true,
                merge_state: "BEHIND".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let actions = available_pr_actions(&item, Some(&detail));
        assert!(actions.contains(&PrAction::AutoSquash));
        assert!(actions.contains(&PrAction::UpdateBranch));
        assert!(!actions.contains(&PrAction::DeleteBranch));
        detail.merge.auto_merge = Some("SQUASH".into());
        detail.merge.can_disable_auto_merge = true;
        let actions = available_pr_actions(&item, Some(&detail));
        assert!(!actions.contains(&PrAction::AutoSquash));
        assert!(actions.contains(&PrAction::DisableAutoMerge));
        item.status = WorkStatus::Merged;
        detail.merge.can_delete_head = true;
        detail.merge.head_ref_id = "REF".into();
        assert_eq!(
            available_pr_actions(&item, Some(&detail)),
            [PrAction::DeleteBranch]
        );
        assert!(available_pr_actions(&item, None).is_empty());
    }

    #[test]
    fn merge_confirmations_warn_about_blockers() {
        let detail = WorkDetail {
            review_decision: "CHANGES_REQUESTED".into(),
            merge: MergeState {
                mergeable: "CONFLICTING".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let failing = [WorkCheck {
            state: "FAILURE".into(),
            ..Default::default()
        }];
        assert_eq!(
            merge_warnings(PrAction::Squash, Some(&detail), Some(&failing)).len(),
            3
        );
        assert!(merge_warnings(PrAction::Close, Some(&detail), Some(&failing)).is_empty());
    }
}
