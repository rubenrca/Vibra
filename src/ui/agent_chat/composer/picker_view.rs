//! The model and mode menus on the composer toolbar: the trigger buttons, the
//! popup menus (the model menu has a provider rail beside its list), and what
//! their rows and keys do. The rules they follow live in `picker.rs`.

use gpui::{
    AnyElement, Context, Div, MouseButton, SharedString, Stateful, deferred, div, prelude::*, px,
    relative, svg,
};

use super::super::tooltip::tooltip;
use super::super::{AgentChatView, AgentChatViewEvent};
use super::picker::{
    MenuKey, MenuOutcome, OpenMenu, PickerAction, PickerMenu, PickerSection, ProviderTab,
    action_at, child_index, mode_sections, model_sections, model_trigger_label, navigate,
    provider_tabs, row_count, selected_index,
};
use crate::domain::agent_chat::ChatAgent;
use crate::ui::agent_marks::agent_compact_badge;
use crate::ui::menu::{MenuRow, menu_heading, menu_hover, menu_panel, menu_separator};
use crate::ui::theme::colors;

/// The model menu: a provider rail beside the list.
const MODEL_MENU_WIDTH: f32 = 280.0;
const RAIL_WIDTH: f32 = 46.0;
/// Longer lists scroll, and the highlighted row is scrolled into view.
const LIST_MAX_HEIGHT: f32 = 300.0;

fn menu_name(kind: PickerMenu) -> &'static str {
    match kind {
        PickerMenu::Model => "model",
        PickerMenu::Mode => "mode",
    }
}

impl AgentChatView {
    /// Rows the open menu lists: the models of `provider`, or the session modes.
    fn menu_sections(&self, kind: PickerMenu, provider: ChatAgent) -> Vec<PickerSection> {
        match kind {
            PickerMenu::Model => model_sections(
                provider,
                self.agent,
                &self.timeline.config_options,
                &self.model_catalog,
            ),
            PickerMenu::Mode => {
                mode_sections(self.timeline.modes.as_ref(), &self.timeline.config_options)
            }
        }
    }

    /// Provider tabs of the model menu. A locked chat keeps only its own agent enabled.
    fn model_tabs(&self) -> Vec<ProviderTab> {
        provider_tabs(self.agent, &self.available_agents, self.agent_locked())
    }

    /// Opens `kind`, or closes it when it is the open menu.
    pub(super) fn toggle_menu(&mut self, kind: PickerMenu, cx: &mut Context<Self>) {
        self.composer.menu = match self.composer.menu {
            Some(menu) if menu.kind == kind => None,
            _ => Some(self.open_menu(kind)),
        };
        cx.notify();
    }

    fn open_menu(&self, kind: PickerMenu) -> OpenMenu {
        let provider = self.agent;
        OpenMenu {
            kind,
            provider,
            cursor: selected_index(&self.menu_sections(kind, provider)),
        }
    }

    /// The model trigger: the agent's mark, then its model and thinking level.
    pub(super) fn model_trigger(&self, enabled: bool, cx: &mut Context<Self>) -> AnyElement {
        let label = model_trigger_label(self.agent, &self.timeline.config_options);
        let mark = agent_compact_badge(Some(self.agent.label()), None, None, true);
        self.picker_trigger(PickerMenu::Model, enabled, label, mark, cx)
    }

    /// The mode trigger: the current session mode.
    pub(super) fn mode_trigger(
        &self,
        label: String,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let icon = svg()
            .path("chrome-icons/shield.svg")
            .size(px(12.0))
            .flex_none()
            .text_color(colors().muted)
            .into_any_element();
        self.picker_trigger(PickerMenu::Mode, enabled, label, icon, cx)
    }

