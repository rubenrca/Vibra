//! Settings navigation, pages, controls, and application of workspace preferences.

mod appearance;

use gpui::{AnyElement, Context, Div, SharedString, Stateful, Window, div, prelude::*, px, svg};

use crate::ui::theme::{self, AppearanceMode, MONO_FONT, ThemeTone, colors, surface, surface_tint};

use super::{WorkspaceSection, WorkspaceView};
use crate::infrastructure::automation::{
    AgentHookStatus, agent_hook_status, install_agent_hooks, uninstall_agent_hooks,
};
use crate::infrastructure::settings::{MAX_DIFF_FONT_SIZE, MIN_DIFF_FONT_SIZE};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SettingsPage {
    General,
    Appearance,
    Agents,
    Security,
    Inbox,
}

impl SettingsPage {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Appearance => "Appearance",
            Self::Agents => "Agents",
            Self::Security => "Privacy",
            Self::Inbox => "Review",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::General => "settings-general",
            Self::Appearance => "settings-appearance",
            Self::Agents => "settings-agents",
            Self::Security => "settings-privacy",
            Self::Inbox => "settings-inbox",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::General => "Choose how Vibra opens and keeps you up to date.",
            Self::Appearance => "Make Vibra your own, from colors to terminal text.",
            Self::Agents => "Connect the assistants working in your terminals.",
            Self::Security => "Control how terminal integrations access your Mac.",
            Self::Inbox => "Connect the services that bring your work into Vibra.",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::General => "chrome-icons/settings.svg",
            Self::Appearance => "chrome-icons/palette.svg",
            Self::Agents => "chrome-icons/sparkles.svg",
            Self::Security => "chrome-icons/shield.svg",
            Self::Inbox => "chrome-icons/inbox.svg",
        }
    }
}

#[derive(Clone, Copy)]
struct SettingsToggleRow {
    label: &'static str,
    description: &'static str,
    enabled: bool,
    divider: bool,
    id: &'static str,
}

struct FontSizeRow {
    label: &'static str,
    description: &'static str,
    size: f32,
    default: f32,
    ids: [&'static str; 3],
}

fn settings_card() -> Div {
    div()
        .rounded(px(12.0))
        .border_1()
        .border_color(colors().border_subtle)
        .bg(surface_tint(colors().elevated, colors().background))
}

fn settings_row_label(label: &'static str, description: &'static str) -> Div {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_size(px(13.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(colors().foreground)
                .child(label),
        )
        .child(
            div()
                .text_size(px(12.0))
                .line_height(px(19.0))
                .text_color(colors().subtle)
                .child(description),
        )
}

fn settings_button_base(label: &'static str, id: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(32.0))
        .flex_none()
        .whitespace_nowrap()
        .px_3()
        .rounded(px(5.0))
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(12.0))
        .child(label)
}

impl WorkspaceView {
    fn settings_section_heading(
        &self,
        title: &'static str,
        description: &'static str,
    ) -> AnyElement {
        div()
            .px_1()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_size(px(14.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(colors().foreground)
                    .child(title),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .line_height(px(19.0))
                    .text_color(colors().subtle)
                    .child(description),
            )
            .into_any_element()
    }

    fn settings_status_chip(&self, label: &'static str, active: bool) -> AnyElement {
        div()
            .flex_none()
            .whitespace_nowrap()
            .px_2()
            .py_1()
            .rounded(px(4.0))
            .bg(if active {
                colors().diff_added_bg
            } else {
                colors().selection
            })
            .text_size(px(11.0))
            .text_color(if active {
                colors().success
            } else {
                colors().muted
            })
            .child(label)
            .into_any_element()
    }

    fn settings_hook_status_row(&self, label: &'static str, installed: bool) -> AnyElement {
        div()
            .flex()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .text_size(px(13.0))
                    .text_color(colors().muted)
                    .child(label),
            )
            .child(self.settings_status_chip(
                if installed {
                    "Installed"
                } else {
                    "Not installed"
                },
                installed,
            ))
            .into_any_element()
    }

    fn settings_button(
        &self,
        label: &'static str,
        id: &'static str,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        settings_button_base(label, id)
            .bg(surface_tint(colors().selection, colors().elevated))
            .border_1()
            .border_color(colors().border_subtle)
            .text_color(colors().muted)
            .hover(|button| {
                button
                    .bg(surface_tint(colors().hover, colors().elevated))
                    .text_color(colors().foreground)
            })
            .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
    }

