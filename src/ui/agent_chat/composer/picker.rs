//! Data behind the composer's pickers: the provider tabs and rows of the model
//! menu, the trigger labels, and the keyboard state machine for an open menu.
//! Pure, so the rules are tested without a window; `picker_view.rs` draws them.

use super::popups::step;
use crate::domain::agent_chat::{
    AgentModelCatalog, ChatAgent, ChatCommand, ConfigCategory, ConfigOption, SessionModes,
};

/// Heading over the thinking-level rows of the running agent's model menu.
const EFFORT_HEADING: &str = "Effort";
/// The one row an agent gets while its models are not known.
const DEFAULT_MODEL_LABEL: &str = "Default model";

/// Which toolbar picker a menu belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PickerMenu {
    Model,
    Mode,
}

/// What choosing a row does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PickerAction {
    /// Sent to the running agent as it is: a session mode or a config option.
    Command(ChatCommand),
    /// A model of `agent`. An empty `model` is that agent's default; only a
    /// new chat can start with it, so the view handles it apart from commands.
    Model { agent: ChatAgent, model: String },
}

/// One selectable row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PickerItem {
    pub label: String,
    pub checked: bool,
    pub action: PickerAction,
}

/// Rows under an optional heading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PickerSection {
    pub heading: Option<String>,
    pub items: Vec<PickerItem>,
}

/// One provider tab. A disabled tab stays visible but cannot be chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ProviderTab {
    pub agent: ChatAgent,
    pub enabled: bool,
}

/// The open menu: which picker, the provider whose rows it lists, and the
/// highlighted row (an index into the flattened rows).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct OpenMenu {
    pub kind: PickerMenu,
    pub provider: ChatAgent,
    pub cursor: usize,
}

/// The keys an open menu reacts to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MenuKey {
    Up,
    Down,
    Left,
    Right,
    Accept,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MenuOutcome {
    /// The highlight or provider moved, or the key had nothing to do.
    Handled,
    /// Run the row at this flat index.
    Accept(usize),
    Close,
}

fn option_of(options: &[ConfigOption], category: ConfigCategory) -> Option<&ConfigOption> {
    options.iter().find(|option| option.category == category)
}

/// The provider tabs: the running agent and the available agents, in the order
/// of `ChatAgent::ALL`. Once the chat is locked, only the running agent is enabled.
pub(super) fn provider_tabs(
    running: ChatAgent,
    available: &[ChatAgent],
    locked: bool,
) -> Vec<ProviderTab> {
    ChatAgent::ALL
        .into_iter()
        .filter(|agent| *agent == running || available.contains(agent))
        .map(|agent| ProviderTab {
            agent,
            enabled: agent == running || !locked,
        })
        .collect()
}

/// The provider the arrow keys reach from `selected`: the next enabled tab in
/// the direction of `delta`, wrapping around. Stays put without another enabled tab.
pub(super) fn next_provider(tabs: &[ProviderTab], selected: ChatAgent, delta: isize) -> ChatAgent {
    let Some(start) = tabs.iter().position(|tab| tab.agent == selected) else {
        return selected;
    };
    let len = tabs.len() as isize;
    for offset in 1..len {
        let index = (start as isize + delta.signum() * offset).rem_euclid(len) as usize;
        if tabs[index].enabled {
            return tabs[index].agent;
        }
    }
    selected
}

/// The model trigger's text, for example "Opus 5.5 · High": the model, then the
/// thinking level. The agent's name stands in while it reports neither.
pub(super) fn model_trigger_label(agent: ChatAgent, options: &[ConfigOption]) -> String {
    let parts: Vec<&str> = [ConfigCategory::Model, ConfigCategory::ThoughtLevel]
        .into_iter()
        .filter_map(|category| option_of(options, category))
        .map(ConfigOption::current_name)
        .filter(|name| !name.is_empty())
        .collect();
    if parts.is_empty() {
        agent.label().to_owned()
    } else {
        parts.join(" · ")
    }
}

