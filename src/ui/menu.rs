//! Shared look for popover menus and tooltips: a lifted surface, icon rows,
//! right-aligned shortcuts, and separators.

use gpui::{Div, Hsla, Rgba, SharedString, Stateful, div, prelude::*, px, svg};

use crate::ui::theme::colors;

pub const MENU_WIDTH: f32 = 232.0;
const ROW_HEIGHT: f32 = 28.0;

fn foreground_alpha(alpha: f32) -> Rgba {
    Rgba {
        a: alpha,
        ..colors().foreground
    }
}

/// Menus sit one step above the chrome so they read as floating.
pub fn menu_surface() -> Hsla {
    let mut color: Hsla = colors().elevated.into();
    if cfg!(target_os = "macos") {
        color.a *= 0.97;
    }
    color
}

/// Visible on any surface; the theme's subtle border can match `elevated`.
pub fn menu_border() -> Rgba {
    foreground_alpha(0.10)
}

pub fn menu_hover() -> Rgba {
    foreground_alpha(0.08)
}

pub fn menu_panel() -> Div {
    div()
        .min_w(px(MENU_WIDTH))
        .p(px(4.0))
        .flex()
        .flex_col()
        .rounded(px(10.0))
        .border_1()
        .border_color(menu_border())
        .bg(menu_surface())
        .shadow_lg()
        .text_size(px(13.0))
}

pub struct MenuRow {
    pub icon: Option<&'static str>,
    pub label: SharedString,
    pub shortcut: Option<&'static str>,
    pub danger: bool,
    pub checked: bool,
}

impl MenuRow {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            icon: None,
            label: label.into(),
            shortcut: None,
            danger: false,
            checked: false,
        }
    }

    pub fn icon(mut self, icon: &'static str) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn shortcut(mut self, shortcut: &'static str) -> Self {
        self.shortcut = Some(shortcut);
        self
    }

    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    pub fn render(self, id: impl Into<gpui::ElementId>) -> Stateful<Div> {
        let danger = self.danger;
        let label_color = if danger {
            colors().danger
        } else {
            colors().foreground
        };
        let icon_color = if danger {
            colors().danger
        } else {
            colors().muted
        };
        div()
            .id(id)
            .h(px(ROW_HEIGHT))
            .px(px(8.0))
            .rounded(px(6.0))
            .flex()
            .items_center()
            .gap(px(10.0))
            .cursor_pointer()
            .text_color(label_color)
            .hover(move |row| {
                if danger {
                    row.bg(Rgba {
                        a: 0.12,
                        ..colors().danger
                    })
                } else {
                    row.bg(menu_hover())
                }
            })
            .when_some(self.icon, |row, icon| {
                row.child(
                    svg()
                        .path(icon)
                        .size(px(15.0))
                        .flex_none()
                        .text_color(icon_color),
                )
            })
            .child(div().flex_1().min_w(px(0.0)).truncate().child(self.label))
            .when(self.checked, |row| {
                row.child(
                    svg()
                        .path("chrome-icons/check.svg")
                        .size(px(14.0))
                        .flex_none()
                        .text_color(colors().accent),
                )
            })
            .when_some(self.shortcut, |row, shortcut| {
                row.child(
                    div()
                        .flex_none()
                        .text_size(px(12.0))
                        .text_color(colors().subtle)
                        .child(shortcut),
                )
            })
    }
}

pub fn menu_separator() -> Div {
    div()
        .h(px(1.0))
        .mx(px(8.0))
        .my(px(4.0))
        .flex_none()
        .bg(menu_border())
}

pub fn menu_heading(label: impl Into<SharedString>) -> Div {
    div()
        .h(px(26.0))
        .px(px(8.0))
        .flex()
        .items_center()
        .text_size(px(11.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(colors().subtle)
        .child(label.into())
}
