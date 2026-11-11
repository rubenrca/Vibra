//! Settings navigation, pages, controls, and application of workspace preferences.

mod appearance;

use gpui::{
    AnyElement, Context, Div, Hsla, SharedString, Stateful, Window, div, prelude::*, px, svg,
};

use crate::ui::tab_style::{TAB_RADIUS, track};
use crate::ui::theme::{
    self, AppearanceMode, MONO_FONT, ThemeTone, colors, content_on, frame_selected,
};

use super::chrome::SIDEBAR_ROW_INSET;
use super::navigation::{rail_row, section_label};
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
}

impl SettingsPage {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Appearance => "Appearance",
            Self::Agents => "Agents",
            Self::Security => "Privacy",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::General => "settings-general",
            Self::Appearance => "settings-appearance",
            Self::Agents => "settings-agents",
            Self::Security => "settings-privacy",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::General => "Choose how Vibra opens and keeps you up to date.",
            Self::Appearance => "Make Vibra your own, from colors to terminal text.",
            Self::Agents => "Connect the assistants working in your terminals.",
            Self::Security => "Control how terminal integrations access your Mac.",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::General => "chrome-icons/settings.svg",
            Self::Appearance => "chrome-icons/palette.svg",
            Self::Agents => "chrome-icons/sparkles.svg",
            Self::Security => "chrome-icons/shield.svg",
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

/// The foreground at `alpha`, for fills and strokes over whatever is below.
fn foreground(alpha: f32) -> Hsla {
    Hsla::from(colors().foreground).opacity(alpha)
}

/// A group of settings: an outlined card a step off the page. Its rows are
/// divided by hairlines.
fn settings_card() -> Div {
    div()
        .rounded(px(10.0))
        .border_1()
        .border_color(colors().border_subtle)
        .bg(foreground(0.025))
}

/// A heading and the card or controls under it.
fn settings_section(heading: AnyElement, body: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .child(heading)
        .child(body)
}

fn settings_row_label(label: &'static str, description: &'static str) -> Div {
    div()
        .flex_1()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .gap(px(3.0))
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
                .line_height(px(18.0))
                .text_color(colors().subtle)
                .child(description),
        )
}

/// Secondary prose inside a card.
fn settings_note(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(12.0))
        .line_height(px(18.0))
        .text_color(colors().subtle)
        .child(text.into())
}

fn settings_button_base(label: &'static str, id: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(30.0))
        .flex_none()
        .whitespace_nowrap()
        .px(px(12.0))
        .rounded(px(TAB_RADIUS))
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(12.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .child(label)
}

/// An on/off switch; its knob sits at the end that is on.
fn switch(enabled: bool) -> Div {
    div()
        .w(px(34.0))
        .h(px(20.0))
        .flex_none()
        .p(px(2.0))
        .rounded_full()
        .flex()
        .items_center()
        .when(enabled, |track| track.justify_end())
        .bg(if enabled {
            colors().accent.into()
        } else {
            foreground(0.18)
        })
        .child(
            div()
                .size(px(16.0))
                .flex_none()
                .rounded_full()
                .bg(gpui::rgb(0xffffff))
                .shadow_sm(),
        )
}

/// One step of a stepper, inside its track.
fn step_button(id: &'static str, icon: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(26.0))
        .flex_none()
        .rounded(px(TAB_RADIUS))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|button| button.bg(foreground(0.08)))
        .child(svg().path(icon).size(px(12.0)).text_color(colors().muted))
}

impl WorkspaceView {
    fn settings_section_heading(
        &self,
        title: &'static str,
        description: &'static str,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(3.0))
            .child(
                div()
                    .text_size(px(13.5))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(colors().foreground)
                    .child(title),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .text_color(colors().subtle)
                    .child(description),
            )
            .into_any_element()
    }

