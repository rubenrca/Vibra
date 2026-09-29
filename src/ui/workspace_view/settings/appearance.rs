//! Visual previews use the same palettes that are applied to the workspace.

use gpui::{
    AnyElement, Context, Div, SharedString, Stateful, Window, div, linear_color_stop,
    linear_gradient, prelude::*, px, relative, svg,
};

use super::{FontSizeRow, WorkspaceView, settings_card};
use crate::ui::theme::{self, AppearanceMode, Theme, ThemeFamily, ThemeTone, colors, surface_tint};

fn preview_line(width: f32, color: gpui::Rgba) -> Div {
    div()
        .w(relative(width))
        .h(px(5.0))
        .flex_none()
        .rounded_full()
        .bg(color)
}

/// A miniature sidebar, terminal and input, painted in the selected palette.
fn workspace_preview(palette: Theme) -> Div {
    div()
        .size_full()
        .flex()
        .overflow_hidden()
        .bg(palette.background)
        .child(
            div()
                .w(relative(0.24))
                .h_full()
                .flex_none()
                .p_2()
                .bg(palette.sidebar)
                .border_r_1()
                .border_color(palette.border_subtle)
                .flex()
                .flex_col()
                .gap(px(7.0))
                .child(
                    div()
                        .h(px(10.0))
                        .w_full()
                        .rounded(px(4.0))
                        .border_1()
                        .border_color(palette.border_subtle)
                        .mb_1(),
                )
                .children(
                    [palette.selection, palette.hover, palette.panel]
                        .map(|color| div().h(px(7.0)).w_full().rounded(px(3.0)).bg(color)),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .h_full()
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div().flex().justify_end().child(
                        div()
                            .w(relative(0.5))
                            .h(px(9.0))
                            .rounded_full()
                            .bg(palette.selection),
                    ),
                )
                .child(preview_line(0.8, palette.muted))
                .child(preview_line(0.58, palette.selection))
                .child(preview_line(0.7, palette.selection))
                .child(div().flex_1())
                .child(
                    div()
                        .h(px(21.0))
                        .flex_none()
                        .px_2()
                        .rounded(px(7.0))
                        .border_1()
                        .border_color(palette.border_subtle)
                        .bg(palette.panel)
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(preview_line(0.5, palette.selection))
                        .child(div().flex_1())
                        .child(div().size(px(9.0)).rounded_full().bg(palette.accent)),
                ),
        )
}

fn theme_orb(family: &ThemeFamily, tone: ThemeTone, selected: bool) -> Div {
    let [sidebar, panel, accent] = family.preview(tone);
    let palette = family.colors(tone);
    div()
        .size(px(70.0))
        .flex_none()
        .relative()
        // GPUI paints a parent's border after its children. Keep the ring and
        // badge as siblings so the badge is painted on top of the entire ring.
        .child(
            div()
                .size_full()
                .p(px(4.0))
                .rounded_full()
                .border_2()
                .border_color(if selected {
                    colors().accent
                } else {
                    gpui::rgba(0)
                })
                .child(
                    div()
                        .size_full()
                        .rounded_full()
                        .overflow_hidden()
                        .shadow_md()
                        .bg(linear_gradient(
                            135.0,
                            linear_color_stop(panel, 0.0),
                            linear_color_stop(accent, 1.0),
                        ))
                        .child(div().size_full().rounded_full().bg(linear_gradient(
                            35.0,
                            linear_color_stop(palette.foreground, 0.0).opacity(0.12),
                            linear_color_stop(sidebar, 1.0).opacity(0.8),
                        ))),
                ),
        )
        .child(
            div()
                .absolute()
                .bottom_0()
                .right_0()
                .size(px(21.0))
                .rounded_full()
                .bg(colors().elevated)
                .border_1()
                .border_color(colors().border_subtle)
                .flex()
                .items_center()
                .justify_center()
                .child(
                    svg()
                        .path(match tone {
                            ThemeTone::Light => "chrome-icons/sun.svg",
                            ThemeTone::Dark => "chrome-icons/moon.svg",
                        })
                        .size(px(12.0))
                        .text_color(colors().muted),
                ),
        )
}

