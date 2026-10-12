//! The context meter inside the card, the quota line below it, and the spinner
//! shown while a turn runs.

use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, Div, IntoElement, Radians, Rgba, Transformation, div,
    prelude::*, px, relative, svg,
};

use super::super::QuotaMeter;
use crate::domain::agent_chat::ContextUsage;
use crate::ui::theme::colors;

/// Share of the context window in use, rounded. `None` when the size is unknown.
pub(super) fn context_percent(usage: ContextUsage) -> Option<u32> {
    if usage.size == 0 {
        return None;
    }
    let percent = (usage.used as f64 * 100.0 / usage.size as f64).round();
    Some((percent as u32).min(100))
}

/// A token count for the meter: "262K" for 262,144, "1.5M" for 1,500,000.
pub(super) fn format_token_count(count: u64) -> String {
    if count < 1_000 {
        count.to_string()
    } else if count < 999_500 {
        format!("{}K", (count + 500) / 1_000)
    } else {
        // One decimal place, rounded, so 999,500 reads as "1M" rather than "1000K".
        let tenths = (count + 50_000) / 100_000;
        if tenths.is_multiple_of(10) {
            format!("{}M", tenths / 10)
        } else {
            format!("{}.{}M", tenths / 10, tenths % 10)
        }
    }
}

/// A quota percentage as a whole number, clamped to 0 … 100.
pub(super) fn quota_percent(used: f32) -> u32 {
    used.round().clamp(0.0, 100.0) as u32
}

/// "19% 3h 52m", or just "19%" when the reset time is unknown.
fn quota_text(meter: &QuotaMeter) -> String {
    let percent = quota_percent(meter.used_percent);
    match &meter.resets_in {
        Some(resets_in) => format!("{percent}% {resets_in}"),
        None => format!("{percent}%"),
    }
}

/// A thin bar filled to `fraction` (0.0 … 1.0).
fn progress_track(fraction: f32, fill: Rgba) -> Div {
    div()
        .h(px(3.0))
        .rounded(px(2.0))
        .overflow_hidden()
        .bg(colors().border_subtle)
        .child(
            div()
                .h_full()
                .w(relative(fraction.clamp(0.0, 1.0)))
                .rounded(px(2.0))
                .bg(fill),
        )
}

/// Bars turn to the warning color near the limit.
fn fill_for(percent: u32) -> Rgba {
    if percent >= 90 {
        colors().warning
    } else {
        colors().accent
    }
}

/// "5%  [bar]  262K": the context used so far against the window size. Only
/// shown once the agent has reported usage.
pub(super) fn context_meter(usage: Option<ContextUsage>) -> Option<AnyElement> {
    let usage = usage?;
    let percent = context_percent(usage)?;
    Some(
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .h(px(16.0))
            .px(px(12.0))
            .pb(px(4.0))
            .text_size(px(10.5))
            .text_color(colors().subtle)
            .child(
                div()
                    .flex_none()
                    .min_w(px(30.0))
                    .child(format!("{percent}%")),
            )
            .child(
                progress_track(percent as f32 / 100.0, fill_for(percent))
                    .flex_1()
                    .min_w(px(40.0)),
            )
            .child(div().flex_none().child(format_token_count(usage.size)))
            .into_any_element(),
    )
}

/// Each quota window as a small bar and its text, separated by dots. Empty
/// when the provider reports no quota.
pub(super) fn quota_line(meters: &[QuotaMeter]) -> Option<AnyElement> {
    if meters.is_empty() {
        return None;
    }
    let mut line = div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .px(px(12.0))
        .h(px(16.0))
        .text_size(px(11.0))
        .text_color(colors().subtle);
    for (index, meter) in meters.iter().enumerate() {
        if index > 0 {
            line = line.child("·");
        }
        let percent = quota_percent(meter.used_percent);
        line = line.child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    progress_track(percent as f32 / 100.0, fill_for(percent))
                        .w(px(28.0))
                        .flex_none(),
                )
                .child(quota_text(meter)),
        );
    }
    Some(line.into_any_element())
}

/// A refresh glyph turning in place, shown while the agent works.
pub(super) fn working_spinner() -> impl IntoElement {
    svg()
        .path("chrome-icons/refresh.svg")
        .size(px(11.0))
        .flex_none()
        .text_color(colors().subtle)
        .with_animation(
            "agent-chat-composer-spinner",
            Animation::new(Duration::from_millis(900)).repeat(),
            |icon, delta| {
                icon.with_transformation(Transformation::rotate(Radians(
                    delta * std::f32::consts::TAU,
                )))
            },
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_counts_round_to_thousands_and_millions() {
        assert_eq!(format_token_count(0), "0");
        assert_eq!(format_token_count(999), "999");
        assert_eq!(format_token_count(1_000), "1K");
        assert_eq!(format_token_count(262_144), "262K");
        assert_eq!(format_token_count(999_499), "999K");
        assert_eq!(format_token_count(999_500), "1M");
        assert_eq!(format_token_count(1_000_000), "1M");
        assert_eq!(format_token_count(1_500_000), "1.5M");
    }

    #[test]
    fn context_percent_needs_a_known_size_and_is_capped() {
        let usage = |used, size| ContextUsage { used, size };
        assert_eq!(context_percent(usage(13_107, 262_144)), Some(5));
        assert_eq!(context_percent(usage(0, 262_144)), Some(0));
        assert_eq!(context_percent(usage(300, 200)), Some(100));
        assert_eq!(context_percent(usage(10, 0)), None);
    }

    #[test]
    fn quota_percent_rounds_and_clamps() {
        assert_eq!(quota_percent(19.4), 19);
        assert_eq!(quota_percent(19.6), 20);
        assert_eq!(quota_percent(150.0), 100);
        assert_eq!(quota_percent(-3.0), 0);
    }

    #[test]
    fn quota_text_includes_the_reset_time_when_known() {
        let meter = |resets_in: Option<&str>| QuotaMeter {
            label: "5h".into(),
            used_percent: 19.0,
            resets_in: resets_in.map(str::to_owned),
        };
        assert_eq!(quota_text(&meter(Some("3h 52m"))), "19% 3h 52m");
        assert_eq!(quota_text(&meter(None)), "19%");
    }
}
