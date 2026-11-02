//! Shared look for modal dialogs: a dimmed scrim that fades in, a lifted panel
//! that settles into place, an always-focused field with its caret, keycaps,
//! and action buttons that carry their keyboard shortcut.

use std::time::Duration;

use gpui::{
    Animation, AnimationElement, AnimationExt, BoxShadow, Div, ElementId, FontWeight, Hsla, Rgba,
    SharedString, Stateful, black, div, ease_out_quint, point, prelude::*, px, svg,
};

use crate::ui::menu::{menu_border, menu_surface};
use crate::ui::theme::{MONO_FONT, colors};

/// Corner radius shared by every dialog panel.
pub const DIALOG_RADIUS: f32 = 14.0;
/// Controls inside a panel keep their corners concentric with it.
pub const CONTROL_RADIUS: f32 = 8.0;
const ENTER_DURATION: Duration = Duration::from_millis(200);
/// How far a panel travels as it settles into place.
const ENTER_OFFSET: f32 = 8.0;

fn foreground(alpha: f32) -> Hsla {
    Hsla::from(colors().foreground).opacity(alpha)
}

/// A tight contact shadow keeps the edge crisp and a wide, soft one lifts the
/// panel off the window. Dark palettes need more of both to read at all.
pub fn dialog_shadow() -> Vec<BoxShadow> {
    let dark = colors().is_dark();
    vec![
        BoxShadow {
            color: black().opacity(if dark { 0.32 } else { 0.08 }),
            offset: point(px(0.0), px(1.0)),
            blur_radius: px(2.0),
            spread_radius: px(0.0),
        },
        BoxShadow {
            color: black().opacity(if dark { 0.50 } else { 0.18 }),
            offset: point(px(0.0), px(20.0)),
            blur_radius: px(50.0),
            spread_radius: px(-8.0),
        },
    ]
}

/// Dims everything behind a dialog. Callers place the panel and attach dismissal.
pub fn dialog_scrim() -> Div {
    div()
        .absolute()
        .inset_0()
        .flex()
        .justify_center()
        .bg(colors().overlay())
}

/// Fades an element in the first time it renders.
pub fn fade_in<E: Styled + IntoElement + 'static>(
    id: impl Into<ElementId>,
    element: E,
) -> AnimationElement<E> {
    element.with_animation(
        id,
        Animation::new(ENTER_DURATION).with_easing(ease_out_quint()),
        |element, delta| element.opacity(delta),
    )
}

/// Lets a panel settle upward into place the first time it renders.
pub fn rise_in<E: Styled + IntoElement + 'static>(
    id: impl Into<ElementId>,
    element: E,
) -> AnimationElement<E> {
    element.with_animation(
        id,
        Animation::new(ENTER_DURATION).with_easing(ease_out_quint()),
        |element, delta| element.relative().top(px(ENTER_OFFSET * (1.0 - delta))),
    )
}

/// The lifted surface every dialog sits on.
pub fn dialog_panel() -> Div {
    div()
        .max_w_full()
        .mx_4()
        .flex()
        .flex_col()
        .rounded(px(DIALOG_RADIUS))
        .border_1()
        .border_color(menu_border())
        .bg(menu_surface())
        .shadow(dialog_shadow())
        .text_size(px(13.0))
        .text_color(colors().foreground)
}

/// Icon tile, title, and an optional supporting line at the top of a dialog.
pub fn dialog_header(
    icon: &'static str,
    tone: Rgba,
    title: impl Into<SharedString>,
    detail: Option<SharedString>,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.0))
        .child(
            div()
                .size(px(34.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(CONTROL_RADIUS))
                .border_1()
                .border_color(Hsla::from(tone).opacity(0.22))
                .bg(Hsla::from(tone).opacity(0.12))
                .child(svg().path(icon).size(px(16.0)).text_color(tone)),
        )
        .child(
            div()
                .min_w(px(0.0))
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(14.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors().foreground)
                        .child(title.into()),
                )
                .when_some(detail, |text, detail| {
                    text.child(
                        div()
                            .truncate()
                            .text_size(px(12.0))
                            .text_color(colors().muted)
                            .child(detail),
                    )
                }),
        )
}