/// Rows of the model menu for `provider`. The running agent lists its Model
/// option, then its thinking level under "Effort". Any other agent lists the
/// models remembered for it, or a single default row when there are none.
pub(super) fn model_sections(
    provider: ChatAgent,
    running: ChatAgent,
    options: &[ConfigOption],
    catalog: &[AgentModelCatalog],
) -> Vec<PickerSection> {
    if provider != running {
        let models = catalog
            .iter()
            .find(|entry| entry.agent == provider)
            .map(|entry| entry.models.as_slice())
            .unwrap_or_default();
        let items = if models.is_empty() {
            vec![PickerItem {
                label: DEFAULT_MODEL_LABEL.to_owned(),
                checked: false,
                action: PickerAction::Model {
                    agent: provider,
                    model: String::new(),
                },
            }]
        } else {
            models
                .iter()
                .map(|model| PickerItem {
                    label: model.name.clone(),
                    checked: false,
                    action: PickerAction::Model {
                        agent: provider,
                        model: model.value.clone(),
                    },
                })
                .collect()
        };
        return vec![PickerSection {
            heading: None,
            items,
        }];
    }

    let mut sections = Vec::new();
    if let Some(model) = option_of(options, ConfigCategory::Model) {
        sections.push(PickerSection {
            heading: None,
            items: model
                .values
                .iter()
                .map(|value| PickerItem {
                    label: value.name.clone(),
                    checked: value.value == model.current,
                    action: PickerAction::Model {
                        agent: provider,
                        model: value.value.clone(),
                    },
                })
                .collect(),
        });
    }
    if let Some(thought) = option_of(options, ConfigCategory::ThoughtLevel) {
        sections.push(PickerSection {
            heading: Some(EFFORT_HEADING.to_owned()),
            items: config_items(thought),
        });
    }
    sections
}

/// Rows that set a config option to one of its values.
fn config_items(option: &ConfigOption) -> Vec<PickerItem> {
    option
        .values
        .iter()
        .map(|value| PickerItem {
            label: value.name.clone(),
            checked: value.value == option.current,
            action: PickerAction::Command(ChatCommand::SetConfigOption {
                option_id: option.id.clone(),
                value: value.value.clone(),
            }),
        })
        .collect()
}

/// Usable session modes: a list the agent actually offers.
fn usable_modes(modes: Option<&SessionModes>) -> Option<&SessionModes> {
    modes.filter(|modes| !modes.available.is_empty())
}

/// The mode trigger's text, for example "Auto": the current session mode, or
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
    option_of(options, ConfigCategory::Mode).map(|option| option.current_name().to_owned())
}

