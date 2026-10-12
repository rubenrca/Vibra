//! Model and mode pickers on the composer toolbar. The choices come from the
//! agent (session modes, or config options for model, thinking level and
//! mode); choosing one sends it back to the agent.

use gpui::{
    AnyElement, Context, MouseButton, SharedString, deferred, div, prelude::*, px, relative, svg,
};

use super::super::AgentChatView;
use crate::domain::agent_chat::{ChatCommand, ConfigCategory, ConfigOption, SessionModes};
use crate::ui::menu::{MenuRow, menu_heading, menu_panel};
use crate::ui::theme::colors;

/// Which toolbar picker a menu belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PickerMenu {
    Model,
    Mode,
}

/// One choice in a picker menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PickerItem {
    pub label: String,
    pub checked: bool,
    pub command: ChatCommand,
}

/// Choices under an optional heading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PickerSection {
    pub heading: Option<String>,
    pub items: Vec<PickerItem>,
}

/// Model and thinking level options, in that order, when the agent offers them.
fn model_options(options: &[ConfigOption]) -> Vec<&ConfigOption> {
    [ConfigCategory::Model, ConfigCategory::ThoughtLevel]
        .into_iter()
        .filter_map(|category| options.iter().find(|option| option.category == category))
        .collect()
}

/// The model picker's text, for example "Opus 5.5 High". `None` when the agent
/// offers neither option.
pub(super) fn model_label(options: &[ConfigOption]) -> Option<String> {
    let parts: Vec<&str> = model_options(options)
        .into_iter()
        .map(ConfigOption::current_name)
        .collect();
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// Menu rows for one config option, checking its current value.
fn config_items(option: &ConfigOption) -> Vec<PickerItem> {
    option
        .values
        .iter()
        .map(|value| PickerItem {
            label: value.name.clone(),
            checked: value.value == option.current,
            command: ChatCommand::SetConfigOption {
                option_id: option.id.clone(),
                value: value.value.clone(),
            },
        })
        .collect()
}

/// The model menu: one section per option, headed when there are two.
fn model_sections(options: &[ConfigOption]) -> Vec<PickerSection> {
    let configs = model_options(options);
    let titled = configs.len() > 1;
    configs
        .into_iter()
        .map(|config| PickerSection {
            heading: titled.then(|| config.name.clone()),
            items: config_items(config),
        })
        .collect()
}

/// Session modes, when the agent offers any.
fn usable_modes(modes: Option<&SessionModes>) -> Option<&SessionModes> {
    modes.filter(|modes| !modes.available.is_empty())
}

/// The mode picker's text, for example "Auto": the current session mode, or
/// else a `Mode` config option. `None` when the agent offers no modes.
pub(super) fn mode_label(modes: Option<&SessionModes>, options: &[ConfigOption]) -> Option<String> {
    if let Some(modes) = usable_modes(modes) {
        let name = modes
            .available
            .iter()
            .find(|mode| mode.id == modes.current)
            .map_or(modes.current.as_str(), |mode| mode.name.as_str());
        return Some(name.to_owned());
    }
    options
        .iter()
        .find(|option| option.category == ConfigCategory::Mode)
        .map(|option| option.current_name().to_owned())
}

/// The mode menu: session modes are set with `SetMode`; a `Mode` config option
/// is set like any other option.
fn mode_sections(modes: Option<&SessionModes>, options: &[ConfigOption]) -> Vec<PickerSection> {
    if let Some(modes) = usable_modes(modes) {
        let items = modes
            .available
            .iter()
            .map(|mode| PickerItem {
                label: mode.name.clone(),
                checked: mode.id == modes.current,
                command: ChatCommand::SetMode {
                    mode_id: mode.id.clone(),
                },
            })
            .collect();
        return vec![PickerSection {
            heading: None,
            items,
        }];
    }
    options
        .iter()
        .filter(|option| option.category == ConfigCategory::Mode)
        .map(|option| PickerSection {
            heading: None,
            items: config_items(option),
        })
        .collect()
}

impl AgentChatView {
    fn menu_sections(&self, kind: PickerMenu) -> Vec<PickerSection> {
        match kind {
            PickerMenu::Model => model_sections(&self.timeline.config_options),
            PickerMenu::Mode => {
                mode_sections(self.timeline.modes.as_ref(), &self.timeline.config_options)
            }
        }
    }

    /// A toolbar button that opens its menu above itself. A second press
    /// closes the menu; the composer's overlay catches presses elsewhere.
    pub(super) fn picker_button(
        &self,
        kind: PickerMenu,
        label: String,
        icon: Option<&'static str>,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = enabled && self.composer.menu == Some(kind);
        let button = div()
            .id(match kind {
                PickerMenu::Model => "agent-chat-model-picker",
                PickerMenu::Mode => "agent-chat-mode-picker",
            })
            .h(px(26.0))
            .max_w(px(240.0))
            .px(px(7.0))
            .flex()
            .items_center()
            .gap(px(5.0))
            .rounded(px(7.0))
            .text_size(px(12.0))
            .text_color(if open {
                colors().foreground
            } else {
                colors().muted
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when(enabled, |button| {
                button
                    .cursor_pointer()
                    .hover(|button| button.bg(colors().hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.composer.menu = if this.composer.menu == Some(kind) {
                            None
                        } else {
                            Some(kind)
                        };
                        cx.notify();
                    }))
            })
            .when(!enabled, |button| button.opacity(0.4))
            .when_some(icon, |button, icon| {
                button.child(
                    svg()
                        .path(icon)
                        .size(px(12.0))
                        .flex_none()
                        .text_color(colors().muted),
                )
            })
            .child(div().min_w(px(0.0)).truncate().child(label))
            .child(
                svg()
                    .path("chrome-icons/chevron-down.svg")
                    .size(px(10.0))
                    .flex_none()
                    .text_color(colors().subtle),
            );
        div()
            .relative()
            .flex_none()
            .child(button)
            .when(open, |wrapper| {
                wrapper.child(deferred(self.picker_menu(kind, cx)))
            })
            .into_any_element()
    }

    /// The menu list. Choosing a row sends it to the agent and closes the menu.
    fn picker_menu(&self, kind: PickerMenu, cx: &mut Context<Self>) -> AnyElement {
        let mut rows: Vec<AnyElement> = Vec::new();
        for (section_index, section) in self.menu_sections(kind).into_iter().enumerate() {
            if let Some(heading) = section.heading {
                rows.push(menu_heading(heading).into_any_element());
            }
            for (item_index, item) in section.items.into_iter().enumerate() {
                let command = item.command;
                rows.push(
                    MenuRow::new(item.label)
                        .checked(item.checked)
                        .render(SharedString::from(format!(
                            "agent-chat-picker-{section_index}-{item_index}"
                        )))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.composer.menu = None;
                            this.send(command.clone(), cx);
                            cx.notify();
                        }))
                        .into_any_element(),
                );
            }
        }
        menu_panel()
            .absolute()
            .bottom(relative(1.0))
            .mb(px(6.0))
            .left_0()
            .occlude()
            .children(rows)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::agent_chat::{ConfigValue, SessionMode};

    fn value(value: &str, name: &str) -> ConfigValue {
        ConfigValue {
            value: value.into(),
            name: name.into(),
            description: None,
        }
    }

    fn config(
        id: &str,
        category: ConfigCategory,
        current: &str,
        values: Vec<ConfigValue>,
    ) -> ConfigOption {
        ConfigOption {
            id: id.into(),
            name: format!("{id} option"),
            category,
            current: current.into(),
            values,
        }
    }

    fn model_and_thinking() -> Vec<ConfigOption> {
        vec![
            config(
                "model",
                ConfigCategory::Model,
                "opus-5.5",
                vec![value("opus-5.5", "Opus 5.5"), value("sonnet", "Sonnet")],
            ),
            config(
                "thought",
                ConfigCategory::ThoughtLevel,
                "high",
                vec![value("low", "Low"), value("high", "High")],
            ),
        ]
    }

    #[test]
    fn model_label_joins_model_and_thinking_level() {
        assert_eq!(
            model_label(&model_and_thinking()).as_deref(),
            Some("Opus 5.5 High")
        );
        let mode_only = [config(
            "mode",
            ConfigCategory::Mode,
            "auto",
            vec![value("auto", "Auto")],
        )];
        assert_eq!(model_label(&mode_only), None);
    }

    #[test]
    fn model_menu_has_two_headed_sections_when_both_options_exist() {
        let sections = model_sections(&model_and_thinking());
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].heading.as_deref(), Some("model option"));
        assert!(sections[0].items[0].checked);
        assert_eq!(
            sections[1].items[1].command,
            ChatCommand::SetConfigOption {
                option_id: "thought".into(),
                value: "high".into(),
            }
        );
        let single = model_sections(&model_and_thinking()[..1]);
        assert_eq!(single.len(), 1);
        assert_eq!(single[0].heading, None);
    }

    #[test]
    fn mode_label_prefers_session_modes_over_config_options() {
        let modes = SessionModes {
            current: "auto".into(),
            available: vec![
                SessionMode {
                    id: "ask".into(),
                    name: "Ask".into(),
                    description: None,
                },
                SessionMode {
                    id: "auto".into(),
                    name: "Auto".into(),
                    description: None,
                },
            ],
        };
        let options = [config(
            "mode",
            ConfigCategory::Mode,
            "plan",
            vec![value("plan", "Plan")],
        )];
        assert_eq!(mode_label(Some(&modes), &options).as_deref(), Some("Auto"));
        let sections = mode_sections(Some(&modes), &options);
        assert_eq!(sections.len(), 1);
        assert!(sections[0].items[1].checked);
        assert_eq!(
            sections[0].items[0].command,
            ChatCommand::SetMode {
                mode_id: "ask".into()
            }
        );
    }

    #[test]
    fn mode_label_falls_back_to_a_mode_config_option() {
        let options = [config(
            "mode",
            ConfigCategory::Mode,
            "plan",
            vec![value("plan", "Plan")],
        )];
        assert_eq!(mode_label(None, &options).as_deref(), Some("Plan"));
        assert_eq!(mode_label(None, &model_and_thinking()), None);
        let sections = mode_sections(None, &options);
        assert_eq!(
            sections[0].items[0].command,
            ChatCommand::SetConfigOption {
                option_id: "mode".into(),
                value: "plan".into(),
            }
        );
    }
}
