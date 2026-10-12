//! The status line above the composer while a turn runs, and the pure helpers
//! behind it.

use gpui::{AnyElement, div, prelude::*, px};

use super::super::AgentChatView;
use super::COLUMN_WIDTH;
use crate::domain::agent_chat::{ConfigCategory, ConfigOption};
use crate::ui::theme::colors;

impl AgentChatView {
    /// "✳ Opus 5.5 working for 1m 53s" while a turn runs. One muted line:
    /// only the glyph carries the accent.
    pub(super) fn working_line(&self) -> Option<AnyElement> {
        let started = self.turn_started?;
        let subject = model_name(&self.timeline.config_options).unwrap_or(self.agent.label());
        let elapsed = format_elapsed(started.elapsed().as_secs());
        Some(
            div()
                .w_full()
                .flex_none()
                .flex()
                .justify_center()
                .px_4()
                .pb_2()
                .child(
                    div()
                        .w_full()
                        .max_w(px(COLUMN_WIDTH))
                        .px_3()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_size(px(12.5))
                        .text_color(colors().muted)
                        .child(div().flex_none().text_color(colors().accent).child("✳"))
                        .child(format!("{subject} working for {elapsed}")),
                )
                .into_any_element(),
        )
    }
}

/// Display name of the current model, from the Model session option.
pub(super) fn model_name(options: &[ConfigOption]) -> Option<&str> {
    options
        .iter()
        .find(|option| option.category == ConfigCategory::Model)
        .map(ConfigOption::current_name)
        .filter(|name| !name.is_empty())
}

/// Elapsed time for the status line: "12s", "1m 53s", "1h 2m".
pub(super) fn format_elapsed(seconds: u64) -> String {
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m {}s", seconds / 60, seconds % 60),
        _ => format!("{}h {}m", seconds / 3600, (seconds % 3600) / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::agent_chat::ConfigValue;

    #[test]
    fn elapsed_time_drops_units_that_are_zero() {
        assert_eq!(format_elapsed(0), "0s");
        assert_eq!(format_elapsed(12), "12s");
        assert_eq!(format_elapsed(59), "59s");
        assert_eq!(format_elapsed(113), "1m 53s");
        assert_eq!(format_elapsed(3599), "59m 59s");
        assert_eq!(format_elapsed(3600), "1h 0m");
        assert_eq!(format_elapsed(3720), "1h 2m");
    }

    #[test]
    fn model_name_comes_from_the_model_option() {
        let model = ConfigOption {
            id: "model".into(),
            name: "Model".into(),
            category: ConfigCategory::Model,
            current: "opus".into(),
            values: vec![ConfigValue {
                value: "opus".into(),
                name: "Opus 5.5".into(),
                description: None,
            }],
        };
        let mode = ConfigOption {
            category: ConfigCategory::Mode,
            current: "auto".into(),
            values: Vec::new(),
            ..model.clone()
        };
        assert_eq!(model_name(&[mode.clone(), model]), Some("Opus 5.5"));
        assert_eq!(model_name(&[mode]), None);
        assert_eq!(model_name(&[]), None);
    }
}
