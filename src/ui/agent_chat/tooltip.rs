//! Tooltip styled like the pane header tooltips. It mirrors
//! `workspace_view::sidebar_tooltip`, which is private to that module.

use gpui::{AnyView, App, IntoElement, Render, SharedString, Window, div, prelude::*, px};

use crate::ui::menu::{menu_border, menu_surface};
use crate::ui::theme::colors;

struct ChatTooltip(SharedString);

impl Render for ChatTooltip {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        // "Label · ⌘K" draws the shortcut quieter than the label.
        let (label, shortcut) = match self.0.rsplit_once(" · ") {
            Some((label, shortcut)) => (label.to_owned(), Some(shortcut.to_owned())),
            None => (self.0.to_string(), None),
        };
        div()
            .max_w(px(420.0))
            .px(px(8.0))
            .py(px(5.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .rounded(px(7.0))
            .bg(menu_surface())
            .border_1()
            .border_color(menu_border())
            .shadow_md()
            .text_size(px(12.0))
            .text_color(colors().foreground)
            .child(label)
            .when_some(shortcut, |tooltip, shortcut| {
                tooltip.child(div().text_color(colors().subtle).child(shortcut))
            })
    }
}

pub(super) fn tooltip(label: impl Into<SharedString>, cx: &mut App) -> AnyView {
    cx.new(|_| ChatTooltip(label.into())).into()
}
