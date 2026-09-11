//! Workspace-level key routing for overlays (settings, menus, rename, palette).

use gpui::{Context, KeyDownEvent, Window};

impl super::WorkspaceView {
    pub(super) fn on_workspace_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.to_ascii_lowercase();
        if self.settings_open {
            if matches!(key.as_str(), "escape" | "esc") {
                if self.settings_page == super::SettingsPage::Appearance
                    && !self.theme_query.is_empty()
                {
                    self.theme_query.clear();
                    cx.notify();
                } else {
                    self.close_settings(cx);
                }
            } else if self.settings_page == super::SettingsPage::Appearance
                && !event.keystroke.modifiers.platform
                && !event.keystroke.modifiers.control
                && !event.keystroke.modifiers.alt
            {
                match key.as_str() {
                    "backspace" => {
                        self.theme_query.pop();
                        cx.notify();
                    }
                    _ => {
                        if let Some(text) = event.keystroke.key_char.as_ref() {
                            self.theme_query.push_str(text);
                            cx.notify();
                        }
                    }
                }
            }
            cx.stop_propagation();
            return;
        }
        if self.ide_menu_open {
            if matches!(key.as_str(), "escape" | "esc") {
                self.ide_menu_open = false;
                cx.notify();
            }
            cx.stop_propagation();
            return;
        }
        if self.context_menu.is_some() {
            if matches!(key.as_str(), "escape" | "esc") {
                self.close_context_menu(cx);
            }
            cx.stop_propagation();
            return;
        }
        if self.rename_prompt.is_some() {
            match key.as_str() {
                "escape" | "esc" => {
                    self.rename_prompt = None;
                    cx.notify();
                }
                "enter" | "return" => self.confirm_rename_prompt(cx),
                "backspace" => {
                    if let Some(prompt) = self.rename_prompt.as_mut() {
                        prompt.value.pop();
                        cx.notify();
                    }
                }
                _ if !event.keystroke.modifiers.platform
                    && !event.keystroke.modifiers.control
                    && !event.keystroke.modifiers.alt =>
                {
                    if let Some(text) = event.keystroke.key_char.as_ref()
                        && let Some(prompt) = self.rename_prompt.as_mut()
                    {
                        prompt.value.push_str(text);
                        cx.notify();
                    }
                }
                _ => {}
            }
            cx.stop_propagation();
            return;
        }
        if self.palette_mode.is_some() {
            match key.as_str() {
                "escape" | "esc" => {
                    self.palette_mode = None;
                    cx.notify();
                }
                "up" => {
                    self.palette_selected = self.palette_selected.saturating_sub(1);
                    cx.notify();
                }
                "down" => {
                    let count = self.palette_items().len();
                    self.palette_selected =
                        (self.palette_selected + 1).min(count.saturating_sub(1));
                    cx.notify();
                }
                "enter" | "return" => {
                    let items = self.palette_items();
                    if let Some(item) = items.get(self.palette_selected) {
                        self.execute_palette_action(item.action.clone(), window, cx);
                    }
                }
                "backspace" => {
                    self.palette_query.pop();
                    self.palette_selected = 0;
                    cx.notify();
                }
                _ if !event.keystroke.modifiers.platform
                    && !event.keystroke.modifiers.control
                    && !event.keystroke.modifiers.alt =>
                {
                    if let Some(text) = event.keystroke.key_char.as_ref() {
                        self.palette_query.push_str(text);
                        self.palette_selected = 0;
                        cx.notify();
                    }
                }
                _ => {}
            }
            cx.stop_propagation();
        }
    }
}