/// Marks where typed text lands in fields that are edited at their end.
pub fn caret(height: f32) -> Div {
    div()
        .w(px(1.5))
        .h(px(height))
        .flex_none()
        .rounded(px(1.0))
        .bg(colors().accent)
}

/// A single-line field that owns the keyboard while its dialog is open, so it
/// always shows the focus ring and a caret after the value.
pub fn dialog_field(value: &str, placeholder: &str, mono: bool) -> Div {
    let accent = Hsla::from(colors().accent);
    let empty = value.is_empty();
    div()
        .h(px(38.0))
        .px(px(12.0))
        .flex()
        .items_center()
        .rounded(px(CONTROL_RADIUS))
        .border_1()
        .border_color(accent.opacity(0.7))
        .bg(foreground(0.04))
        .shadow(vec![BoxShadow {
            color: accent.opacity(0.22),
            offset: point(px(0.0), px(0.0)),
            blur_radius: px(4.0),
            spread_radius: px(1.0),
        }])
        .text_size(px(13.0))
        .when(mono, |field| {
            field.font_family(MONO_FONT).text_size(px(12.5))
        })
        .when(!empty, |field| {
            field.child(
                div()
                    .min_w(px(0.0))
                    .truncate()
                    .mr(px(1.0))
                    .text_color(colors().foreground)
                    .child(value.to_owned()),
            )
        })
        .child(caret(16.0))
        .when(empty, |field| {
            field.child(
                div()
                    .min_w(px(0.0))
                    .flex_1()
                    .truncate()
                    .ml(px(1.0))
                    .text_color(colors().subtle)
                    .child(placeholder.to_owned()),
            )
        })
}

/// A key drawn as a small raised cap.
pub fn keycap(key: impl Into<SharedString>) -> Div {
    div()
        .h(px(18.0))
        .min_w(px(18.0))
        .px(px(4.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .border_1()
        .border_color(foreground(0.14))
        .bg(foreground(0.05))
        .font_family(".SystemUIFont")
        .text_size(px(10.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(colors().muted)
        .child(key.into())
}

/// A shortcut such as `⇧⌘D`, one cap per key.
pub fn shortcut_keys(shortcut: &str) -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(2.0))
        .children(shortcut.chars().map(|key| keycap(key.to_string())))
}

/// Keys followed by what they do, for dialog footers.
pub fn key_hint(keys: &[&'static str], label: &'static str) -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(2.0))
                .children(keys.iter().map(|key| keycap(*key))),
        )
        .child(
            div()
                .text_size(px(11.5))
                .text_color(colors().subtle)
                .child(label),
        )
}

/// The bottom strip that holds a dialog's hints or actions.
pub fn dialog_footer() -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(8.0))
        .px(px(16.0))
        .py(px(12.0))
        .border_t_1()
        .border_color(menu_border())
        .bg(foreground(0.025))
        .rounded_b(px(DIALOG_RADIUS - 1.0))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogButton {
    Primary,
    Secondary,
}

/// An action button that shows the key that also triggers it.
pub fn dialog_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    key: &'static str,
    kind: DialogButton,
) -> Stateful<Div> {
    let primary = kind == DialogButton::Primary;
    let ink: Hsla = if primary {
        colors().background.into()
    } else {
        colors().foreground.into()
    };
    div()
        .id(id)
        .h(px(30.0))
        .pl(px(12.0))
        .pr(px(6.0))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(8.0))
        .rounded(px(CONTROL_RADIUS))
        .cursor_pointer()
        .text_size(px(12.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(ink)
        .when(primary, |button| {
            button
                .bg(colors().accent)
                .hover(|button| button.opacity(0.9))
        })
        .when(!primary, |button| {
            button
                .border_1()
                .border_color(menu_border())
                .bg(foreground(0.04))
                .hover(|button| button.bg(foreground(0.08)))
        })
        .child(label.into())
        .child(
            div()
                .h(px(18.0))
                .min_w(px(18.0))
                .px(px(4.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.0))
                .bg(ink.opacity(if primary { 0.18 } else { 0.07 }))
                .font_family(".SystemUIFont")
                .text_size(px(10.5))
                .text_color(ink.opacity(0.72))
                .child(key),
        )
}