    fn settings_primary_button(
        &self,
        label: &'static str,
        id: &'static str,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        settings_button_base(label, id)
            .bg(colors().accent)
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(colors().background)
            .hover(|button| button.opacity(0.88))
            .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
    }

    fn settings_toggle_row(
        &self,
        row: SettingsToggleRow,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        div()
            .id(row.id)
            .min_h(px(82.0))
            .px_5()
            .py_4()
            .flex()
            .items_center()
            .gap_3()
            .cursor_pointer()
            .when(row.divider, |row| {
                row.border_b_1().border_color(colors().border_subtle)
            })
            .hover(|row| row.bg(surface_tint(colors().hover, colors().elevated)))
            .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
            .child(settings_row_label(row.label, row.description).min_w(px(0.0)))
            .child(
                div()
                    .w(px(36.0))
                    .h(px(22.0))
                    .flex_none()
                    .p(px(2.0))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_end()
                    .bg(if row.enabled {
                        colors().accent
                    } else {
                        colors().selection
                    })
                    .when(!row.enabled, |toggle| toggle.justify_start())
                    .child(
                        div()
                            .size(px(18.0))
                            .flex_none()
                            .rounded_full()
                            .bg(gpui::rgb(0xffffff)),
                    ),
            )
    }

    pub(super) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_palette(cx);
        self.rename_prompt = None;
        self.select_section(WorkspaceSection::Settings, window, cx);
    }