    /// A state in a tinted pill: green with a dot when it is on.
    fn settings_status_chip(&self, label: &'static str, active: bool) -> AnyElement {
        let color = if active {
            colors().success
        } else {
            colors().muted
        };
        div()
            .flex_none()
            .whitespace_nowrap()
            .h(px(22.0))
            .px(px(8.0))
            .rounded_full()
            .flex()
            .items_center()
            .gap(px(6.0))
            .bg(if active {
                gpui::Rgba {
                    a: 0.14,
                    ..colors().success
                }
                .into()
            } else {
                foreground(0.06)
            })
            .text_size(px(11.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(color)
            .child(div().size(px(6.0)).flex_none().rounded_full().bg(color))
            .child(label)
            .into_any_element()
    }

    /// A titled block inside a card: the title and its state, then prose.
    fn settings_info_row(
        &self,
        title: &'static str,
        status: AnyElement,
        description: &'static str,
    ) -> Div {
        div()
            .px(px(16.0))
            .py(px(14.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(13.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(colors().foreground)
                            .child(title),
                    )
                    .child(status),
            )
            .child(settings_note(description))
    }

    fn settings_hook_status_row(
        &self,
        label: &'static str,
        installed: bool,
        divider: bool,
    ) -> AnyElement {
        div()
            .h(px(40.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .when(divider, |row| {
                row.border_t_1().border_color(colors().border_subtle)
            })
            .child(
                div()
                    .flex_1()
                    .text_size(px(12.5))
                    .text_color(colors().foreground)
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
            .border_1()
            .border_color(colors().border_subtle)
            .text_color(colors().muted)
            .hover(|button| button.bg(foreground(0.06)).text_color(colors().foreground))
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
            .text_color(content_on(colors().accent))
            .hover(|button| button.opacity(0.9))
            .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
    }

    fn settings_toggle_row(
        &self,
        row: SettingsToggleRow,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        div()
            .id(row.id)
            .min_h(px(64.0))
            .px(px(16.0))
            .py(px(12.0))
            .flex()
            .items_center()
            .gap(px(16.0))
            .cursor_pointer()
            .when(row.divider, |row| {
                row.border_b_1().border_color(colors().border_subtle)
            })
            .hover(|row| row.bg(foreground(0.025)))
            .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
            .child(settings_row_label(row.label, row.description))
            .child(switch(row.enabled))
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

    /// The pages, on the frame like the global navigation and with its rows:
    /// the way back on top, then each group under its label.
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
        ];
        let mut navigation = div()
            .id("settings-navigation")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .px(px(SIDEBAR_ROW_INSET))
            .pb(px(10.0))
            .flex()
            .flex_col()
            .gap(px(3.0));
        for (label, pages) in groups {
            navigation = navigation.child(section_label(label));
            for &page in pages.iter() {
                let selected = self.settings_page == page;
                navigation = navigation.child(
                    rail_row(page.id())
                        .when(selected, |row| {
                            row.bg(frame_selected())
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(colors().foreground)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.settings_page = page;
                            if page == SettingsPage::Appearance {
                                theme::refresh_user_themes();
                            }
                            cx.notify();
                        }))
                        .child(
                            svg()
                                .path(page.icon())
                                .size(px(15.0))
                                .flex_none()
                                .text_color(if selected {
                                    colors().foreground
                                } else {
                                    colors().subtle
                                }),
                        )
                        .child(div().flex_1().min_w(px(0.0)).truncate().child(page.label())),
                );
            }
        }
        div()
            .w(px(self.left_sidebar_width()))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(SIDEBAR_ROW_INSET))
                    .pt(px(10.0))
                    .pb(px(8.0))
                    .border_b_1()
                    .border_color(colors().border_subtle)
                    .child(
                        rail_row("settings-back")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.leave_settings(window, cx)),
                            )
                            .child(
                                svg()
                                    .path("chrome-icons/chevron-left.svg")
                                    .size(px(15.0))
                                    .flex_none()
                                    .text_color(colors().subtle),
                            )
                            .child("Back"),
                    ),
            )
            .child(navigation)
            .into_any_element()
    }

