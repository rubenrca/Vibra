//! The shared look of every tab strip and segmented control. The selected
//! tab is lifted off the surface it sits on, with a fill of its own and a
//! soft shadow; the others stay flat until hovered. Segmented controls set
//! their tabs in a recessed track.

use gpui::{Div, Hsla, Rgba, Stateful, div, prelude::*, px};

use crate::ui::theme::{HOVER_LIFT, colors, floating_surface, lift, mix, surface_tint};

/// Tabs keep squared-off corners whatever their height.
pub const TAB_RADIUS: f32 = 7.0;

/// The lifted tab's fill over `surface`. It is opaque enough that its
/// shadow never shows through.
pub fn selected_fill(surface: Rgba) -> Hsla {
    floating_surface(lift(surface, 1.0))
}

/// An unselected tab under the pointer, partway to the lifted one.
fn hover_fill(surface: Rgba) -> Rgba {
    surface_tint(lift(surface, HOVER_LIFT), surface)
}

/// Gives `tab` the look for its state over `surface`, the color it sits
/// on. Callers keep its size in the strip, padding, and cursor.
pub fn lifted_tab(tab: Stateful<Div>, selected: bool, surface: Rgba, height: f32) -> Stateful<Div> {
    let tab = tab
        .h(px(height))
        .flex()
        .items_center()
        .rounded(px(TAB_RADIUS));
    if selected {
        tab.bg(selected_fill(surface))
            .shadow_sm()
            .text_color(colors().foreground)
    } else {
        let hover = hover_fill(surface);
        tab.text_color(colors().muted)
            .hover(move |tab| tab.bg(hover).text_color(colors().foreground))
    }
}

/// The recessed track that holds a segmented control's tabs.
pub fn track(surface: Rgba) -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .p(px(2.0))
        .gap(px(2.0))
        .rounded(px(TAB_RADIUS + 2.0))
        .bg(surface_tint(
            mix(surface, colors().foreground, 0.05),
            surface,
        ))
}