/// Rows of the mode menu: session modes use `SetMode`; a `Mode` config option is
/// set like any other option.
pub(super) fn mode_sections(
    modes: Option<&SessionModes>,
    options: &[ConfigOption],
) -> Vec<PickerSection> {
    if let Some(modes) = usable_modes(modes) {
        let items = modes
            .available
            .iter()
            .map(|mode| PickerItem {
                label: mode.name.clone(),
                checked: mode.id == modes.current,
                action: PickerAction::Command(ChatCommand::SetMode {
                    mode_id: mode.id.clone(),
                }),
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

/// Number of selectable rows across all sections.
pub(super) fn row_count(sections: &[PickerSection]) -> usize {
    sections.iter().map(|section| section.items.len()).sum()
}

/// The action of the row at flat `index`.
pub(super) fn action_at(sections: &[PickerSection], index: usize) -> Option<PickerAction> {
    sections
        .iter()
        .flat_map(|section| section.items.iter())
        .nth(index)
        .map(|item| item.action.clone())
}

/// The highlight for a freshly opened menu: its checked row, else the first.
pub(super) fn selected_index(sections: &[PickerSection]) -> usize {
    sections
        .iter()
        .flat_map(|section| section.items.iter())
        .position(|item| item.checked)
        .unwrap_or(0)
}

/// Position, among the list's children, of the row at flat `cursor`. The list
/// holds a separator before every section but the first, then each heading and
/// its rows, so the scroll target has to count those too.
pub(super) fn child_index(sections: &[PickerSection], cursor: usize) -> usize {
    let mut children = 0;
    let mut row = 0;
    for (section_index, section) in sections.iter().enumerate() {
        if section_index > 0 {
            children += 1;
        }
        if section.heading.is_some() {
            children += 1;
        }
        for _ in &section.items {
            if row == cursor {
                return children;
            }
            row += 1;
            children += 1;
        }
    }
    children
}

/// Moves the highlight or the provider of `menu` for `key`. `rows` counts the
/// rows of the open provider; `tabs` are the provider tabs (empty for modes).
/// Changing provider highlights its first row.
pub(super) fn navigate(
    menu: &mut OpenMenu,
    key: MenuKey,
    tabs: &[ProviderTab],
    rows: usize,
) -> MenuOutcome {
    match key {
        MenuKey::Up | MenuKey::Down => {
            if rows > 0 {
                menu.cursor = step(menu.cursor.min(rows - 1), rows, key == MenuKey::Down);
            }
            MenuOutcome::Handled
        }
        MenuKey::Left | MenuKey::Right => {
            let delta = if key == MenuKey::Right { 1 } else { -1 };
            let provider = next_provider(tabs, menu.provider, delta);
            if provider != menu.provider {
                menu.provider = provider;
                menu.cursor = 0;
            }
            MenuOutcome::Handled
        }
        MenuKey::Accept if rows > 0 => MenuOutcome::Accept(menu.cursor.min(rows - 1)),
        MenuKey::Accept => MenuOutcome::Handled,
        MenuKey::Close => MenuOutcome::Close,
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

    fn remembered(agent: ChatAgent, names: &[(&str, &str)]) -> AgentModelCatalog {
        AgentModelCatalog {
            agent,
            models: names
                .iter()
                .map(|(value, name)| crate::domain::agent_chat::CatalogModel {
                    value: (*value).into(),
                    name: (*name).into(),
                })
                .collect(),
        }
    }

    fn menu(provider: ChatAgent, cursor: usize) -> OpenMenu {
        OpenMenu {
            kind: PickerMenu::Model,
            provider,
            cursor,
        }
    }

    #[test]
    fn provider_tabs_follow_the_chat_order_and_keep_the_running_agent() {
        let tabs = provider_tabs(
            ChatAgent::Codex,
            &[ChatAgent::Grok, ChatAgent::Claude],
            false,
        );
        let agents: Vec<ChatAgent> = tabs.iter().map(|tab| tab.agent).collect();
        assert_eq!(
            agents,
            [ChatAgent::Claude, ChatAgent::Codex, ChatAgent::Grok]
        );
        assert!(tabs.iter().all(|tab| tab.enabled));
        // Without the running agent in the list it still gets a tab.
        let only = provider_tabs(ChatAgent::Grok, &[], false);
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].agent, ChatAgent::Grok);
    }

    #[test]
    fn locked_chat_disables_every_other_provider() {
        let tabs = provider_tabs(ChatAgent::Claude, &ChatAgent::ALL, true);
        let enabled: Vec<ChatAgent> = tabs
            .iter()
            .filter(|tab| tab.enabled)
            .map(|tab| tab.agent)
            .collect();
        assert_eq!(enabled, [ChatAgent::Claude]);
        assert_eq!(tabs.len(), 3, "disabled tabs stay visible");
    }

    #[test]
    fn next_provider_skips_disabled_tabs_and_wraps() {
        let tabs = [
            ProviderTab {
                agent: ChatAgent::Claude,
                enabled: true,
            },
            ProviderTab {
                agent: ChatAgent::Codex,
                enabled: false,
            },
            ProviderTab {
                agent: ChatAgent::Grok,
                enabled: true,
            },
        ];
        assert_eq!(next_provider(&tabs, ChatAgent::Claude, 1), ChatAgent::Grok);
        assert_eq!(next_provider(&tabs, ChatAgent::Grok, 1), ChatAgent::Claude);
        assert_eq!(next_provider(&tabs, ChatAgent::Claude, -1), ChatAgent::Grok);
        let alone = [tabs[0], tabs[1]];
        assert_eq!(
            next_provider(&alone, ChatAgent::Claude, 1),
            ChatAgent::Claude
        );
        assert_eq!(next_provider(&[], ChatAgent::Claude, 1), ChatAgent::Claude);
    }

    #[test]
    fn trigger_label_joins_model_and_thinking_level() {
        assert_eq!(
            model_trigger_label(ChatAgent::Claude, &model_and_thinking()),
            "Opus 5.5 · High"
        );
        let model_only = &model_and_thinking()[..1];
        assert_eq!(
            model_trigger_label(ChatAgent::Claude, model_only),
            "Opus 5.5"
        );
        assert_eq!(model_trigger_label(ChatAgent::Codex, &[]), "Codex");
    }

    #[test]
    fn running_agent_lists_its_models_then_effort() {
        let sections = model_sections(
            ChatAgent::Claude,
            ChatAgent::Claude,
            &model_and_thinking(),
            &[],
        );
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].heading, None);
        assert!(sections[0].items[0].checked);
        assert_eq!(
            sections[0].items[1].action,
            PickerAction::Model {
                agent: ChatAgent::Claude,
                model: "sonnet".into(),
            }
        );
        assert_eq!(sections[1].heading.as_deref(), Some("Effort"));
        assert_eq!(
            sections[1].items[1].action,
            PickerAction::Command(ChatCommand::SetConfigOption {
                option_id: "thought".into(),
                value: "high".into(),
            })
        );
        assert!(sections[1].items[1].checked);
    }

    #[test]
    fn other_agents_list_remembered_models_or_one_default_row() {
        let catalog = [remembered(ChatAgent::Codex, &[("gpt-5", "GPT-5")])];
        let known = model_sections(ChatAgent::Codex, ChatAgent::Claude, &[], &catalog);
        assert_eq!(known.len(), 1);
        assert_eq!(known[0].items[0].label, "GPT-5");
        assert!(!known[0].items[0].checked);
        assert_eq!(
            known[0].items[0].action,
            PickerAction::Model {
                agent: ChatAgent::Codex,
                model: "gpt-5".into(),
            }
        );

        // Grok has no remembered models, so the only row is the default.
        let unknown = model_sections(ChatAgent::Grok, ChatAgent::Claude, &[], &catalog);
        assert_eq!(unknown.len(), 1);
        assert_eq!(unknown[0].items.len(), 1);
        assert_eq!(unknown[0].items[0].label, "Default model");
        assert_eq!(
            unknown[0].items[0].action,
            PickerAction::Model {
                agent: ChatAgent::Grok,
                model: String::new(),
            }
        );
    }

    #[test]
    fn running_agent_without_a_model_option_has_no_rows() {
        let sections = model_sections(ChatAgent::Claude, ChatAgent::Claude, &[], &[]);
        assert!(sections.is_empty());
        assert_eq!(row_count(&sections), 0);
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
            sections[0].items[0].action,
            PickerAction::Command(ChatCommand::SetMode {
                mode_id: "ask".into()
            })
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
            sections[0].items[0].action,
            PickerAction::Command(ChatCommand::SetConfigOption {
                option_id: "mode".into(),
                value: "plan".into(),
            })
        );
    }

    #[test]
    fn navigation_moves_wraps_and_accepts_the_highlight() {
        let tabs = provider_tabs(ChatAgent::Claude, &ChatAgent::ALL, false);
        let mut open = menu(ChatAgent::Claude, 0);
        assert_eq!(
            navigate(&mut open, MenuKey::Up, &tabs, 3),
            MenuOutcome::Handled
        );
        assert_eq!(open.cursor, 2, "Up from the first row wraps to the last");
        assert_eq!(
            navigate(&mut open, MenuKey::Down, &tabs, 3),
            MenuOutcome::Handled
        );
        assert_eq!(open.cursor, 0, "Down from the last row wraps to the first");
        navigate(&mut open, MenuKey::Down, &tabs, 3);
        assert_eq!(
            navigate(&mut open, MenuKey::Accept, &tabs, 3),
            MenuOutcome::Accept(1)
        );
        assert_eq!(
            navigate(&mut open, MenuKey::Close, &tabs, 3),
            MenuOutcome::Close
        );
    }

    #[test]
    fn arrow_keys_switch_provider_and_reset_the_highlight() {
        let tabs = provider_tabs(ChatAgent::Claude, &ChatAgent::ALL, false);
        let mut open = menu(ChatAgent::Claude, 4);
        assert_eq!(
            navigate(&mut open, MenuKey::Right, &tabs, 5),
            MenuOutcome::Handled
        );
        assert_eq!(open.provider, ChatAgent::Codex);
        assert_eq!(open.cursor, 0);
        navigate(&mut open, MenuKey::Left, &tabs, 5);
        navigate(&mut open, MenuKey::Left, &tabs, 5);
        assert_eq!(open.provider, ChatAgent::Grok, "Left from Claude wraps");
    }

    #[test]
    fn locked_chat_keeps_the_arrow_keys_on_the_running_provider() {
        let tabs = provider_tabs(ChatAgent::Codex, &ChatAgent::ALL, true);
        let mut open = menu(ChatAgent::Codex, 1);
        navigate(&mut open, MenuKey::Right, &tabs, 2);
        navigate(&mut open, MenuKey::Left, &tabs, 2);
        assert_eq!(open.provider, ChatAgent::Codex);
    }

    #[test]
    fn enter_on_an_empty_menu_is_consumed_without_accepting() {
        let mut open = menu(ChatAgent::Claude, 0);
        assert_eq!(
            navigate(&mut open, MenuKey::Accept, &[], 0),
            MenuOutcome::Handled
        );
        assert_eq!(
            navigate(&mut open, MenuKey::Down, &[], 0),
            MenuOutcome::Handled
        );
        assert_eq!(open.cursor, 0);
    }

    #[test]
    fn opened_menu_highlights_the_checked_row_and_maps_rows_to_list_children() {
        let sections = model_sections(
            ChatAgent::Claude,
            ChatAgent::Claude,
            &model_and_thinking(),
            &[],
        );
        // The thinking level is the checked row after the models.
        assert_eq!(selected_index(&sections), 0);
        let effort_first = [sections[1].clone()];
        assert_eq!(selected_index(&effort_first), 1);
        assert!(action_at(&sections, 3).is_some());
        assert_eq!(action_at(&sections, 4), None);

        // Models, separator, heading, then the effort rows.
        assert_eq!(child_index(&sections, 0), 0);
        assert_eq!(child_index(&sections, 1), 1);
        assert_eq!(
            child_index(&sections, 2),
            4,
            "separator and heading come first"
        );
        assert_eq!(child_index(&sections, 3), 5);
    }
}
