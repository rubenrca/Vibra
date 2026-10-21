//! Contrast floors applied to every palette before it reaches the UI.
//!
//! Upstream palettes are tuned for terminal text, so their status colors and
//! the secondary text derived from them can fade into light backgrounds. Each
//! role moves along OKLCH lightness only as far as its floor requires, which
//! keeps its hue. Authentic terminal colors are stored apart and stay intact.

use gpui::Rgba;

use super::{Theme, mix};

/// Primary text against every chrome surface.
const FOREGROUND: f32 = 5.5;
/// Secondary labels, punctuation and inactive titles.
const MUTED: f32 = 4.5;
/// Tertiary hints, timestamps and placeholders.
const SUBTLE: f32 = 3.0;
/// Accent, status, Git and folder marks drawn as icons or short labels.
const SIGNAL: f32 = 3.0;
/// Diff counts and line numbers drawn over their own tinted rows.
const DIFF_TEXT: f32 = 4.5;
/// OKLab distance (×100) that keeps a selected row apart from a hovered one.
const SELECTION_SEPARATION: f32 = 2.5;

/// Raise each text and signal role to its floor without touching surfaces.
pub(super) fn legible(mut theme: Theme) -> Theme {
    let surfaces = [theme.background, theme.sidebar, theme.panel, theme.elevated];
    let lighten = theme.is_dark();
    let floor = |color: Rgba, min: f32| ensure_contrast(color, &surfaces, min, lighten);

    theme.foreground = floor(theme.foreground, FOREGROUND);
    theme.muted = floor(theme.muted, MUTED);
    theme.subtle = floor(theme.subtle, SUBTLE);
    for color in [
        &mut theme.accent,
        &mut theme.success,
        &mut theme.danger,
        &mut theme.warning,
        &mut theme.folder,
        &mut theme.git_modified,
        &mut theme.git_added,
        &mut theme.git_deleted,
    ] {
        *color = floor(*color, SIGNAL);
    }
    let added_row = composite(theme.diff_added_bg, theme.background);
    let deleted_row = composite(theme.diff_deleted_bg, theme.background);
    theme.diff_added = ensure_contrast(
        floor(theme.diff_added, SIGNAL),
        &[added_row],
        DIFF_TEXT,
        lighten,
    );
    theme.diff_deleted = ensure_contrast(
        floor(theme.diff_deleted, SIGNAL),
        &[deleted_row],
        DIFF_TEXT,
        lighten,
    );
    theme.selection = separate_selection(theme.selection, theme.hover, theme.accent);
    theme
}

/// WCAG 2 contrast ratio between two opaque colors.
pub(super) fn contrast_ratio(a: Rgba, b: Rgba) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// Perceptual distance in OKLab, scaled so 1.0 is roughly a just-visible step.
pub(super) fn distance(a: Rgba, b: Rgba) -> f32 {
    let (a, b) = (Oklab::from(a), Oklab::from(b));
    ((a.l - b.l).powi(2) + (a.a - b.a).powi(2) + (a.b - b.b).powi(2)).sqrt() * 100.0
}

/// Move `color` away from the surfaces until it clears `min` against all of
/// them. Colors that already pass are returned unchanged.
fn ensure_contrast(color: Rgba, surfaces: &[Rgba], min: f32, lighten: bool) -> Rgba {
    let worst = |color: Rgba| {
        surfaces
            .iter()
            .map(|&surface| contrast_ratio(color, surface))
            .fold(f32::INFINITY, f32::min)
    };
    if worst(color) >= min {
        return color;
    }
    let lab = Oklab::from(color);
    let at = |lightness: f32| {
        Oklab {
            l: lightness,
            ..lab
        }
        .to_rgba(color.a)
    };
    // Binary-search the smallest lightness shift that clears the floor, with a
    // little headroom so 8-bit rounding on screen never lands below it.
    let goal = min + 0.05;
    let (mut near, mut far) = (lab.l, if lighten { 1.0 } else { 0.0 });
    if worst(at(far)) < goal {
        return at(far);
    }
    for _ in 0..24 {
        let middle = (near + far) / 2.0;
        if worst(at(middle)) >= goal {
            far = middle;
        } else {
            near = middle;
        }
    }
    at(far)
}

/// Lean a selection that reads like a hover toward the accent until the two
/// states are distinguishable.
fn separate_selection(selection: Rgba, hover: Rgba, accent: Rgba) -> Rgba {
    let apart = |color: Rgba| distance(color, hover) >= SELECTION_SEPARATION;
    if apart(selection) {
        return selection;
    }
    (1..=40)
        .map(|step| mix(selection, accent, step as f32 * 0.0125))
        .find(|&color| apart(color))
        .unwrap_or(selection)
}

fn composite(color: Rgba, base: Rgba) -> Rgba {
    mix(base, Rgba { a: 1.0, ..color }, color.a)
}

fn luminance(color: Rgba) -> f32 {
    0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
}

