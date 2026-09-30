//! App color roles and built-in palettes.
//!
//! UI code reads the active palette through [`colors`]. Preference resolution
//! (theme id + light/dark/system) lives in [`resolve`] / [`apply_preference`].
//! User Warp YAML / Ghostty files load from `~/.vibra/themes`.

use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use gpui::{Hsla, Rgba, WindowAppearance, rgb, rgba};

pub use crate::domain::appearance::AppearanceMode;

mod palettes;
use crate::ports::terminal::TerminalRgb;
pub use palettes::built_in_themes;
#[cfg(test)]
pub(crate) use palettes::{DEFAULT_THEME_ID, midnight_light, moss_dark};
use palettes::{midnight_dark, seed, theme_from_seed};

pub use crate::ports::terminal::TerminalPalette;
use crate::ui::theme_import::{self, ImportedScheme};

/// Family name of the bundled JetBrains Mono Variable font.
pub const MONO_FONT: &str = "JetBrains Mono";

/// Use one continuous window fill with 12% transparency over the native backdrop.
/// Keep this separate from floating fills, which need more opacity for readability.
pub fn window_surface() -> Hsla {
    let mut color: Hsla = colors().background.into();
    if cfg!(target_os = "macos") {
        color.a *= 0.88;
    }
    color
}

/// Main panels tint the continuous window base. Never paint another full fill
/// here: stacking those layers would hide the native backdrop.
pub fn surface(color: impl Into<Hsla>) -> Hsla {
    surface_tint(color.into().into(), colors().background).into()
}

/// Floating content needs its own fill to remain readable above other content.
/// Keep the 6% transparency at paint time; palette and foreground stay opaque.
pub fn floating_surface(color: impl Into<Hsla>) -> Hsla {
    let mut color = color.into();
    if cfg!(target_os = "macos") {
        color.a *= 0.94;
    }
    color
}

/// Shared fill for app dialogs, menus, and tooltips, matching the sidebar tone.
/// Only the background is translucent; text and icons retain their full opacity.
pub fn popover_surface() -> Hsla {
    let mut color: Hsla = colors().sidebar.into();
    if cfg!(target_os = "macos") {
        color.a *= 0.90;
    }
    color
}

/// Reproduce a nested surface as a tint over its parent instead of covering the
/// backdrop with another opaque fill. Equal colors need no additional paint.
pub fn surface_tint(color: Rgba, base: Rgba) -> Rgba {
    let channels = [(color.r, base.r), (color.g, base.g), (color.b, base.b)];
    let alpha = channels.iter().fold(0.0_f32, |alpha, &(target, base)| {
        alpha.max(if target > base {
            (target - base) / (1.0 - base)
        } else if target < base {
            (base - target) / base
        } else {
            0.0
        })
    });
    if alpha == 0.0 {
        return rgba(0x00000000);
    }
    let channel =
        |target: f32, base: f32| ((target - base * (1.0 - alpha)) / alpha).clamp(0.0, 1.0);
    Rgba {
        r: channel(color.r, base.r),
        g: channel(color.g, base.g),
        b: channel(color.b, base.b),
        a: alpha * color.a,
    }
}

/// Product color roles used across chrome, terminal shell, diffs, and editors.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    pub background: Rgba,
    pub terminal: Rgba,
    pub titlebar: Rgba,
    pub sidebar: Rgba,
    pub panel: Rgba,
    pub elevated: Rgba,
    pub hover: Rgba,
    pub selection: Rgba,
    pub border_subtle: Rgba,
    pub foreground: Rgba,
    pub muted: Rgba,
    pub subtle: Rgba,
    pub success: Rgba,
    pub danger: Rgba,
    pub warning: Rgba,
    pub accent: Rgba,
    pub diff_added: Rgba,
    pub diff_added_bg: Rgba,
    pub diff_deleted: Rgba,
    pub diff_deleted_bg: Rgba,
    pub diff_hunk_bg: Rgba,
    /// Gutter behind line numbers in the diff viewer.
    pub gutter: Rgba,
    /// Soft vertical indent guides in the file tree.
    pub indent_guide: Rgba,
    /// Default folder icon tint.
    pub folder: Rgba,
    /// Git-modified name tint.
    pub git_modified: Rgba,
    /// Git-added / untracked name tint.
    pub git_added: Rgba,
    /// Git-deleted / conflict name tint.
    pub git_deleted: Rgba,
    /// Authentic terminal colors when the palette ships them.
    pub terminal_colors: Option<TerminalPalette>,
}