impl WorkspaceView {
    fn appearance_mode_cards(&self, cx: &mut Context<Self>) -> Div {
        let appearance = self.appearance_mode();
        let light = theme::resolve(&self.settings.theme_id, AppearanceMode::Light, false);
        let dark = theme::resolve(&self.settings.theme_id, AppearanceMode::Dark, true);
        div().flex().gap_3().children(
            [
                (
                    AppearanceMode::System,
                    "System",
                    "settings-appearance-system",
                ),
                (AppearanceMode::Light, "Light", "settings-appearance-light"),
                (AppearanceMode::Dark, "Dark", "settings-appearance-dark"),
            ]
            .into_iter()
            .map(|(mode, label, id)| {
                let selected = appearance == mode;
                let preview = div()
                    .h(px(122.0))
                    .w_full()
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(colors().border_subtle)
                    .overflow_hidden()
                    .flex();
                let preview = match mode {
                    AppearanceMode::Light => preview.child(workspace_preview(light)),
                    AppearanceMode::Dark => preview.child(workspace_preview(dark)),
                    AppearanceMode::System => preview
                        .child(
                            div().w(relative(0.5)).h_full().overflow_hidden().child(
                                div()
                                    .w(relative(2.0))
                                    .h_full()
                                    .child(workspace_preview(light)),
                            ),
                        )
                        .child(
                            div().w(relative(0.5)).h_full().overflow_hidden().child(
                                div()
                                    .w(relative(2.0))
                                    .h_full()
                                    .relative()
                                    .left(relative(-1.0))
                                    .child(workspace_preview(dark)),
                            ),
                        ),
                };
                div()
                    .id(id)
                    .flex_1()
                    .min_w(px(0.0))
                    .p_2()
                    .rounded(px(13.0))
                    .border_1()
                    .border_color(if selected {
                        colors().accent
                    } else {
                        colors().border_subtle
                    })
                    .bg(surface_tint(colors().elevated, colors().background))
                    .cursor_pointer()
                    .hover(|card| card.border_color(colors().accent))
                    .child(preview)
                    .child(
                        div()
                            .h(px(30.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(13.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(if selected {
                                colors().foreground
                            } else {
                                colors().muted
                            })
                            .child(label),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_appearance_mode(mode, window, cx);
                    }))
            }),
        )
    }

    fn settings_theme_grid(
        &self,
        families: &[ThemeFamily],
        preview_tone: ThemeTone,
        columns: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut grid = div().flex().flex_col().gap_3();
        for families in families.chunks(columns) {
            let mut row = div().flex().gap_3();
            for family in families {
                let selected = family.id == self.settings.theme_id;
                let theme_id = family.id.clone();
                row = row.child(
                    div()
                        .id(SharedString::from(format!("settings-theme-{theme_id}")))
                        .flex_1()
                        .min_w(px(0.0))
                        .p_4()
                        .rounded(px(13.0))
                        .border_1()
                        .border_color(if selected {
                            colors().accent
                        } else {
                            colors().border_subtle
                        })
                        .bg(surface_tint(colors().elevated, colors().background))
                        .cursor_pointer()
                        .hover(|card| card.border_color(colors().accent))
                        .child(
                            div()
                                .h(px(86.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .gap_3()
                                .child(theme_orb(
                                    family,
                                    ThemeTone::Light,
                                    selected && preview_tone == ThemeTone::Light,
                                ))
                                .child(theme_orb(
                                    family,
                                    ThemeTone::Dark,
                                    selected && preview_tone == ThemeTone::Dark,
                                )),
                        )
                        .child(
                            div()
                                .pt_2()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .truncate()
                                        .text_size(px(13.0))
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .child(family.label.clone()),
                                )
                                .when(selected, |label| {
                                    label.child(
                                        svg()
                                            .path("chrome-icons/check.svg")
                                            .size(px(15.0))
                                            .flex_none()
                                            .text_color(colors().accent),
                                    )
                                }),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.set_theme_id(&theme_id, window, cx);
                        })),
                );
            }
            for _ in families.len()..columns {
                row = row.child(div().flex_1().min_w(px(0.0)));
            }
            grid = grid.child(row);
        }
        grid.into_any_element()
    }

