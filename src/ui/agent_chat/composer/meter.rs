//! The context meter in the composer toolbar and its formatting helpers.

use gpui::{AnyElement, Div, Rgba, div, prelude::*, px, relative};

use super::super::tooltip::tooltip;
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

/// A thin bar for the share of the context window in use. Shown once the agent
/// has reported usage; its tooltip gives the percentage and the token counts.
pub(super) fn context_meter(usage: Option<ContextUsage>) -> Option<AnyElement> {
    let usage = usage?;
    let percent = context_percent(usage)?;
    let label = format!(
        "{percent}% · {}/{} tokens",
        format_token_count(usage.used),
        format_token_count(usage.size)
    );
    Some(
        div()
            .id("agent-chat-context-meter")
            .w(px(40.0))
            .h(px(20.0))
            .flex_none()
            .flex()
            .items_center()
            .tooltip(move |_, cx| tooltip(label.clone(), cx))
            .child(progress_track(percent as f32 / 100.0, fill_for(percent)).w_full())
            .into_any_element(),
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
}