impl Theme {
    pub fn is_dark(self) -> bool {
        relative_luminance(self.background) < 0.5
    }

    pub fn overlay(self) -> Rgba {
        let mut color = mix(self.background, rgb(0x000000), 0.4);
        color.a = if self.is_dark() { 0.24 } else { 0.16 };
        color
    }

    pub fn terminal_palette(self) -> TerminalPalette {
        self.terminal_colors
            .unwrap_or_else(|| self.synthesize_terminal_palette())
    }

    fn synthesize_terminal_palette(self) -> TerminalPalette {
        let dark = self.is_dark();
        let black = if dark {
            mix(self.terminal, self.foreground, 0.10)
        } else {
            mix(self.foreground, self.terminal, 0.14)
        };
        let white = if dark {
            mix(self.foreground, self.terminal, 0.08)
        } else {
            mix(self.terminal, self.foreground, 0.10)
        };
        let magenta = mix(
            self.danger,
            rgb(if dark { 0xc084fc } else { 0x7c3aed }),
            0.48,
        );
        let cyan = mix(
            self.accent,
            rgb(if dark { 0x22d3ee } else { 0x0e7490 }),
            0.42,
        );
        let normal = [
            black,
            self.danger,
            self.success,
            self.warning,
            self.accent,
            magenta,
            cyan,
            white,
        ];
        let lift = |color: Rgba| {
            if dark {
                mix(color, rgb(0xffffff), 0.16)
            } else {
                mix(color, rgb(0x000000), 0.12)
            }
        };
        let mut ansi = [TerminalRgb::new(0, 0, 0); 16];
        for (index, color) in normal.into_iter().enumerate() {
            ansi[index] = to_terminal_rgb(color);
            ansi[index + 8] = to_terminal_rgb(lift(color));
        }
        TerminalPalette {
            background: to_terminal_rgb(self.terminal),
            foreground: to_terminal_rgb(self.foreground),
            cursor: to_terminal_rgb(self.foreground),
            ansi,
        }
    }
}

pub(crate) fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

pub(super) fn relative_luminance(color: Rgba) -> f32 {
    0.2126 * color.r + 0.7152 * color.g + 0.0722 * color.b
}

pub(super) fn with_alpha(mut color: Rgba, alpha: f32) -> Rgba {
    color.a = alpha.clamp(0.0, 1.0);
    color
}

/// Blend a soft 12% stroke over the actual surface, including its backdrop.
/// A fixed near-background color can disappear as the translucent window moves.
pub(super) fn subtle_border(foreground: Rgba) -> Rgba {
    with_alpha(foreground, 0.12)
}