    pub(super) fn leave_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.navigation.can_go_back() {
            self.navigate_back(&crate::NavigateBack, window, cx);
        }
        if self.workspace_section == WorkspaceSection::Settings {
            self.select_section(WorkspaceSection::Workspace, window, cx);
        }
    }

    pub(super) fn settings_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let groups: &[(&str, &[SettingsPage])] = &[
            (
                "App",
                &[
                    SettingsPage::General,
                    SettingsPage::Appearance,
                    SettingsPage::Security,
                ],
            ),
            ("Agents", &[SettingsPage::Agents]),
            ("Workspace", &[SettingsPage::Inbox]),
        ];
        let mut navigation = div()
            .id("settings-navigation")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .px(px(10.0))
            .py_4()
            .flex()
            .flex_col()
            .gap_5();
        for (label, pages) in groups {
            navigation = navigation.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .px_2()
                            .pb_1()
                            .text_size(px(11.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(colors().subtle)
                            .child(*label),
                    )
                    .children(pages.iter().map(|&page| {
                        let selected = self.settings_page == page;
                        div()
                            .id(page.id())
                            .h(px(34.0))
                            .px_2()
                            .rounded(px(7.0))
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .cursor_pointer()
                            .text_size(px(13.0))
                            .text_color(if selected {
                                colors().foreground
                            } else {
                                colors().muted
                            })
                            .when(selected, |row| {
                                row.bg(surface_tint(colors().selection, colors().sidebar))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                            })
                            .hover(|row| row.bg(surface_tint(colors().hover, colors().sidebar)))
                            .child(
                                svg()
                                    .path(page.icon())
                                    .size(px(16.0))
                                    .flex_none()
                                    .text_color(if selected {
                                        colors().foreground
                                    } else {
                                        colors().muted
                                    }),
                            )
                            .child(page.label())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.settings_page = page;
                                if page == SettingsPage::Appearance {
                                    theme::refresh_user_themes();
                                }
                                cx.notify();
                            }))
                    })),
            );
        }
        div()
            .w(px(self.left_sidebar_width()))
            .h_full()
            .flex_none()
            .bg(surface(colors().sidebar))
            .border_r_1()
            .border_color(colors().border_subtle)
            .flex()
            .flex_col()
            .child(navigation)
            .child(
                div().p(px(10.0)).child(
                    div()
                        .id("settings-back")
                        .h(px(34.0))
                        .px_2()
                        .rounded(px(7.0))
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .cursor_pointer()
                        .text_size(px(13.0))
                        .text_color(colors().muted)
                        .hover(|row| row.bg(surface_tint(colors().hover, colors().sidebar)))
                        .child(
                            svg()
                                .path("chrome-icons/chevron-left.svg")
                                .size(px(16.0))
                                .text_color(colors().muted),
                        )
                        .child("Back")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.leave_settings(window, cx)),
                        ),
                ),
            )
            .into_any_element()
    }

    pub(super) fn settings_content(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let panel = div()
            .id(self.settings_page.id())
            .w_full()
            .max_w(px(1120.0))
            .min_w(px(0.0))
            .flex_none()
            .px(px(40.0))
            .pt(px(36.0))
            .pb(px(48.0))
            .flex()
            .flex_col()
            .gap(px(24.0))
            .child(
                div()
                    .mb_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(23.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(self.settings_page.label()),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .line_height(px(20.0))
                            .text_color(colors().muted)
                            .child(self.settings_page.description()),
                    ),
            );
        let panel = match self.settings_page {
            SettingsPage::Appearance => self.appearance_settings(panel, window, cx),
            SettingsPage::General => self.general_settings(panel, cx),
            SettingsPage::Agents => self.agent_settings(panel, cx),
            SettingsPage::Security => self.security_settings(panel),
            SettingsPage::Inbox => panel
                .child(self.settings_section_heading(
                    "Connections",
                    concat!(
                        "GitHub uses your gh session. Connect Linear with a personal API key ",
                        "stored only on this Mac.",
                    ),
                ))
                .child(
                    settings_card()
                        .py_3()
                        .child(self.inbox_connection_controls(cx)),
                ),
        };
        div()
            .id(SharedString::from(format!(
                "{}-scroll",
                self.settings_page.id()
            )))
            .flex_1()
            .h_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .bg(surface(colors().background))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .child(panel)
            .into_any_element()
    }

    fn settings_font_row(
        &self,
        row: FontSizeRow,
        cx: &mut Context<Self>,
        set: impl Fn(&mut Self, f32, &mut Context<Self>) + Copy + 'static,
    ) -> Div {
        let size = row.size;
        let [down, reset, up] = row.ids;
        div()
            .flex()
            .items_center()
            .gap_3()
            .child(settings_row_label(row.label, row.description))
            .child(
                div()
                    .w(px(52.0))
                    .h(px(32.0))
                    .flex_none()
                    .rounded(px(5.0))
                    .bg(surface_tint(colors().terminal, colors().elevated))
                    .border_1()
                    .border_color(colors().border_subtle)
                    .flex()
                    .items_center()
                    .justify_center()
                    .font_family(MONO_FONT)
                    .text_size(px(12.0))
                    .text_color(colors().foreground)
                    .child(format!("{size:.0} px")),
            )
            .child(self.settings_button("−", down, cx, move |this, cx| {
                set(this, size - 1.0, cx);
            }))
            .child(self.settings_button("Reset", reset, cx, {
                let default = row.default;
                move |this, cx| {
                    set(this, default, cx);
                }
            }))
            .child(self.settings_button("+", up, cx, move |this, cx| {
                set(this, size + 1.0, cx);
            }))
    }

    fn general_settings(&self, panel: Stateful<Div>, cx: &mut Context<Self>) -> Stateful<Div> {
        let startup = [
            SettingsToggleRow {
                label: "Hidden files",
                description: "Include files and folders whose names start with a dot.",
                enabled: self.settings.show_hidden_files,
                divider: true,
                id: "settings-hidden",
            },
            SettingsToggleRow {
                label: "Global navigation",
                description: "Show projects and global shortcuts when the app opens.",
                enabled: self.settings.left_sidebar_visible,
                divider: true,
                id: "settings-sidebar-visible",
            },
            SettingsToggleRow {
                label: "Workspace panel",
                description: "Show Explorer and Changes on the right.",
                enabled: self.settings.right_sidebar_visible,
                divider: false,
                id: "settings-git-visible",
            },
        ];
        let mut startup_card = settings_card().overflow_hidden();
        for row in startup {
            let id = row.id;
            startup_card =
                startup_card.child(self.settings_toggle_row(row, cx, move |this, cx| {
                    this.toggle_general_setting(id, cx);
                }));
        }
        panel
            .child(self.settings_section_heading(
                "Alerts",
                "Choose how Vibra gets your attention while you work elsewhere.",
            ))
            .child(
                settings_card()
                    .overflow_hidden()
                    .child(self.settings_toggle_row(
                        SettingsToggleRow {
                            label: "Activity notifications",
                            description: concat!(
                                "Notify when an agent finishes or needs attention ",
                                "outside the current pane.",
                            ),
                            enabled: self.settings.agent_notifications,
                            divider: false,
                            id: "settings-agent-notifications",
                        },
                        cx,
                        |this, cx| this.toggle_general_setting("settings-agent-notifications", cx),
                    )),
            )
            .child(self.settings_section_heading(
                "On startup",
                "Choose which elements appear when Vibra opens.",
            ))
            .child(startup_card)
    }

    fn toggle_general_setting(&mut self, id: &'static str, cx: &mut Context<Self>) {
        match id {
            "settings-agent-notifications" => {
                self.settings.agent_notifications = !self.settings.agent_notifications;
                if self.settings.agent_notifications {
                    crate::infrastructure::notifications::request_authorization();
                }
                self.persist_settings(cx);
            }
            "settings-hidden" => {
                self.settings.show_hidden_files = !self.settings.show_hidden_files;
                self.refresh_project_files(cx);
                self.persist_settings(cx);
            }
            "settings-sidebar-visible" => {
                self.set_left_sidebar_visible(!self.settings.left_sidebar_visible, true, cx);
            }
            "settings-git-visible" => {
                let open = !self.settings.right_sidebar_visible;
                self.set_right_sidebar_visible(open, true, cx);
                if open {
                    self.sync_diff_root(cx);
                }
            }
            _ => {}
        }
    }

    fn agent_settings(&self, panel: Stateful<Div>, cx: &mut Context<Self>) -> Stateful<Div> {
        let agent_hooks = self.agent_hook_status.unwrap_or_default();
        let all_agent_hooks_installed = agent_hooks.all_installed();
        let any_agent_hooks_installed = agent_hooks.any_installed();
        let agent_hooks_status = if all_agent_hooks_installed {
            "Configured"
        } else if any_agent_hooks_installed {
            "Partially configured"
        } else {
            "Optional"
        };
        panel
            .child(self.settings_section_heading(
                "Agent activity",
                "Track the status of assistants running inside Vibra.",
            ))
            .child(
                settings_card()
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(13.0))
                                    .text_color(colors().foreground)
                                    .child("Automatic detection"),
                            )
                            .child(self.settings_status_chip("Always on", true)),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .line_height(px(20.0))
                            .text_color(colors().subtle)
                            .child(concat!(
                                "Vibra recognizes agents running in its terminals ",
                                "and shows their activity in panes and tabs."
                            )),
                    )
                    .child(div().h(px(1.0)).bg(colors().border_subtle))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(13.0))
                                    .text_color(colors().foreground)
                                    .child("Hooks for precise status"),
                            )
                            .child(self.settings_status_chip(
                                agent_hooks_status,
                                all_agent_hooks_installed,
                            )),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .line_height(px(20.0))
                            .text_color(colors().subtle)
                            .child(concat!(
                                "Optional: install hooks so Claude and Codex report when they ",
                                "work, finish, or ask for permission. Other agents are detected ",
                                "through their process and screen output."
                            )),
                    )
                    .child(self.settings_hook_status_row("Claude", agent_hooks.claude_installed))
                    .child(self.settings_hook_status_row("Codex", agent_hooks.codex_installed))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .line_height(px(20.0))
                            .text_color(colors().subtle)
                            .child("In Codex, approve the configuration once using /hooks."),
                    )
                    .when_some(self.agent_hook_error.clone(), |card, error| {
                        card.child(
                            div()
                                .text_size(px(12.0))
                                .line_height(px(20.0))
                                .text_color(colors().danger)
                                .child(error),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(self.settings_primary_button(
                                if all_agent_hooks_installed {
                                    "Update hooks"
                                } else if any_agent_hooks_installed {
                                    "Install missing hooks"
                                } else {
                                    "Install hooks"
                                },
                                "settings-agent-hooks-install",
                                cx,
                                |this, cx| this.install_agent_hooks_from_settings(cx),
                            ))
                            .when(any_agent_hooks_installed, |buttons| {
                                buttons.child(self.settings_button(
                                    "Disable",
                                    "settings-agent-hooks-uninstall",
                                    cx,
                                    |this, cx| this.uninstall_agent_hooks_from_settings(cx),
                                ))
                            }),
                    ),
            )
    }

    fn security_settings(&self, panel: Stateful<Div>) -> Stateful<Div> {
        panel
            .child(self.settings_section_heading(
                "Terminal privacy",
                "Protections for local terminal integrations.",
            ))
            .child(
                settings_card()
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(13.0))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(colors().foreground)
                                    .child("Clipboard access (OSC 52)"),
                            )
                            .child(self.settings_status_chip("Confirmation required", true)),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .line_height(px(20.0))
                            .text_color(colors().subtle)
                            .child(concat!(
                                "Each read requires confirmation. Local communication uses ",
                                "a private socket and a separate token for each pane."
                            )),
                    ),
            )
    }

    fn apply_agent_hook_result(
        &mut self,
        result: anyhow::Result<AgentHookStatus>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(status) => {
                self.agent_hook_status = Some(status);
                self.agent_hook_error = None;
            }
            Err(error) => {
                if let Ok(status) = agent_hook_status() {
                    self.agent_hook_status = Some(status);
                }
                self.agent_hook_error =
                    Some(format!("Could not update integrations: {error}").into());
            }
        }
        cx.notify();
    }

    fn install_agent_hooks_from_settings(&mut self, cx: &mut Context<Self>) {
        self.apply_agent_hook_result(install_agent_hooks(), cx);
    }

    fn uninstall_agent_hooks_from_settings(&mut self, cx: &mut Context<Self>) {
        self.apply_agent_hook_result(uninstall_agent_hooks(), cx);
    }

    fn set_diff_font_size(&mut self, size: f32, cx: &mut Context<Self>) {
        let size = size.clamp(MIN_DIFF_FONT_SIZE, MAX_DIFF_FONT_SIZE);
        if self.settings.diff_font_size == size {
            return;
        }
        self.settings.diff_font_size = size;
        let (split, wrap) = (self.settings.diff_split, self.settings.diff_wrap);
        self.diff_view.update(cx, |diff_view, cx| {
            diff_view.set_preferences(split, wrap, size, cx)
        });
        self.sync_inbox_review_preferences(cx);
        self.persist_settings(cx);
    }

    pub(super) fn set_terminal_font_size(&mut self, size: f32, cx: &mut Context<Self>) {
        let size = size.clamp(8.0, 32.0);
        if self.settings.terminal_font_size == size {
            return;
        }
        self.settings.terminal_font_size = size;
        for terminal in self.terminals.values() {
            terminal.update(cx, |terminal, cx| terminal.apply_font_size(size, cx));
        }
        self.persist_settings(cx);
    }

    fn appearance_mode(&self) -> AppearanceMode {
        self.settings.appearance_mode
    }

    pub(super) fn apply_theme_preference(&mut self, system_dark: bool, cx: &mut Context<Self>) {
        if let Some(id) = theme::available_theme_id(&self.settings.theme_id)
            && self.settings.theme_id != id
        {
            self.settings.theme_id = id;
            self.persist_settings(cx);
        }
        theme::apply_preference(&self.settings.theme_id, self.appearance_mode(), system_dark);
        for terminal in self.terminals.values() {
            terminal.update(cx, |_, cx| cx.notify());
        }
        cx.notify();
    }

    pub(super) fn ensure_appearance_subscription(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self._appearance_subscription.is_some() {
            return;
        }
        let system_dark = ThemeTone::from_window_appearance(window.appearance()) == ThemeTone::Dark;
        self.apply_theme_preference(system_dark, cx);
        self._appearance_subscription =
            Some(cx.observe_window_appearance(window, |this, window, cx| {
                let system_dark =
                    ThemeTone::from_window_appearance(window.appearance()) == ThemeTone::Dark;
                this.apply_theme_preference(system_dark, cx);
            }));
    }

    fn set_theme_id(&mut self, theme_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let theme_id = theme::canonicalize_theme_id(theme_id);
        if self.settings.theme_id == theme_id {
            return;
        }
        self.settings.theme_id = theme_id;
        let system_dark = ThemeTone::from_window_appearance(window.appearance()) == ThemeTone::Dark;
        self.apply_theme_preference(system_dark, cx);
        self.persist_settings(cx);
    }

    fn set_appearance_mode(
        &mut self,
        mode: AppearanceMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.appearance_mode() == mode {
            return;
        }
        self.settings.appearance_mode = mode;
        let system_dark = ThemeTone::from_window_appearance(window.appearance()) == ThemeTone::Dark;
        self.apply_theme_preference(system_dark, cx);
        self.persist_settings(cx);
    }
}
