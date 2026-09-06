//! Settings navigation, pages, controls, and application of workspace preferences.

use gpui::{
    AnyElement, Context, Div, MouseButton, SharedString, Stateful, Window, div, prelude::*, px,
};

use crate::ui::theme::{self, AppearanceMode, ThemeTone, colors};

use super::WorkspaceView;
use crate::infrastructure::automation::{
    AgentHookStatus, install_agent_hooks, uninstall_agent_hooks,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SettingsPage {
    General,
    Appearance,
    Iphone,
    Agents,
    Security,
}

impl SettingsPage {
    fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Appearance => "Apariencia",
            Self::Iphone => "iPhone",
            Self::Agents => "Agentes",
            Self::Security => "Privacidad",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::General => "settings-general",
            Self::Appearance => "settings-appearance",
            Self::Iphone => "settings-iphone",
            Self::Agents => "settings-agents",
            Self::Security => "settings-privacy",
        }
    }
}

struct SettingsToggleRow {
    label: &'static str,
    description: &'static str,
    enabled: bool,
    divider: bool,
    id: &'static str,
}

fn settings_button_base(label: &'static str, id: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(26.0))
        .px_3()
        .rounded(px(5.0))
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(9.0))
        .child(label)
}

impl WorkspaceView {
    fn remote_result(&mut self, result: anyhow::Result<()>, cx: &mut Context<Self>) {
        match result {
            Err(error) => self.remote_feedback = Some(error.to_string()),
            Ok(()) => {
                self.remote_feedback = None;
                self.persistence_error = None;
            }
        }
        cx.notify();
    }