pub fn to_terminal_rgb(color: Rgba) -> TerminalRgb {
    TerminalRgb::new(
        (color.r * 255.0).round().clamp(0.0, 255.0) as u8,
        (color.g * 255.0).round().clamp(0.0, 255.0) as u8,
        (color.b * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

#[cfg(test)]
pub fn terminal_palette() -> TerminalPalette {
    colors().terminal_palette()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeTone {
    Light,
    Dark,
}

impl ThemeTone {
    pub fn from_system_dark(system_dark: bool) -> Self {
        if system_dark { Self::Dark } else { Self::Light }
    }

    pub fn from_window_appearance(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::Light,
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::Dark,
        }
    }
}

/// A dual-mode palette shown as one card in Settings.
#[derive(Debug, Clone)]
pub struct ThemeFamily {
    pub id: String,
    pub label: String,
    pub light: Theme,
    pub dark: Theme,
}

impl ThemeFamily {
    pub fn colors(&self, tone: ThemeTone) -> Theme {
        match tone {
            ThemeTone::Light => self.light,
            ThemeTone::Dark => self.dark,
        }
    }

    /// Three swatches for the settings preview (sidebar, surface, accent).
    pub fn preview(&self, tone: ThemeTone) -> [Rgba; 3] {
        let colors = self.colors(tone);
        [colors.sidebar, colors.panel, colors.accent]
    }
}

const USER_THEME_PREFIX: &str = "user:";
const MAX_USER_THEMES: usize = 200;

fn user_theme_slot() -> &'static RwLock<Vec<ThemeFamily>> {
    static SLOT: OnceLock<RwLock<Vec<ThemeFamily>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(Vec::new()))
}

/// `~/.vibra/themes` when a home directory is available.
pub fn user_themes_directory() -> Option<PathBuf> {
    directories::BaseDirs::new()
        .map(|directories| directories.home_dir().join(".vibra").join("themes"))
}

pub fn user_themes() -> Vec<ThemeFamily> {
    user_theme_slot()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// Reload Warp YAML / Ghostty files from `~/.vibra/themes`.
pub fn refresh_user_themes() {
    let themes = user_themes_directory()
        .map(|directory| load_user_themes(&directory))
        .unwrap_or_default();
    *user_theme_slot()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = themes;
}

pub fn load_user_themes(directory: &Path) -> Vec<ThemeFamily> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut loaded = Vec::new();
    for entry in entries.flatten() {
        if loaded.len() >= MAX_USER_THEMES {
            break;
        }
        let path = entry.path();
        if !theme_import::is_theme_file(&path) {
            continue;
        }
        let Ok(scheme) = theme_import::parse_theme_file(&path) else {
            continue;
        };
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("theme");
        loaded.push(LoadedUserTheme {
            stem: stem.to_string(),
            group: grouping_key(stem),
            label: scheme
                .name
                .clone()
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| pretty_label(stem)),
            dark: scheme_is_dark(&scheme),
            theme: theme_from_scheme(&scheme),
        });
    }
    pair_user_themes(loaded)
}

struct LoadedUserTheme {
    stem: String,
    group: String,
    label: String,
    dark: bool,
    theme: Theme,
}