    pub(super) fn settings_content(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let page = self.settings_page;
        let panel = div()
            .id(page.id())
            .w_full()
            .max_w(px(820.0))
            .min_w(px(0.0))
            .flex_none()
            .px(px(40.0))
            .pt(px(32.0))
            .pb(px(48.0))
            .flex()
            .flex_col()
            .gap(px(28.0))
            // The page's icon, name, and purpose, then a note that changes
            // apply as they are made.
            .child(
                div()
                    .pb(px(22.0))
                    .border_b_1()
                    .border_color(colors().border_subtle)
                    .flex()
                    .items_start()
                    .gap(px(14.0))
                    .child(
                        div()
                            .size(px(40.0))
                            .flex_none()
                            .rounded(px(10.0))
                            .border_1()
                            .border_color(foreground(0.07))
                            .bg(foreground(0.04))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                svg()
                                    .path(page.icon())
                                    .size(px(18.0))
                                    .text_color(colors().muted),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .text_size(px(20.0))
                                    .line_height(px(26.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(page.label()),
                            )
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .line_height(px(19.0))
                                    .text_color(colors().muted)
                                    .child(page.description()),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .h(px(26.0))
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .text_size(px(11.5))
                            .text_color(colors().subtle)
                            .child(
                                svg()
                                    .path("chrome-icons/check.svg")
                                    .size(px(12.0))
                                    .text_color(colors().subtle),
                            )
                            .child("Saved automatically"),
                    ),
            );
        let panel = match page {
            SettingsPage::Appearance => self.appearance_settings(panel, window, cx),
            SettingsPage::General => self.general_settings(panel, cx),
            SettingsPage::Agents => self.agent_settings(panel, cx),
            SettingsPage::Security => self.security_settings(panel),
        };
        div()
            .id(SharedString::from(format!("{}-scroll", page.id())))
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .child(panel)
            .into_any_element()
    }

    /// A size with a stepper in a recessed track, and Reset beside it while
    /// the size is not the default.
    fn settings_font_row(
        &self,
        row: FontSizeRow,
        divider: bool,
        cx: &mut Context<Self>,
        set: impl Fn(&mut Self, f32, &mut Context<Self>) + Copy + 'static,
    ) -> Div {
        let size = row.size;
        let default = row.default;
        let [down, reset, up] = row.ids;
        div()
            .px(px(16.0))
            .py(px(14.0))
            .flex()
            .items_center()
            .gap(px(16.0))
            .when(divider, |row| {
                row.border_t_1().border_color(colors().border_subtle)
            })
            .child(settings_row_label(row.label, row.description))
            .child(
                div()
                    .id(reset)
                    .h(px(30.0))
                    .px(px(10.0))
                    .flex_none()
                    .rounded(px(TAB_RADIUS))
                    .flex()
                    .items_center()
                    .text_size(px(12.0))
                    .text_color(colors().muted)
                    .when(size == default, |button| button.opacity(0.0))
                    .when(size != default, |button| {
                        button
                            .cursor_pointer()
                            .hover(|button| {
                                button.bg(foreground(0.06)).text_color(colors().foreground)
                            })
                            .on_click(cx.listener(move |this, _, _, cx| set(this, default, cx)))
                    })
                    .child("Reset"),
            )
            .child(
                track(colors().background)
                    .child(
                        step_button(down, "chrome-icons/minus.svg").on_click(cx.listener(
                            move |this, _, _, cx| {
                                set(this, size - 1.0, cx);
                            },
                        )),
                    )
                    .child(
                        div()
                            .w(px(50.0))
                            .h(px(26.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .font_family(MONO_FONT)
                            .text_size(px(12.0))
                            .text_color(colors().foreground)
                            .child(format!("{size:.0} px")),
                    )
                    .child(
                        step_button(up, "chrome-icons/plus.svg").on_click(cx.listener(
                            move |this, _, _, cx| {
                                set(this, size + 1.0, cx);
                            },
                        )),
                    ),
            )
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
                description: "Show spaces, agents, and global shortcuts when the app opens.",
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
                startup_card.child(self.settings_toggle_row(row, cx, move |this, _, cx| {
                    this.toggle_general_setting(id, cx);
                }));
        }
        panel
            .child(settings_section(
                self.settings_section_heading(
                    "Alerts",
                    "Choose how Vibra gets your attention while you work elsewhere.",
                ),
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
                        |this, _, cx| {
                            this.toggle_general_setting("settings-agent-notifications", cx)
                        },
                    )),
            ))
            .child(settings_section(
                self.settings_section_heading(
                    "On startup",
                    "Choose which elements appear when Vibra opens.",
                ),
                startup_card,
            ))
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
        let hooks = self
            .settings_info_row(
                "Hooks for task titles",
                self.settings_status_chip(agent_hooks_status, all_agent_hooks_installed),
                concat!(
                    "Optional: hooks name tasks after your prompts, show the model, ",
                    "and report status for Claude and Codex releases that do not ",
                    "report it to the terminal. Other agents are detected through ",
                    "their process, title, and screen output."
                ),
            )
            .border_t_1()
            .border_color(colors().border_subtle)
            .gap(px(10.0))
            .child(
                div()
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(colors().border_subtle)
                    .bg(foreground(0.02))
                    .overflow_hidden()
                    .child(self.settings_hook_status_row(
                        "Claude",
                        agent_hooks.claude_installed,
                        false,
                    ))
                    .child(self.settings_hook_status_row(
                        "Codex",
                        agent_hooks.codex_installed,
                        true,
                    )),
            )
            .child(settings_note(
                "In Codex, approve the configuration once using /hooks.",
            ))
            .when_some(self.agent_hook_error.clone(), |card, error| {
                card.child(settings_note(error).text_color(colors().danger))
            })
            .child(
                div()
                    .pt(px(2.0))
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
            );
        panel.child(settings_section(
            self.settings_section_heading(
                "Agent activity",
                "Track the status of assistants running inside Vibra.",
            ),
            settings_card()
                .overflow_hidden()
                .child(self.settings_info_row(
                    "Automatic detection",
                    self.settings_status_chip("Always on", true),
                    concat!(
                        "Vibra recognizes agents running in its terminals ",
                        "and shows their activity in panes and tabs. Claude Code, ",
                        "Codex, Grok, and pi report their own status to the terminal, ",
                        "so they need no setup."
                    ),
                ))
                .child(hooks),
        ))
    }

    fn security_settings(&self, panel: Stateful<Div>) -> Stateful<Div> {
        panel.child(settings_section(
            self.settings_section_heading(
                "Terminal privacy",
                "Protections for local terminal integrations.",
            ),
            settings_card().child(self.settings_info_row(
                "Clipboard access (OSC 52)",
                self.settings_status_chip("Confirmation required", true),
                concat!(
                    "Each read requires confirmation. Local communication uses ",
                    "a private socket and a separate token for each pane."
                ),
            )),
        ))
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

    fn set_auto_hide_tabs(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.settings.auto_hide_tabs == enabled {
            return;
        }
        self.settings.auto_hide_tabs = enabled;
        self.tabs_revealed = false;
        cx.notify();
        self.persist_settings(cx);
    }

    fn set_window_transparency(
        &mut self,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings.window_transparency == enabled {
            return;
        }
        self.settings.window_transparency = enabled;
        theme::set_translucent(enabled);
        window.set_background_appearance(theme::window_background());
        for terminal in self.terminals.values() {
            terminal.update(cx, |_, cx| cx.notify());
        }
        cx.notify();
        self.persist_settings(cx);
    }
}
