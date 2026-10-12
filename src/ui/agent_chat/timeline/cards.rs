//! Timeline cards for plans and notices. Permission requests are answered in
//! the composer instead (see `composer/approval.rs`).

use gpui::{AnyElement, Div, FontWeight, Rgba, div, prelude::*, px};

use super::super::tool_card::icon;
use crate::domain::agent_chat::{NoticeLevel, PlanEntry, PlanEntryStatus};
use crate::ui::theme::{colors, surface_tint};

/// Compact checklist for the agent's plan.
pub(super) fn plan(entries: &[PlanEntry]) -> AnyElement {
    let done = entries
        .iter()
        .filter(|entry| entry.status == PlanEntryStatus::Completed)
        .count();
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .p_3()
        .rounded(px(12.0))
        .bg(surface_tint(colors().elevated, colors().terminal))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Plan"),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(colors().subtle)
                        .child(format!("{done}/{} done", entries.len())),
                ),
        )
        .children(entries.iter().map(|entry| {
            let (icon_path, color) = match entry.status {
                PlanEntryStatus::Completed => (Some("chrome-icons/check.svg"), colors().success),
                PlanEntryStatus::InProgress => (Some("chrome-icons/refresh.svg"), colors().accent),
                PlanEntryStatus::Pending => (None, colors().subtle),
            };
            div()
                .flex()
                .items_start()
                .gap_2()
                .text_size(px(12.5))
                .line_height(px(18.0))
                .text_color(if entry.status == PlanEntryStatus::Completed {
                    colors().muted
                } else {
                    colors().foreground
                })
                .child(plan_marker(icon_path, color))
                .child(div().flex_1().min_w(px(0.0)).child(entry.content.clone()))
        }))
        .into_any_element()
}

fn plan_marker(icon_path: Option<&'static str>, color: Rgba) -> AnyElement {
    let marker = div()
        .w(px(13.0))
        .h(px(18.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center();
    match icon_path {
        Some(path) => marker.child(icon(path, color)).into_any_element(),
        None => marker
            .child(
                div()
                    .size(px(9.0))
                    .rounded(px(5.0))
                    .border_1()
                    .border_color(color),
            )
            .into_any_element(),
    }
}

/// Info notices are a centered system divider: a hairline on each side of a
/// muted line. Errors are danger-tinted text with a small icon, not a box.
pub(super) fn notice(level: NoticeLevel, text: &str) -> AnyElement {
    match level {
        NoticeLevel::Info => div()
            .w_full()
            .min_w(px(0.0))
            .flex()
            .items_center()
            .gap_2()
            .py(px(4.0))
            .child(hairline())
            .child(
                div()
                    .min_w(px(0.0))
                    .truncate()
                    .text_size(px(11.5))
                    .text_color(colors().muted)
                    .child(text.to_owned()),
            )
            .child(hairline())
            .into_any_element(),
        NoticeLevel::Error => div()
            .w_full()
            .px_1()
            .flex()
            .items_start()
            .gap_2()
            .text_size(px(12.5))
            .line_height(px(18.0))
            .text_color(colors().danger)
            .child(
                div()
                    .h(px(18.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .child(icon("chrome-icons/alert-circle.svg", colors().danger)),
            )
            .child(div().min_w(px(0.0)).child(text.to_owned()))
            .into_any_element(),
    }
}

/// The 1px rule on either side of a system divider.
fn hairline() -> Div {
    div().flex_1().h(px(1.0)).bg(colors().border_subtle)
}