    /// A quiet text button that opens its menu above itself. A press outside
    /// the open menu closes it through the composer's overlay.
    fn picker_trigger(
        &self,
        kind: PickerMenu,
        enabled: bool,
        label: String,
        leading: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self
            .composer
            .menu
            .filter(|menu| menu.kind == kind && enabled);
        let button = div()
            .id(match kind {
                PickerMenu::Model => "agent-chat-model-picker",
                PickerMenu::Mode => "agent-chat-mode-picker",
            })
            .h(px(24.0))
            .max_w(px(260.0))
            .px(px(6.0))
            .flex()
            .items_center()
            .gap(px(5.0))
            .rounded(px(7.0))
            .text_size(px(12.0))
            .text_color(if open.is_some() {
                colors().foreground
            } else {
                colors().muted
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when(enabled, |button| {
                button
                    .cursor_pointer()
                    .hover(|button| button.bg(colors().hover))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        // The menu takes the arrow keys, so the chat must hold focus.
                        this.focus_handle.focus(window);
                        this.toggle_menu(kind, cx);
                    }))
            })
            .when(!enabled, |button| button.opacity(0.4))
            .child(leading)
            .child(div().min_w(px(0.0)).truncate().child(label))
            .child(
                svg()
                    .path("chrome-icons/chevron-down.svg")
                    .size(px(9.0))
                    .flex_none()
                    .text_color(colors().subtle),
            );
        let popup = open.map(|menu| deferred(self.picker_menu(menu, cx)));
        div()
            .relative()
            .flex_none()
            .child(button)
            .when_some(popup, |wrapper, popup| wrapper.child(popup))
            .into_any_element()
    }

    /// The popup above a trigger. Models get the provider rail beside the list.
    fn picker_menu(&self, menu: OpenMenu, cx: &mut Context<Self>) -> AnyElement {
        let sections = self.menu_sections(menu.kind, menu.provider);
        let list = self.menu_list(menu, &sections, cx);
        let panel = match menu.kind {
            PickerMenu::Model => {
                let rail = self.provider_rail(menu.provider, cx);
                menu_panel()
                    .flex_row()
                    .p(px(0.0))
                    .w(px(MODEL_MENU_WIDTH))
                    .child(rail)
                    .child(div().flex_1().min_w(px(0.0)).child(list))
            }
            PickerMenu::Mode => menu_panel().child(list),
        };
        panel
            .absolute()
            .bottom(relative(1.0))
            .mb(px(6.0))
            .left_0()
            .occlude()
            .into_any_element()
    }

    /// The rows of the open menu under their headings. The highlighted row is
    /// tinted like a hovered one; a click runs the row.
    fn menu_list(
        &self,
        menu: OpenMenu,
        sections: &[PickerSection],
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let mut children: Vec<AnyElement> = Vec::new();
        if sections.is_empty() {
            children.push(
                div()
                    .h(px(28.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .text_size(px(12.5))
                    .text_color(colors().subtle)
                    .child("No models reported yet.")
                    .into_any_element(),
            );
        }
        let mut row = 0;
        for (section_index, section) in sections.iter().enumerate() {
            if section_index > 0 {
                children.push(menu_separator().into_any_element());
            }
            if let Some(heading) = &section.heading {
                children.push(menu_heading(heading.clone()).into_any_element());
            }
            for item in &section.items {
                let index = row;
                row += 1;
                children.push(
                    MenuRow::new(item.label.clone())
                        .checked(item.checked)
                        .render(SharedString::from(format!(
                            "agent-chat-{}-row-{index}",
                            menu_name(menu.kind)
                        )))
                        .when(index == menu.cursor, |row| row.bg(menu_hover()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.choose_menu_item(menu.kind, menu.provider, index, cx);
                        }))
                        .into_any_element(),
                );
            }
        }
        div()
            .id(SharedString::from(format!(
                "agent-chat-{}-list",
                menu_name(menu.kind)
            )))
            .flex()
            .flex_col()
            .p(px(4.0))
            .max_h(px(LIST_MAX_HEIGHT))
            .overflow_y_scroll()
            .track_scroll(&self.composer.menu_scroll)
            .children(children)
    }

    /// The provider tabs down the left of the model menu. A disabled tab says why on hover.
    fn provider_rail(&self, selected: ChatAgent, cx: &mut Context<Self>) -> AnyElement {
        div()
            .w(px(RAIL_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(4.0))
            .p(px(6.0))
            .border_r_1()
            .border_color(colors().border_subtle)
            .children(
                self.model_tabs()
                    .into_iter()
                    .map(|tab| self.provider_tab(tab, selected, cx)),
            )
            .into_any_element()
    }

    fn provider_tab(
        &self,
        tab: ProviderTab,
        selected: ChatAgent,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = tab.agent.label();
        let active = tab.agent == selected;
        let hint = if tab.enabled {
            label.to_owned()
        } else {
            format!("Start a new chat to use {label}")
        };
        div()
            .id(SharedString::from(format!(
                "agent-chat-provider-{}",
                label.to_lowercase()
            )))
            .size(px(32.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(8.0))
            .when(active, |item| item.bg(menu_hover()))
            .when(tab.enabled, |item| {
                item.cursor_pointer()
                    .hover(|item| item.bg(menu_hover()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_provider(tab.agent, cx);
                    }))
            })
            .when(!tab.enabled, |item| item.opacity(0.35))
            .tooltip(move |_, cx| tooltip(hint.clone(), cx))
            .child(agent_compact_badge(Some(label), None, None, active))
            .into_any_element()
    }

    /// Shows `provider`'s rows, highlighting its checked row.
    fn select_provider(&mut self, provider: ChatAgent, cx: &mut Context<Self>) {
        if let Some(mut menu) = self.composer.menu
            && menu.provider != provider
        {
            menu.provider = provider;
            menu.cursor = selected_index(&self.menu_sections(menu.kind, provider));
            self.composer.menu = Some(menu);
        }
        cx.notify();
    }

    /// Runs a row of an open menu and closes the menu.
    fn choose_menu_item(
        &mut self,
        kind: PickerMenu,
        provider: ChatAgent,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        self.composer.menu = None;
        let sections = self.menu_sections(kind, provider);
        if let Some(action) = action_at(&sections, index) {
            self.apply_picker_action(action, cx);
        }
        cx.notify();
    }

    fn apply_picker_action(&mut self, action: PickerAction, cx: &mut Context<Self>) {
        match action {
            PickerAction::Command(command) => self.send(command, cx),
            // Another agent's default model starts a new chat with that agent,
            // as long as no message has been sent.
            PickerAction::Model { agent, model } if model.is_empty() && agent != self.agent => {
                if !self.agent_locked() {
                    cx.emit(AgentChatViewEvent::SwitchAgent { agent, model: None });
                }
            }
            PickerAction::Model { agent, model } => self.choose_model(agent, model, cx),
        }
    }

    /// Keys for the open menu. Every menu key is used, so none reaches the draft.
    pub(super) fn handle_menu_key(&mut self, key: MenuKey, cx: &mut Context<Self>) {
        let Some(mut menu) = self.composer.menu else {
            return;
        };
        let previous = menu.provider;
        let tabs = if menu.kind == PickerMenu::Model {
            self.model_tabs()
        } else {
            Vec::new()
        };
        let rows = row_count(&self.menu_sections(menu.kind, menu.provider));
        match navigate(&mut menu, key, &tabs, rows) {
            MenuOutcome::Handled => {
                if menu.provider != previous {
                    menu.cursor = selected_index(&self.menu_sections(menu.kind, menu.provider));
                }
                self.composer.menu = Some(menu);
                let sections = self.menu_sections(menu.kind, menu.provider);
                if row_count(&sections) > 0 {
                    self.composer
                        .menu_scroll
                        .scroll_to_item(child_index(&sections, menu.cursor));
                }
            }
            MenuOutcome::Accept(index) => {
                self.choose_menu_item(menu.kind, menu.provider, index, cx);
            }
            MenuOutcome::Close => self.composer.menu = None,
        }
        cx.notify();
    }
}