fn linear(channel: f32) -> f32 {
    if channel <= 0.04045 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

fn encoded(channel: f32) -> f32 {
    if channel <= 0.003_130_8 {
        channel * 12.92
    } else {
        1.055 * channel.powf(1.0 / 2.4) - 0.055
    }
}

#[derive(Debug, Clone, Copy)]
struct Oklab {
    l: f32,
    a: f32,
    b: f32,
}

impl From<Rgba> for Oklab {
    fn from(color: Rgba) -> Self {
        let (r, g, b) = (linear(color.r), linear(color.g), linear(color.b));
        let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
        let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
        let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
        Self {
            l: 0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
            a: 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
            b: 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
        }
    }
}

impl Oklab {
    fn linear_rgb(self) -> [f32; 3] {
        let l = (self.l + 0.396_337_78 * self.a + 0.215_803_76 * self.b).powi(3);
        let m = (self.l - 0.105_561_346 * self.a - 0.063_854_17 * self.b).powi(3);
        let s = (self.l - 0.089_484_18 * self.a - 1.291_485_5 * self.b).powi(3);
        [
            4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
            -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
            -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
        ]
    }

    /// Convert to sRGB, shedding chroma (never lightness or hue) when the
    /// color falls outside the gamut.
    fn to_rgba(self, alpha: f32) -> Rgba {
        let fits = |chroma: f32| {
            let scaled = Self {
                a: self.a * chroma,
                b: self.b * chroma,
                ..self
            };
            let channels = scaled.linear_rgb();
            channels
                .iter()
                .all(|channel| (-1e-4..=1.0 + 1e-4).contains(channel))
                .then_some(channels)
        };
        let channels = fits(1.0).unwrap_or_else(|| {
            let (mut inside, mut outside) = (0.0_f32, 1.0_f32);
            for _ in 0..20 {
                let middle = (inside + outside) / 2.0;
                if fits(middle).is_some() {
                    inside = middle;
                } else {
                    outside = middle;
                }
            }
            fits(inside).unwrap_or([self.l.clamp(0.0, 1.0).powi(3); 3])
        });
        let [r, g, b] = channels.map(|channel| encoded(channel.clamp(0.0, 1.0)));
        Rgba { r, g, b, a: alpha }
    }
}

#[cfg(test)]
mod tests {
    use gpui::rgb;

    use super::*;
    use crate::ui::theme::built_in_themes;

    #[test]
    fn oklab_round_trips_srgb() {
        for hex in [0x000000, 0xffffff, 0x528bff, 0xdd6b6b, 0x1f8a4c, 0xf2ae49] {
            let color = rgb(hex);
            let back = Oklab::from(color).to_rgba(1.0);
            for (a, b) in [(color.r, back.r), (color.g, back.g), (color.b, back.b)] {
                assert!((a - b).abs() < 1e-3, "{hex:06x} drifted");
            }
        }
    }

    #[test]
    fn contrast_ratio_matches_wcag_reference() {
        assert!((contrast_ratio(rgb(0x000000), rgb(0xffffff)) - 21.0).abs() < 0.01);
        assert!((contrast_ratio(rgb(0x767676), rgb(0xffffff)) - 4.54).abs() < 0.01);
    }

    #[test]
    fn floors_keep_hue_and_leave_passing_colors_alone() {
        let light = [rgb(0xeceff4)];
        let faded = rgb(0xa3be8c);
        let lifted = ensure_contrast(faded, &light, SIGNAL, false);
        assert!(contrast_ratio(lifted, light[0]) >= SIGNAL);
        let (before, after) = (Oklab::from(faded), Oklab::from(lifted));
        let hue = |lab: Oklab| lab.b.atan2(lab.a);
        assert!((hue(before) - hue(after)).abs() < 0.05);
        let crisp = rgb(0x1f2328);
        assert_eq!(ensure_contrast(crisp, &light, SIGNAL, false), crisp);
    }

    #[test]
    fn every_built_in_palette_meets_its_floors() {
        for family in built_in_themes() {
            for theme in [family.light, family.dark] {
                let surfaces = [theme.background, theme.sidebar, theme.panel, theme.elevated];
                let worst = |color: Rgba| {
                    surfaces
                        .iter()
                        .map(|&surface| contrast_ratio(color, surface))
                        .fold(f32::INFINITY, f32::min)
                };
                let id = &family.id;
                assert!(
                    worst(theme.foreground) >= FOREGROUND - 0.01,
                    "{id} foreground"
                );
                assert!(worst(theme.muted) >= MUTED - 0.01, "{id} muted");
                assert!(worst(theme.subtle) >= SUBTLE - 0.01, "{id} subtle");
                for color in [theme.accent, theme.success, theme.danger, theme.warning] {
                    assert!(worst(color) >= SIGNAL - 0.01, "{id} signal");
                }
                let added_row = composite(theme.diff_added_bg, theme.background);
                assert!(
                    contrast_ratio(theme.diff_added, added_row) >= DIFF_TEXT - 0.01,
                    "{id} diff"
                );
                assert!(
                    distance(theme.selection, theme.hover) >= SELECTION_SEPARATION - 0.01,
                    "{id}"
                );
            }
        }
    }
}