    fn remote_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        use crate::infrastructure::remote::hub;
        let status = hub().status();
        let mut panel = div()
            .p_4()
            .rounded(px(9.0))
            .border_1()
            .border_color(colors().border_subtle)
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_sm().child("iPhone"))
            .child(
                div()
                    .text_xs()
                    .text_color(colors().subtle)
                    .child("Controla tus terminales desde el iPhone en la misma red Wi-Fi. Sin servidor ni cuenta adicional."),
            )
            .child(div().text_xs().child(if status.paired && !status.enabled {
                "iPhone vinculado · acceso desactivado"
            } else if status.pending.is_some() {
                "Un iPhone espera tu aprobación"
            } else if status.paired {
                "iPhone vinculado"
            } else if status.invitation.is_some() {
                "Escanea el código con Vibra en tu iPhone"
            } else {
                "Vincula tu iPhone para comenzar"
            }));
        if let Some(message) = &self.remote_feedback {
            panel = panel.child(
                div()
                    .p_3()
                    .rounded(px(6.0))
                    .bg(colors().selection)
                    .text_xs()
                    .child(message.clone()),
            );
        }
        if status.paired {
            panel = panel.child(div().text_xs().text_color(colors().subtle)
                .child("Haz clic derecho dentro de una terminal y elige Compartir con iPhone. Puedes recuperar el control desde el Mac en cualquier momento."));
        } else if status.invitation.is_none() && status.pending.is_none() {
            panel = panel
                .child(
                    div()
                        .text_xs()
                        .text_color(colors().subtle)
                        .child("Conecta una vez. Después, elige qué terminales quieres compartir."),
                )
                .child(div().text_xs().child("1 · Crea tu código de vinculación"))
                .child(
                    div()
                        .text_xs()
                        .text_color(colors().subtle)
                        .child("2 · Escanéalo con Vibra en tu iPhone"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(colors().subtle)
                        .child("3 · Acepta el iPhone en este Mac"),
                )
                .child(
                    self.settings_primary_button(
                        "Vincular iPhone",
                        "remote-pair",
                        cx,
                        |this, cx| {
                            this.remote_copied = false;
                            this.remote_result(hub().pair(), cx);
                        },
                    )
                    .h(px(36.0))
                    .border_1()
                    .border_color(colors().accent),
                );
        }
        if let Some(name) = status.pending {
            panel = panel
                .child(format!(
                    "¿Permitir que {name} controle las terminales compartidas?"
                ))
                .child(self.settings_primary_button(
                    "Aceptar y vincular",
                    "remote-approve",
                    cx,
                    |_, cx| {
                        hub().approve(true);
                        cx.notify();
                    },
                ))
                .child(
                    self.settings_button("Rechazar", "remote-reject", cx, |_, cx| {
                        hub().approve(false);
                        cx.notify();
                    }),
                );
        }
        if let Some(invitation) = status.invitation {
            panel = panel
                .child(
                    div()
                        .text_xs()
                        .child("Abre Vibra en tu iPhone y toca Escanear código QR. Después, acepta la conexión aquí. El código dura 5 minutos."),
                )
                .child(self.settings_button(
                    if self.remote_copied { "Invitación copiada" } else { "Copiar invitación" },
                    "remote-copy",
                    cx,
                    |this, cx| {
                        if let Some(value) = hub().status().invitation {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(value));
                            this.remote_copied = true;
                            cx.notify();
                        }
                    },
                ));
            if let Ok(qr) = qrcode::QrCode::new(invitation.as_bytes()) {
                let width = qr.width();
                let modules = qr.to_colors();
                let mut grid = div()
                    .flex()
                    .flex_col()
                    .p_4()
                    .bg(gpui::rgb(0xffffff))
                    .w(px((width * 3 + 32) as f32));
                for row in modules.chunks(width) {
                    grid = grid.child(div().flex().children(row.iter().map(|color| {
                        div().w(px(3.0)).h(px(3.0)).flex_none().bg(gpui::rgb(
                            if *color == qrcode::Color::Dark {
                                0x000000
                            } else {
                                0xffffff
                            },
                        ))
                    })));
                }
                panel = panel.child(grid);
            }
        }
        if status.paired || status.enabled {
            panel = panel.child(self.settings_toggle_row(
                SettingsToggleRow {
                    label: "Permitir acceso desde el iPhone",
                    description: "Al desactivarlo, el Mac recupera el control. La vinculación se conserva.",
                    enabled: status.enabled,
                    divider: false,
                    id: "remote-enabled-toggle",
                }, cx, |this, cx| {
                    if hub().status().enabled {
                        hub().disable();
                        cx.notify();
                    } else {
                        this.remote_result(hub().enable(), cx);
                    }
                }));
        }
        if status.paired {
            if self.remote_forget_confirm {
                panel = panel
                    .child(div().text_xs().child("El iPhone perderá el acceso. Para volver, tendrás que escanear otro código."))
                    .child(self.settings_button(
                        "Confirmar desvinculación",
                        "remote-revoke-confirm",
                        cx,
                        |this, cx| {
                            this.remote_forget_confirm = false;
                            this.remote_result(hub().revoke(), cx);
                        },
                    ))
                    .child(self.settings_button(
                        "Cancelar",
                        "remote-revoke-cancel",
                        cx,
                        |this, cx| {
                            this.remote_forget_confirm = false;
                            cx.notify();
                        },
                    ));
            } else {
                panel = panel.child(self.settings_button(
                    "Desvincular iPhone",
                    "remote-revoke",
                    cx,
                    |this, cx| {
                        this.remote_forget_confirm = true;
                        cx.notify();
                    },
                ));
            }
        }
        panel = panel.child(
            div()
                .text_xs()
                .text_color(colors().subtle)
                .child(status.description),
        );
        panel.into_any_element()
    }

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
                    .text_size(px(11.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(colors().foreground)
                    .child(title),
            )
            .child(
                div()
                    .text_size(px(9.0))
                    .text_color(colors().subtle)
                    .child(description),
            )
            .into_any_element()
    }

    fn settings_status_chip(&self, label: &'static str, active: bool) -> AnyElement {
        div()
            .px_2()
            .py_1()
            .rounded(px(4.0))
            .bg(if active {
                colors().diff_added_bg
            } else {
                colors().selection
            })
            .text_size(px(8.0))
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
                    .text_size(px(10.0))
                    .text_color(colors().muted)
                    .child(label),
            )
            .child(self.settings_status_chip(
                if installed {
                    "Instalado"
                } else {
                    "No instalado"
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
            .h(px(24.0))
            .bg(colors().selection)
            .border_1()
            .border_color(colors().border_subtle)
            .text_color(colors().muted)
            .hover(|button| button.bg(colors().hover).text_color(colors().foreground))
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

    fn settings_mode_button(
        &self,
        label: &'static str,
        id: &'static str,
        selected: bool,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        settings_button_base(label, id)
            .bg(if selected {
                colors().selection
            } else {
                colors().elevated
            })
            .border_1()
            .border_color(if selected {
                colors().accent
            } else {
                colors().border_subtle
            })
            .text_color(if selected {
                colors().foreground
            } else {
                colors().muted
            })
            .hover(|button| button.bg(colors().hover).text_color(colors().foreground))
            .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
    }

    fn settings_theme_grid(
        &self,
        active_theme_id: &str,
        preview_tone: ThemeTone,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut grid = div().flex().flex_col().gap_2();
        for families in theme::built_in_themes().chunks(2) {
            let mut row = div().flex().gap_2();
            for family in families {
                let selected = family.id == active_theme_id;
                let theme_id = family.id;
                let [sidebar, panel, accent] = family.preview(preview_tone);
                let card = div()
                    .id(SharedString::from(format!("settings-theme-{theme_id}")))
                    .flex_1()
                    .min_w(px(0.0))
                    .px_2()
                    .py_1()
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .border_1()
                    .border_color(if selected {
                        colors().accent
                    } else {
                        colors().border_subtle
                    })
                    .bg(if selected {
                        colors().selection
                    } else {
                        colors().elevated
                    })
                    .hover(|card| card.bg(colors().hover))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_theme_id(theme_id, window, cx);
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(div().size(px(9.0)).rounded_full().bg(sidebar))
                                    .child(div().size(px(9.0)).rounded_full().bg(panel))
                                    .child(div().size(px(9.0)).rounded_full().bg(accent)),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(colors().foreground)
                                    .child(family.label),
                            ),
                    );
                row = row.child(card);
            }
            if families.len() == 1 {
                row = row.child(div().flex_1());
            }
            grid = grid.child(row);
        }
        grid.into_any_element()
    }

    fn settings_toggle_row(
        &self,
        row: SettingsToggleRow,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        div()
            .id(row.id)
            .min_h(px(52.0))
            .px_3()
            .py_2()
            .flex()
            .items_center()
            .cursor_pointer()
            .when(row.divider, |row| {
                row.border_b_1().border_color(colors().border_subtle)
            })
            .hover(|row| row.bg(colors().hover))
            .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(10.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(colors().foreground)
                            .child(row.label),
                    )
                    .child(
                        div()
                            .text_size(px(8.5))
                            .text_color(colors().subtle)
                            .child(row.description),
                    ),
            )
            .child(
                div()
                    .w(px(30.0))
                    .h(px(16.0))
                    .p(px(2.0))
                    .rounded_full()
                    .flex()
                    .justify_end()
                    .bg(if row.enabled {
                        colors().success
                    } else {
                        colors().selection
                    })
                    .when(!row.enabled, |toggle| toggle.justify_start())
                    .child(div().size(px(12.0)).rounded_full().bg(colors().foreground)),
            )
    }

    pub(super) fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.settings_open = true;
        self.palette_mode = None;
        self.palette_files.clear();
        self.context_menu = None;
        self.ide_menu_open = false;
        self.rename_prompt = None;
        cx.notify();
    }

    pub(super) fn close_settings(&mut self, cx: &mut Context<Self>) {
        if self.settings_open {
            self.settings_open = false;
            cx.notify();
        }
    }

    pub(super) fn settings_modal(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.settings_open {
            return None;
        }
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(colors().overlay())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.close_settings(cx);
                    }),
                )
                .child(
                    div()
                        .id("settings-modal")
                        .w(px(780.0))
                        .h(px(660.0))
                        .max_w_full()
                        .max_h_full()
                        .mx_4()
                        .rounded(px(12.0))
                        .border_1()
                        .border_color(colors().border_subtle)
                        .bg(colors().panel)
                        .shadow_lg()
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .child(
                            div()
                                .h(px(56.0))
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap_3()
                                .px_5()
                                .border_b_1()
                                .border_color(colors().border_subtle)
                                .child(
                                    div()
                                        .flex_1()
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_size(px(13.0))
                                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                                .text_color(colors().foreground)
                                                .child("Ajustes"),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(9.0))
                                                .text_color(colors().subtle)
                                                .child("Configura tu espacio de trabajo"),
                                        ),
                                )
                                .child(
                                    div()
                                        .px_2()
                                        .py_1()
                                        .rounded(px(4.0))
                                        .border_1()
                                        .border_color(colors().border_subtle)
                                        .text_size(px(8.0))
                                        .text_color(colors().subtle)
                                        .child("ESC"),
                                )
                                .child(self.sidebar_close_button(
                                    "close-settings-modal",
                                    cx,
                                    |this, cx| {
                                        this.close_settings(cx);
                                    },
                                )),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_1()
                                .min_h(px(0.0))
                                .child(self.settings_navigation(cx))
                                .child(self.settings_modal_content(window, cx)),
                        )
                        .child(
                            div()
                                .flex_none()
                                .px_5()
                                .py_2()
                                .border_t_1()
                                .border_color(colors().border_subtle)
                                .text_xs()
                                .text_color(colors().subtle)
                                .child("Los cambios se guardan automáticamente."),
                        ),
                )
                .into_any_element(),
        )
    }

    fn settings_navigation(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut navigation = div()
            .id("settings-navigation")
            .w(px(150.0))
            .flex_none()
            .p_3()
            .flex()
            .flex_col()
            .gap_1()
            .border_r_1()
            .border_color(colors().border_subtle);
        for page in [
            SettingsPage::General,
            SettingsPage::Appearance,
            SettingsPage::Iphone,
            SettingsPage::Agents,
            SettingsPage::Security,
        ] {
            let selected = self.settings_page == page;
            navigation = navigation.child(
                div()
                    .id(page.id())
                    .px_3()
                    .py_2()
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .text_sm()
                    .bg(if selected {
                        colors().selection
                    } else {
                        colors().panel
                    })
                    .text_color(if selected {
                        colors().foreground
                    } else {
                        colors().muted
                    })
                    .hover(|style| style.bg(colors().selection))
                    .child(page.label())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings_page = page;
                        this.remote_forget_confirm = false;
                        cx.notify();
                    })),
            );
        }
        navigation.into_any_element()
    }

    fn settings_modal_content(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let panel = div()
            .id(self.settings_page.id())
            .min_w(px(0.0))
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .p_5()
            .flex()
            .flex_col()
            .gap_4();
        let panel = match self.settings_page {
            SettingsPage::Iphone => panel.child(self.remote_settings(cx)),
            SettingsPage::Appearance => self.appearance_settings(panel, window, cx),
            SettingsPage::General => self.general_settings(panel, cx),
            SettingsPage::Agents => self.agent_settings(panel, cx),
            SettingsPage::Security => self.security_settings(panel),
        };
        panel.child(div().h(px(1.0))).into_any_element()
    }

    fn appearance_settings(
        &self,
        panel: Stateful<Div>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let font_size = self.settings.terminal_font_size;
        let appearance = self.appearance_mode();
        let system_dark = ThemeTone::from_window_appearance(window.appearance()) == ThemeTone::Dark;
        let preview_tone = theme::resolve_tone(appearance, system_dark);
        panel
            .child(self.settings_section_heading(
                "Apariencia",
                "Elige cómo se ve Vibra y ajusta la lectura de la terminal.",
            ))
            .child(
                div()
                    .p_4()
                    .rounded(px(9.0))
                    .border_1()
                    .border_color(colors().border_subtle)
                    .bg(colors().elevated)
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_size(px(10.5))
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .text_color(colors().foreground)
                                            .child("Modo de apariencia"),
                                    )
                                    .child(
                                        div().text_size(px(9.0)).text_color(colors().subtle).child(
                                            "Sigue macOS o fija un modo para la aplicación.",
                                        ),
                                    ),
                            )
                            .child(
                                div().flex().items_center().gap_1().children(
                                    [
                                        (
                                            AppearanceMode::System,
                                            "Sistema",
                                            "settings-appearance-system",
                                        ),
                                        (
                                            AppearanceMode::Light,
                                            "Claro",
                                            "settings-appearance-light",
                                        ),
                                        (
                                            AppearanceMode::Dark,
                                            "Oscuro",
                                            "settings-appearance-dark",
                                        ),
                                    ]
                                    .into_iter()
                                    .map(
                                        |(mode, label, id)| {
                                            self.settings_mode_button(
                                                label,
                                                id,
                                                appearance == mode,
                                                cx,
                                                move |this, window, cx| {
                                                    this.set_appearance_mode(mode, window, cx);
                                                },
                                            )
                                        },
                                    ),
                                ),
                            ),
                    )
                    .child(div().h(px(1.0)).bg(colors().border_subtle))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(colors().foreground)
                                    .child("Tema"),
                            )
                            .child(div().text_size(px(9.0)).text_color(colors().subtle).child(
                                "La paleta también se aplica al terminal y al resaltado de código.",
                            )),
                    )
                    .child(
                        div()
                            .id("settings-theme-list")
                            .max_h(px(220.0))
                            .pr_1()
                            .overflow_y_scroll()
                            // Keep the outer settings pane still while scrolling themes.
                            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                            .child(self.settings_theme_grid(
                                &self.settings.theme_id,
                                preview_tone,
                                cx,
                            )),
                    ),
            )
            .child(
                div()
                    .p_4()
                    .rounded(px(9.0))
                    .border_1()
                    .border_color(colors().border_subtle)
                    .bg(colors().elevated)
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_size(px(10.5))
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .text_color(colors().foreground)
                                            .child("Tamaño del texto"),
                                    )
                                    .child(
                                        div().text_size(px(9.0)).text_color(colors().subtle).child(
                                            "Fuente JetBrains Mono en todas las terminales.",
                                        ),
                                    ),
                            )
                            .child(
                                div()
                                    .w(px(52.0))
                                    .h(px(26.0))
                                    .rounded(px(5.0))
                                    .bg(colors().panel)
                                    .border_1()
                                    .border_color(colors().border_subtle)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .font_family("JetBrains Mono")
                                    .text_size(px(9.0))
                                    .text_color(colors().foreground)
                                    .child(format!("{font_size:.0} px")),
                            )
                            .child(self.settings_button(
                                "−",
                                "settings-font-down",
                                cx,
                                |this, cx| {
                                    this.set_terminal_font_size(
                                        this.settings.terminal_font_size - 1.0,
                                        cx,
                                    );
                                },
                            ))
                            .child(self.settings_button(
                                "Restablecer",
                                "settings-font-reset",
                                cx,
                                |this, cx| {
                                    this.set_terminal_font_size(12.0, cx);
                                },
                            ))
                            .child(self.settings_button(
                                "+",
                                "settings-font-up",
                                cx,
                                |this, cx| {
                                    this.set_terminal_font_size(
                                        this.settings.terminal_font_size + 1.0,
                                        cx,
                                    );
                                },
                            )),
                    ),
            )
    }

    fn general_settings(&self, panel: Stateful<Div>, cx: &mut Context<Self>) -> Stateful<Div> {
        panel
            .child(self.settings_section_heading(
                "Al abrir Vibra",
                "Configura qué elementos estarán disponibles al abrir Vibra.",
            ))
            .child(
                div()
                    .rounded(px(9.0))
                    .overflow_hidden()
                    .border_1()
                    .border_color(colors().border_subtle)
                    .bg(colors().elevated)
                    .child(self.settings_toggle_row(
                        SettingsToggleRow {
                            label: "Archivos ocultos",
                            description: "Incluye archivos y carpetas que comienzan con punto.",
                            enabled: self.settings.show_hidden_files,
                            divider: true,
                            id: "settings-hidden",
                        },
                        cx,
                        |this, cx| {
                            this.settings.show_hidden_files = !this.settings.show_hidden_files;
                            this.refresh_project_files(cx);
                            this.persist_settings(cx);
                        },
                    ))
                    .child(self.settings_toggle_row(
                        SettingsToggleRow {
                            label: "Lista de sesiones",
                            description: "Muestra la lista de sesiones al abrir la aplicación.",
                            enabled: self.settings.left_sidebar_visible,
                            divider: true,
                            id: "settings-sidebar-visible",
                        },
                        cx,
                        |this, cx| {
                            this.set_left_sidebar_visible(!this.left_sidebar_visible, true, cx);
                        },
                    ))
                    .child(self.settings_toggle_row(
                        SettingsToggleRow {
                            label: "Panel de archivos y Git",
                            description: "Abre el panel derecho del proyecto al iniciar.",
                            enabled: self.settings.git_panel_visible,
                            divider: false,
                            id: "settings-git-visible",
                        },
                        cx,
                        |this, cx| {
                            let open = !this.right_sidebar_visible;
                            this.set_right_sidebar_visible(open, true, cx);
                            if open {
                                this.sync_diff_root(cx);
                            }
                        },
                    )),
            )
    }

    fn agent_settings(&self, panel: Stateful<Div>, cx: &mut Context<Self>) -> Stateful<Div> {
        let agent_hooks = self.agent_hook_status.unwrap_or_default();
        let all_agent_hooks_installed = agent_hooks.all_installed();
        let any_agent_hooks_installed = agent_hooks.any_installed();
        let agent_hooks_status = if all_agent_hooks_installed {
            "Configurados"
        } else if any_agent_hooks_installed {
            "Parciales"
        } else {
            "Opcionales"
        };
        panel
            .child(self.settings_section_heading(
                "Agentes y notificaciones",
                "Sigue el estado de los asistentes que ejecutas dentro de Vibra.",
            ))
            .child(
                div()
                    .p_4()
                    .rounded(px(9.0))
                    .border_1()
                    .border_color(colors().border_subtle)
                    .bg(colors().elevated)
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
                                    .text_size(px(10.5))
                                    .text_color(colors().foreground)
                                    .child("Detección automática"),
                            )
                            .child(self.settings_status_chip("Siempre activa", true)),
                    )
                    .child(
                        div()
                            .text_size(px(9.0))
                            .line_height(px(13.0))
                            .text_color(colors().subtle)
                            .child(
                                "Vibra reconoce los agentes que ejecutas en sus terminales y muestra su actividad en panes, tabs y sesiones.",
                            ),
                    )
                    .child(div().h(px(1.0)).bg(colors().border_subtle))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(10.5))
                                    .text_color(colors().foreground)
                                    .child("Hooks para estados precisos"),
                            )
                            .child(self.settings_status_chip(
                                agent_hooks_status,
                                all_agent_hooks_installed,
                            )),
                    )
                    .child(
                        div()
                            .text_size(px(9.0))
                            .line_height(px(13.0))
                            .text_color(colors().subtle)
                            .child(
                                "Opcional: instala hooks para que Claude y Codex informen cuándo trabajan, terminan o piden permiso. Los demás agentes se detectan por el proceso y la pantalla.",
                            ),
                    )
                    .child(self.settings_hook_status_row("Claude", agent_hooks.claude_installed))
                    .child(self.settings_hook_status_row("Codex", agent_hooks.codex_installed))
                    .child(
                        div()
                            .text_size(px(9.0))
                            .line_height(px(13.0))
                            .text_color(colors().subtle)
                            .child(
                                "En Codex tendrás que aprobar la configuración una vez con /hooks.",
                            ),
                    )
                    .when_some(self.agent_hook_error.clone(), |card, error| {
                        card.child(
                            div()
                                .text_size(px(9.0))
                                .line_height(px(13.0))
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
                                    "Actualizar hooks"
                                } else if any_agent_hooks_installed {
                                    "Instalar hooks faltantes"
                                } else {
                                    "Instalar hooks"
                                },
                                "settings-agent-hooks-install",
                                cx,
                                |this, cx| this.install_agent_hooks_from_settings(cx),
                            ))
                            .when(any_agent_hooks_installed, |buttons| {
                                buttons.child(self.settings_button(
                                    "Desactivar",
                                    "settings-agent-hooks-uninstall",
                                    cx,
                                    |this, cx| this.uninstall_agent_hooks_from_settings(cx),
                                ))
                            }),
                    ),
            )
            .child(
                div()
                    .rounded(px(9.0))
                    .overflow_hidden()
                    .border_1()
                    .border_color(colors().border_subtle)
                    .bg(colors().elevated)
                    .child(self.settings_toggle_row(
                        SettingsToggleRow {
                            label: "Notificaciones de actividad",
                            description: "Avisa si un agente termina o necesita atención fuera del pane actual.",
                            enabled: self.settings.agent_notifications,
                            divider: false,
                            id: "settings-agent-notifications",
                        },
                        cx,
                        |this, cx| {
                            this.settings.agent_notifications = !this.settings.agent_notifications;
                            if this.settings.agent_notifications {
                                crate::infrastructure::notifications::request_authorization();
                            }
                            this.persist_settings(cx);
                        },
                    )),
            )
    }

    fn security_settings(&self, panel: Stateful<Div>) -> Stateful<Div> {
        panel
            .child(self.settings_section_heading(
                "Privacidad de la terminal",
                "Protecciones aplicadas a las integraciones locales de la terminal.",
            ))
            .child(
                div()
                    .p_4()
                    .rounded(px(9.0))
                    .border_1()
                    .border_color(colors().border_subtle)
                    .bg(colors().elevated)
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
                                    .text_size(px(10.5))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(colors().foreground)
                                    .child("Lectura del portapapeles (OSC 52)"),
                            )
                            .child(self.settings_status_chip("Confirmación obligatoria", true)),
                    )
                    .child(
                        div()
                            .text_size(px(9.0))
                            .line_height(px(13.0))
                            .text_color(colors().subtle)
                            .child("Cada lectura requiere confirmación. La comunicación local usa un socket privado y un token distinto por pane."),
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
                self.agent_hook_status = None;
                self.agent_hook_error =
                    Some(format!("No se pudo actualizar las integraciones: {error}").into());
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

    pub(super) fn set_terminal_font_size(&mut self, size: f32, cx: &mut Context<Self>) {
        let size = size.clamp(8.0, 32.0);
        if self.settings.terminal_font_size == size {
            return;
        }
        self.settings.terminal_font_size = size;
        for terminal in self.terminals.values() {
            terminal.update(cx, |terminal, cx| terminal.apply_font_size(size, cx));
        }
        for drawer in self.dev_terminals.values() {
            for terminal in &drawer.terminals {
                terminal.update(cx, |terminal, cx| terminal.apply_font_size(size, cx));
            }
        }
        self.persist_settings(cx);
    }

    fn appearance_mode(&self) -> AppearanceMode {
        AppearanceMode::parse(&self.settings.appearance_mode)
    }

    pub(super) fn apply_theme_preference(&mut self, system_dark: bool, cx: &mut Context<Self>) {
        theme::apply_preference(&self.settings.theme_id, self.appearance_mode(), system_dark);
        for terminal in self.terminals.values() {
            terminal.update(cx, |_, cx| cx.notify());
        }
        for drawer in self.dev_terminals.values() {
            for terminal in &drawer.terminals {
                terminal.update(cx, |_, cx| cx.notify());
            }
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
        self.settings.theme_id = theme_id.to_string();
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
        self.settings.appearance_mode = mode.as_str().to_string();
        let system_dark = ThemeTone::from_window_appearance(window.appearance()) == ThemeTone::Dark;
        self.apply_theme_preference(system_dark, cx);
        self.persist_settings(cx);
    }
}