fn pair_user_themes(loaded: Vec<LoadedUserTheme>) -> Vec<ThemeFamily> {
    let mut groups: Vec<(String, Vec<LoadedUserTheme>)> = Vec::new();
    for item in loaded {
        if let Some((_, members)) = groups.iter_mut().find(|(key, _)| *key == item.group) {
            members.push(item);
        } else {
            groups.push((item.group.clone(), vec![item]));
        }
    }
    let mut families = Vec::new();
    for (group, mut members) in groups {
        members.sort_by(|left, right| left.stem.cmp(&right.stem));
        let light = members.iter().find(|item| !item.dark);
        let dark = members.iter().find(|item| item.dark);
        if members.len() == 2
            && let (Some(light), Some(dark)) = (light, dark)
        {
            families.push(ThemeFamily {
                id: format!("{USER_THEME_PREFIX}{group}"),
                label: preferred_user_label(&light.label, &dark.label, &group),
                light: light.theme,
                dark: dark.theme,
            });
            continue;
        }
        for item in members {
            let id_stem = sanitize_theme_id(&item.stem);
            families.push(ThemeFamily {
                id: format!("{USER_THEME_PREFIX}{id_stem}"),
                label: item.label,
                light: item.theme,
                dark: item.theme,
            });
        }
    }
    families.sort_by(|left, right| {
        left.label
            .to_ascii_lowercase()
            .cmp(&right.label.to_ascii_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    families
}

fn preferred_user_label(light: &str, dark: &str, group: &str) -> String {
    let light_key = grouping_key(light);
    let dark_key = grouping_key(dark);
    if light_key == dark_key || light_key == group {
        light.to_string()
    } else if dark_key == group {
        dark.to_string()
    } else {
        pretty_label(group)
    }
}

fn grouping_key(stem: &str) -> String {
    let lowered = stem.to_ascii_lowercase();
    let stripped = lowered
        .strip_suffix("-dark")
        .or_else(|| lowered.strip_suffix("_dark"))
        .or_else(|| lowered.strip_suffix("-light"))
        .or_else(|| lowered.strip_suffix("_light"))
        .or_else(|| lowered.strip_suffix("-darker"))
        .or_else(|| lowered.strip_suffix("-lighter"))
        .or_else(|| lowered.strip_suffix(" dark"))
        .or_else(|| lowered.strip_suffix(" light"))
        .unwrap_or(&lowered);
    let id = sanitize_theme_id(stripped);
    if id.is_empty() {
        "theme".to_string()
    } else {
        id
    }
}

fn sanitize_theme_id(raw: &str) -> String {
    let mut out = String::new();
    for character in raw.chars() {
        if character.is_ascii_alphanumeric() {
            out.push(character.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn pretty_label(raw: &str) -> String {
    raw.replace(['-', '_'], " ")
        .split_whitespace()
        .map(|word| {
            let mut characters = word.chars();
            match characters.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), characters.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn scheme_is_dark(scheme: &ImportedScheme) -> bool {
    scheme.dark.unwrap_or_else(|| {
        let red = ((scheme.background >> 16) & 0xff) as f32 / 255.0;
        let green = ((scheme.background >> 8) & 0xff) as f32 / 255.0;
        let blue = (scheme.background & 0xff) as f32 / 255.0;
        0.2126 * red + 0.7152 * green + 0.0722 * blue < 0.5
    })
}

fn theme_from_scheme(scheme: &ImportedScheme) -> Theme {
    let accent = scheme
        .accent
        .or(scheme.ansi[4])
        .unwrap_or(scheme.foreground);
    let danger = scheme.ansi[1].unwrap_or(0xdc2626);
    let success = scheme.ansi[2].unwrap_or(0x16a34a);
    let warning = scheme.ansi[3].unwrap_or(0xca8a04);
    let mut spec = seed(
        scheme.background,
        scheme.foreground,
        accent,
        danger,
        success,
        warning,
    );
    if let Some(cursor) = scheme.cursor.or(scheme.accent) {
        spec = spec.with_cursor(cursor);
    }
    if scheme.ansi.iter().any(Option::is_some) {
        let synthesized = theme_from_seed(spec).synthesize_terminal_palette();
        let mut ansi = [0u32; 16];
        for (index, color) in scheme.ansi.into_iter().enumerate() {
            ansi[index] = color.unwrap_or_else(|| {
                let rgb = synthesized.ansi[index];
                (u32::from(rgb.red) << 16) | (u32::from(rgb.green) << 8) | u32::from(rgb.blue)
            });
        }
        spec = spec.with_terminal(ansi);
    }
    theme_from_seed(spec)
}

fn family_by_id(id: &str) -> ThemeFamily {
    built_in_themes()
        .iter()
        .find(|family| family.id == id)
        .cloned()
        .or_else(|| user_themes().into_iter().find(|family| family.id == id))
        .unwrap_or_else(|| built_in_themes()[0].clone())
}

pub fn canonicalize_theme_id(id: &str) -> String {
    family_by_id(id).id
}

pub fn resolve_tone(mode: AppearanceMode, system_dark: bool) -> ThemeTone {
    match mode {
        AppearanceMode::Light => ThemeTone::Light,
        AppearanceMode::Dark => ThemeTone::Dark,
        AppearanceMode::System => ThemeTone::from_system_dark(system_dark),
    }
}

pub fn resolve(theme_id: &str, mode: AppearanceMode, system_dark: bool) -> Theme {
    family_by_id(theme_id).colors(resolve_tone(mode, system_dark))
}

// ---------------------------------------------------------------------------
// Active palette (shared by all views during paint)
// ---------------------------------------------------------------------------

fn active_slot() -> &'static RwLock<Theme> {
    static SLOT: OnceLock<RwLock<Theme>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(midnight_dark()))
}

/// Colors currently painted by the app shell.
///
/// One `RwLock` read per thread per generation — later calls reuse a
/// thread-local copy until [`set_active`] bumps the generation.
pub fn colors() -> Theme {
    use std::cell::Cell;
    use std::sync::atomic::Ordering;

    thread_local! {
        static CACHED: Cell<Option<(u64, Theme)>> = const { Cell::new(None) };
    }

    let generation = theme_generation().load(Ordering::Acquire);
    CACHED.with(|cached| {
        if let Some((cached_generation, theme)) = cached.get()
            && cached_generation == generation
        {
            return theme;
        }
        let theme = *active_slot()
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cached.set(Some((generation, theme)));
        theme
    })
}

fn theme_generation() -> &'static std::sync::atomic::AtomicU64 {
    use std::sync::atomic::AtomicU64;
    static GENERATION: AtomicU64 = AtomicU64::new(1);
    &GENERATION
}

pub fn generation() -> u64 {
    theme_generation().load(std::sync::atomic::Ordering::Acquire)
}

pub fn set_active(theme: Theme) {
    *active_slot()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = theme;
    let generation = theme_generation().fetch_add(1, std::sync::atomic::Ordering::Release) + 1;
    crate::infrastructure::publish_terminal_palette(generation, theme.terminal_palette());
}

/// Resolve preference and install it as the active palette. Returns the result.
pub fn apply_preference(theme_id: &str, mode: AppearanceMode, system_dark: bool) -> Theme {
    let theme = resolve(theme_id, mode, system_dark);
    set_active(theme);
    theme
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_tints_preserve_colors_when_composited() {
        for theme in [midnight_dark(), midnight_light()] {
            for base in [
                theme.background,
                theme.panel,
                theme.sidebar,
                rgb(0),
                rgb(0xffffff),
            ] {
                for target in [base, theme.elevated, theme.selection, theme.diff_added_bg] {
                    let tint = surface_tint(target, base);
                    assert!((0.0..=1.0).contains(&tint.a));
                    for (channel, base, target) in [
                        (tint.r, base.r, target.r),
                        (tint.g, base.g, target.g),
                        (tint.b, base.b, target.b),
                    ] {
                        let composited = channel * tint.a + base * (1.0 - tint.a);
                        assert!((composited - target).abs() < 0.00001);
                    }
                }
            }
        }
    }

    #[test]
    fn catalog_ids_are_unique_and_default_resolves() {
        let mut seen = std::collections::HashSet::new();
        for family in built_in_themes() {
            assert!(
                seen.insert(family.id.clone()),
                "duplicate theme id {}",
                family.id
            );
            assert!(!family.label.is_empty());
            assert!(!family.id.starts_with(USER_THEME_PREFIX));
        }
        assert_eq!(canonicalize_theme_id("missing"), DEFAULT_THEME_ID);
        assert_eq!(
            resolve(DEFAULT_THEME_ID, AppearanceMode::Dark, true).background,
            midnight_dark().background
        );
        assert_eq!(
            resolve(DEFAULT_THEME_ID, AppearanceMode::Light, false).background,
            midnight_light().background
        );
        assert_eq!(
            resolve("moss", AppearanceMode::System, true).accent,
            moss_dark().accent
        );
        assert!(built_in_themes().len() >= 30);
        for id in [
            "nord",
            "gruvbox",
            "catppuccin",
            "tokyo",
            "warp",
            "flexoki",
            "tokyo-storm",
            "catppuccin-frappe",
        ] {
            assert!(seen.contains(id), "missing theme id {id}");
        }
        assert!(resolve("nord", AppearanceMode::Dark, true).is_dark());
        assert!(!resolve("nord", AppearanceMode::Light, false).is_dark());
        assert!(resolve("gruvbox", AppearanceMode::Dark, true).is_dark());
        assert!(!resolve("solarized", AppearanceMode::Light, false).is_dark());
    }

    #[test]
    fn popular_palettes_keep_authentic_ansi() {
        let nord = resolve("nord", AppearanceMode::Dark, true).terminal_palette();
        assert_eq!(nord.ansi[0], TerminalRgb::new(0x3b, 0x42, 0x52));
        assert_eq!(nord.ansi[1], TerminalRgb::new(0xbf, 0x61, 0x6a));
        assert_eq!(nord.ansi[5], TerminalRgb::new(0xb4, 0x8e, 0xad));
        let gruvbox = resolve("gruvbox", AppearanceMode::Dark, true).terminal_palette();
        assert_eq!(gruvbox.ansi[5], TerminalRgb::new(0xb1, 0x62, 0x86));
        assert!(
            resolve("midnight", AppearanceMode::Dark, true)
                .terminal_colors
                .is_none()
        );
    }

    #[test]
    fn user_themes_load_and_pair_light_dark_files() {
        let root = std::env::temp_dir().join(format!("vibra-themes-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("nord.yaml"),
            r###"
name: Nord
background: "#2E3440"
foreground: "#D8DEE9"
details: darker
accent: "#81A1C1"
terminal_colors:
  normal:
    black: "#3B4252"
    red: "#BF616A"
    green: "#A3BE8C"
    yellow: "#EBCB8B"
    blue: "#81A1C1"
    magenta: "#B48EAD"
    cyan: "#88C0D0"
    white: "#E5E9F0"
  bright:
    black: "#4C566A"
    red: "#BF616A"
    green: "#A3BE8C"
    yellow: "#EBCB8B"
    blue: "#81A1C1"
    magenta: "#B48EAD"
    cyan: "#8FBCBB"
    white: "#ECEFF4"
"###,
        )
        .unwrap();
        std::fs::write(
            root.join("nord-light.yaml"),
            r###"
name: Nord Light
background: "#ECEFF4"
foreground: "#2E3440"
details: lighter
accent: "#5E81AC"
"###,
        )
        .unwrap();
        std::fs::write(
            root.join("solo.conf"),
            "background = #101010\nforeground = #ffffff\npalette = 1=#ff8080\n",
        )
        .unwrap();

        let themes = load_user_themes(&root);
        std::fs::remove_dir_all(&root).unwrap();
        assert!(themes.iter().any(|family| family.id == "user:nord"));
        let nord = themes
            .iter()
            .find(|family| family.id == "user:nord")
            .unwrap();
        assert!(nord.dark.is_dark());
        assert!(!nord.light.is_dark());
        assert_eq!(
            nord.dark.terminal_palette().ansi[1],
            TerminalRgb::new(0xbf, 0x61, 0x6a)
        );
        let solo = themes
            .iter()
            .find(|family| family.id == "user:solo")
            .unwrap();
        assert_eq!(solo.light.background, solo.dark.background);
    }

    #[test]
    fn active_palette_round_trips() {
        let before = colors();
        set_active(moss_dark());
        assert_eq!(colors().accent, moss_dark().accent);
        set_active(before);
    }

    #[test]
    fn terminal_palette_follows_the_active_theme() {
        let before = colors();
        set_active(moss_dark());
        let moss = terminal_palette();
        assert_eq!(moss.background, to_terminal_rgb(moss_dark().terminal));
        assert_eq!(moss.foreground, to_terminal_rgb(moss_dark().foreground));
        assert_ne!(moss.background, to_terminal_rgb(midnight_dark().terminal));

        set_active(midnight_light());
        let light = terminal_palette();
        assert_eq!(light.background, to_terminal_rgb(midnight_light().terminal));
        assert_eq!(
            light.foreground,
            to_terminal_rgb(midnight_light().foreground)
        );
        assert_ne!(light.background, moss.background);
        assert!(midnight_light().overlay().a > 0.0);
        assert!(midnight_dark().is_dark());
        assert!(!midnight_light().is_dark());
        set_active(before);
    }
}