    fn settings_theme_search(&self) -> Div {
        div()
            .w(px(200.0))
            .h(px(30.0))
            .px_3()
            .rounded(px(7.0))
            .border_1()
            .border_color(colors().border_subtle)
            .flex()
            .items_center()
            .gap_2()
            .child(
                svg()
                    .path("chrome-icons/search.svg")
                    .size(px(14.0))
                    .flex_none()
                    .text_color(colors().subtle),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_size(px(12.0))
                    .text_color(if self.theme_query.is_empty() {
                        colors().subtle
                    } else {
                        colors().foreground
                    })
                    .child(if self.theme_query.is_empty() {
                        "Filter themes…".to_string()
                    } else {
                        self.theme_query.clone()
                    }),
            )
    }

    pub(super) fn appearance_settings(
        &self,
        panel: Stateful<Div>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let system_dark = ThemeTone::from_window_appearance(window.appearance()) == ThemeTone::Dark;
        let preview_tone = theme::resolve_tone(self.appearance_mode(), system_dark);
        let width: f32 = window.bounds().size.width.into();
        let columns = if width - self.left_sidebar_width() >= 760.0 {
            3
        } else {
            2
        };
        let query = self.theme_query.trim().to_ascii_lowercase();
        let matches = |family: &&ThemeFamily| {
            query.is_empty()
                || family.label.to_ascii_lowercase().contains(&query)
                || family.id.to_ascii_lowercase().contains(&query)
        };
        let bundled: Vec<_> = theme::built_in_themes()
            .iter()
            .filter(matches)
            .cloned()
            .collect();
        let user: Vec<_> = theme::user_themes()
            .iter()
            .filter(matches)
            .cloned()
            .collect();
        panel
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(self.settings_section_heading(
                        "Color scheme",
                        "Follow your Mac or choose a light or dark appearance.",
                    ))
                    .child(self.appearance_mode_cards(cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_3()
                            .child(div().flex_1().child(self.settings_section_heading(
                                "Themes",
                                "Colors for the workspace, terminal and code.",
                            )))
                            .child(self.settings_theme_search()),
                    )
                    .when(bundled.is_empty() && user.is_empty(), |section| {
                        section.child(
                            div()
                                .py_6()
                                .text_size(px(13.0))
                                .text_color(colors().muted)
                                .child("No matching themes."),
                        )
                    })
                    .when(!bundled.is_empty(), |section| {
                        section.child(self.settings_theme_grid(&bundled, preview_tone, columns, cx))
                    })
                    .when(!user.is_empty(), |section| {
                        section
                            .child(
                                div()
                                    .pt_3()
                                    .text_size(px(13.0))
                                    .text_color(colors().muted)
                                    .child("Your themes"),
                            )
                            .child(self.settings_theme_grid(&user, preview_tone, columns, cx))
                    })
                    .child(
                        div()
                            .pt_1()
                            .text_size(px(12.0))
                            .text_color(colors().subtle)
                            .child(format!(
                                "Add Warp YAML or Ghostty themes to {}.",
                                theme::user_themes_directory()
                                    .map(|path| path.display().to_string())
                                    .unwrap_or_else(|| "~/.vibra/themes".into())
                            )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(self.settings_section_heading(
                        "Text",
                        "Adjust readability in terminals and code reviews.",
                    ))
                    .child(
                        settings_card()
                            .child(div().p_5().child(self.settings_font_row(
                                FontSizeRow {
                                    label: "Terminal text",
                                    description: "JetBrains Mono in all terminals.",
                                    size: self.settings.terminal_font_size,
                                    ids: [
                                        "settings-font-down",
                                        "settings-font-reset",
                                        "settings-font-up",
                                    ],
                                },
                                cx,
                                |this, size, cx| this.set_terminal_font_size(size, cx),
                            )))
                            .child(div().h(px(1.0)).bg(colors().border_subtle))
                            .child(div().p_5().child(self.settings_font_row(
                                FontSizeRow {
                                    label: "Diff text",
                                    description: "Code in reviews, independent of the terminal.",
                                    size: self.settings.diff_font_size,
                                    ids: [
                                        "settings-diff-font-down",
                                        "settings-diff-font-reset",
                                        "settings-diff-font-up",
                                    ],
                                },
                                cx,
                                |this, size, cx| this.set_diff_font_size(size, cx),
                            ))),
                    ),
            )
    }
}
