//! App color roles and built-in palettes.
//!
//! UI code reads the active palette through [`colors`]. Preference resolution
//! (theme id + light/dark/system) lives in [`resolve`] / [`apply_preference`].

use std::sync::{LazyLock, OnceLock, RwLock};

use gpui::{Rgba, WindowAppearance, rgb, rgba};

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
}

/// How the app chooses light vs dark for dual-mode palettes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AppearanceMode {
    Light,
    Dark,
    #[default]
    System,
}

impl AppearanceMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
            Self::System => "system",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "light" => Self::Light,
            "dark" => Self::Dark,
            _ => Self::System,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeTone {
    Light,
    Dark,
}

impl ThemeTone {
    pub fn from_system_dark(system_dark: bool) -> Self {
        if system_dark {
            Self::Dark
        } else {
            Self::Light
        }
    }

    pub fn from_window_appearance(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::Light,
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::Dark,
        }
    }
}

/// A dual-mode palette shown as one card in Settings.
#[derive(Debug, Clone, Copy)]
pub struct ThemeFamily {
    pub id: &'static str,
    pub label: &'static str,
    pub light: Theme,
    pub dark: Theme,
}

impl ThemeFamily {
    pub fn colors(self, tone: ThemeTone) -> Theme {
        match tone {
            ThemeTone::Light => self.light,
            ThemeTone::Dark => self.dark,
        }
    }

    /// Three swatches for the settings preview (sidebar, surface, accent).
    pub fn preview(self, tone: ThemeTone) -> [Rgba; 3] {
        let colors = self.colors(tone);
        [colors.sidebar, colors.panel, colors.accent]
    }
}

pub const DEFAULT_THEME_ID: &str = "midnight";

// ---------------------------------------------------------------------------
// Built-in palettes
// ---------------------------------------------------------------------------

fn midnight_dark() -> Theme {
    Theme {
        background: rgb(0x101011),
        terminal: rgb(0x101011),
        titlebar: rgb(0x101011),
        sidebar: rgb(0x141415),
        panel: rgb(0x18181a),
        elevated: rgb(0x1c1c1f),
        hover: rgb(0x222226),
        selection: rgb(0x2a2a2e),
        border_subtle: rgb(0x29292b),
        foreground: rgb(0xd4d4d8),
        muted: rgb(0x9898a0),
        subtle: rgb(0x6c6c74),
        success: rgb(0x58b87a),
        danger: rgb(0xdd6b6b),
        warning: rgb(0xd7ad61),
        accent: rgb(0x82aaff),
        diff_added: rgb(0x6bcf8e),
        diff_added_bg: rgba(0x1a3322ff),
        diff_deleted: rgb(0xe06c75),
        diff_deleted_bg: rgba(0x3a1c20ff),
        diff_hunk_bg: rgba(0x161b26ff),
        gutter: rgb(0x121213),
        indent_guide: rgba(0x3a3a3e55),
        folder: rgb(0x8b8ba3),
        git_modified: rgb(0xdcb67a),
        git_added: rgb(0x6bcf8e),
        git_deleted: rgb(0xe06c75),
    }
}

fn midnight_light() -> Theme {
    Theme {
        background: rgb(0xf4f4f5),
        terminal: rgb(0xfafafa),
        titlebar: rgb(0xf4f4f5),
        sidebar: rgb(0xecebee),
        panel: rgb(0xffffff),
        elevated: rgb(0xffffff),
        hover: rgb(0xe8e8ec),
        selection: rgb(0xdcdce3),
        border_subtle: rgb(0xd4d4da),
        foreground: rgb(0x1c1c22),
        muted: rgb(0x5c5c68),
        subtle: rgb(0x8a8a96),
        success: rgb(0x2f9e5b),
        danger: rgb(0xc23b3b),
        warning: rgb(0xb07920),
        accent: rgb(0x3b6fd4),
        diff_added: rgb(0x2f9e5b),
        diff_added_bg: rgba(0xd8f0e0ff),
        diff_deleted: rgb(0xc23b3b),
        diff_deleted_bg: rgba(0xf5d8daff),
        diff_hunk_bg: rgba(0xe8ecf5ff),
        gutter: rgb(0xeeeef1),
        indent_guide: rgba(0x9a9aa855),
        folder: rgb(0x6a6a88),
        git_modified: rgb(0xa07830),
        git_added: rgb(0x2f9e5b),
        git_deleted: rgb(0xc23b3b),
    }
}

fn moss_dark() -> Theme {
    Theme {
        background: rgb(0x121916),
        terminal: rgb(0x121916),
        titlebar: rgb(0x121916),
        sidebar: rgb(0x16201b),
        panel: rgb(0x1a2620),
        elevated: rgb(0x1f2d26),
        hover: rgb(0x25362e),
        selection: rgb(0x2c4036),
        border_subtle: rgb(0x2a3a32),
        foreground: rgb(0xd6e6dc),
        muted: rgb(0x8eaa98),
        subtle: rgb(0x6a8474),
        success: rgb(0x5ecf8a),
        danger: rgb(0xe07a72),
        warning: rgb(0xd4b05a),
        accent: rgb(0x5dce98),
        diff_added: rgb(0x5ecf8a),
        diff_added_bg: rgba(0x1a3326ff),
        diff_deleted: rgb(0xe07a72),
        diff_deleted_bg: rgba(0x3a2220ff),
        diff_hunk_bg: rgba(0x16261fff),
        gutter: rgb(0x141c18),
        indent_guide: rgba(0x3a4a4255),
        folder: rgb(0x7a9a88),
        git_modified: rgb(0xd4b05a),
        git_added: rgb(0x5ecf8a),
        git_deleted: rgb(0xe07a72),
    }
}

fn moss_light() -> Theme {
    Theme {
        background: rgb(0xf1f7f3),
        terminal: rgb(0xf7fbf8),
        titlebar: rgb(0xf1f7f3),
        sidebar: rgb(0xe4efe8),
        panel: rgb(0xffffff),
        elevated: rgb(0xffffff),
        hover: rgb(0xdcebe2),
        selection: rgb(0xcfe0d5),
        border_subtle: rgb(0xc5d6cb),
        foreground: rgb(0x1a2b22),
        muted: rgb(0x4a6656),
        subtle: rgb(0x789486),
        success: rgb(0x1f8a4c),
        danger: rgb(0xb83a34),
        warning: rgb(0x9a6f18),
        accent: rgb(0x1f8a54),
        diff_added: rgb(0x1f8a4c),
        diff_added_bg: rgba(0xd2efdcff),
        diff_deleted: rgb(0xb83a34),
        diff_deleted_bg: rgba(0xf3d6d4ff),
        diff_hunk_bg: rgba(0xdfece4ff),
        gutter: rgb(0xeaf3ed),
        indent_guide: rgba(0x7a9a8855),
        folder: rgb(0x4a7a60),
        git_modified: rgb(0x9a6f18),
        git_added: rgb(0x1f8a4c),
        git_deleted: rgb(0xb83a34),
    }
}

fn harbor_dark() -> Theme {
    Theme {
        background: rgb(0x11161d),
        terminal: rgb(0x11161d),
        titlebar: rgb(0x11161d),
        sidebar: rgb(0x151b24),
        panel: rgb(0x1a212c),
        elevated: rgb(0x1f2734),
        hover: rgb(0x253040),
        selection: rgb(0x2c3a4d),
        border_subtle: rgb(0x2a3442),
        foreground: rgb(0xd5deea),
        muted: rgb(0x8ea0b8),
        subtle: rgb(0x6a7c94),
        success: rgb(0x55c48a),
        danger: rgb(0xe0727a),
        warning: rgb(0xd4a85a),
        accent: rgb(0x5ea8ef),
        diff_added: rgb(0x55c48a),
        diff_added_bg: rgba(0x173328ff),
        diff_deleted: rgb(0xe0727a),
        diff_deleted_bg: rgba(0x3a1e24ff),
        diff_hunk_bg: rgba(0x161e2aff),
        gutter: rgb(0x131820),
        indent_guide: rgba(0x3a4a5a55),
        folder: rgb(0x7a90aa),
        git_modified: rgb(0xd4a85a),
        git_added: rgb(0x55c48a),
        git_deleted: rgb(0xe0727a),
    }
}

fn harbor_light() -> Theme {
    Theme {
        background: rgb(0xf0f4f9),
        terminal: rgb(0xf7fafc),
        titlebar: rgb(0xf0f4f9),
        sidebar: rgb(0xe2eaf3),
        panel: rgb(0xffffff),
        elevated: rgb(0xffffff),
        hover: rgb(0xd8e3ef),
        selection: rgb(0xc9d8ea),
        border_subtle: rgb(0xc0cfde),
        foreground: rgb(0x182230),
        muted: rgb(0x4a5c74),
        subtle: rgb(0x7a8ca4),
        success: rgb(0x1f8a54),
        danger: rgb(0xb83a48),
        warning: rgb(0x9a6f18),
        accent: rgb(0x2563b8),
        diff_added: rgb(0x1f8a54),
        diff_added_bg: rgba(0xd0eddcff),
        diff_deleted: rgb(0xb83a48),
        diff_deleted_bg: rgba(0xf3d6daff),
        diff_hunk_bg: rgba(0xdfe8f4ff),
        gutter: rgb(0xe8eef5),
        indent_guide: rgba(0x7a90aa55),
        folder: rgb(0x4a6a90),
        git_modified: rgb(0x9a6f18),
        git_added: rgb(0x1f8a54),
        git_deleted: rgb(0xb83a48),
    }
}

fn cinder_dark() -> Theme {
    Theme {
        background: rgb(0x1a1412),
        terminal: rgb(0x1a1412),
        titlebar: rgb(0x1a1412),
        sidebar: rgb(0x201916),
        panel: rgb(0x261e1a),
        elevated: rgb(0x2c241f),
        hover: rgb(0x352c26),
        selection: rgb(0x40352d),
        border_subtle: rgb(0x3a302a),
        foreground: rgb(0xeadfd6),
        muted: rgb(0xb09a88),
        subtle: rgb(0x847466),
        success: rgb(0x6bcf8e),
        danger: rgb(0xe86a62),
        warning: rgb(0xe0a84a),
        accent: rgb(0xef8a52),
        diff_added: rgb(0x6bcf8e),
        diff_added_bg: rgba(0x1f3324ff),
        diff_deleted: rgb(0xe86a62),
        diff_deleted_bg: rgba(0x3a1e1cff),
        diff_hunk_bg: rgba(0x241a16ff),
        gutter: rgb(0x1c1614),
        indent_guide: rgba(0x4a3a3255),
        folder: rgb(0xa08878),
        git_modified: rgb(0xe0a84a),
        git_added: rgb(0x6bcf8e),
        git_deleted: rgb(0xe86a62),
    }
}

fn cinder_light() -> Theme {
    Theme {
        background: rgb(0xfaf4ef),
        terminal: rgb(0xfffaf6),
        titlebar: rgb(0xfaf4ef),
        sidebar: rgb(0xf2e6dc),
        panel: rgb(0xffffff),
        elevated: rgb(0xffffff),
        hover: rgb(0xeadcd0),
        selection: rgb(0xe0d0c0),
        border_subtle: rgb(0xd8c8b8),
        foreground: rgb(0x2a1e18),
        muted: rgb(0x6a5244),
        subtle: rgb(0x9a8070),
        success: rgb(0x2f8a4c),
        danger: rgb(0xc23b34),
        warning: rgb(0xa07018),
        accent: rgb(0xc4602f),
        diff_added: rgb(0x2f8a4c),
        diff_added_bg: rgba(0xd8efdcff),
        diff_deleted: rgb(0xc23b34),
        diff_deleted_bg: rgba(0xf5d6d4ff),
        diff_hunk_bg: rgba(0xf0e6dcff),
        gutter: rgb(0xf5ece4),
        indent_guide: rgba(0xa0887855),
        folder: rgb(0x8a6a50),
        git_modified: rgb(0xa07018),
        git_added: rgb(0x2f8a4c),
        git_deleted: rgb(0xc23b34),
    }
}

fn violet_dark() -> Theme {
    Theme {
        background: rgb(0x15121c),
        terminal: rgb(0x15121c),
        titlebar: rgb(0x15121c),
        sidebar: rgb(0x1a1624),
        panel: rgb(0x1f1a2a),
        elevated: rgb(0x252032),
        hover: rgb(0x2c273c),
        selection: rgb(0x352f48),
        border_subtle: rgb(0x322c40),
        foreground: rgb(0xe0d8f0),
        muted: rgb(0xa090c0),
        subtle: rgb(0x786c98),
        success: rgb(0x5ecf8a),
        danger: rgb(0xe07290),
        warning: rgb(0xd4a85a),
        accent: rgb(0xa88cf0),
        diff_added: rgb(0x5ecf8a),
        diff_added_bg: rgba(0x1a2e28ff),
        diff_deleted: rgb(0xe07290),
        diff_deleted_bg: rgba(0x3a1e2aff),
        diff_hunk_bg: rgba(0x1c1828ff),
        gutter: rgb(0x17131f),
        indent_guide: rgba(0x4a3a6055),
        folder: rgb(0x9080b0),
        git_modified: rgb(0xd4a85a),
        git_added: rgb(0x5ecf8a),
        git_deleted: rgb(0xe07290),
    }
}

fn violet_light() -> Theme {
    Theme {
        background: rgb(0xf5f2fb),
        terminal: rgb(0xfaf8fd),
        titlebar: rgb(0xf5f2fb),
        sidebar: rgb(0xeae4f5),
        panel: rgb(0xffffff),
        elevated: rgb(0xffffff),
        hover: rgb(0xe0d8f0),
        selection: rgb(0xd4c8ea),
        border_subtle: rgb(0xc8bce0),
        foreground: rgb(0x221a30),
        muted: rgb(0x5a4c78),
        subtle: rgb(0x8a7ca8),
        success: rgb(0x1f8a4c),
        danger: rgb(0xb83a58),
        warning: rgb(0x9a6f18),
        accent: rgb(0x6a48b8),
        diff_added: rgb(0x1f8a4c),
        diff_added_bg: rgba(0xd2efdcff),
        diff_deleted: rgb(0xb83a58),
        diff_deleted_bg: rgba(0xf3d6e0ff),
        diff_hunk_bg: rgba(0xe8e0f4ff),
        gutter: rgb(0xeee8f6),
        indent_guide: rgba(0x9080b055),
        folder: rgb(0x6a5890),
        git_modified: rgb(0x9a6f18),
        git_added: rgb(0x1f8a4c),
        git_deleted: rgb(0xb83a58),
    }
}

fn bloom_dark() -> Theme {
    Theme {
        background: rgb(0x1a141a),
        terminal: rgb(0x1a141a),
        titlebar: rgb(0x1a141a),
        sidebar: rgb(0x211820),
        panel: rgb(0x281e27),
        elevated: rgb(0x2f2430),
        hover: rgb(0x382c39),
        selection: rgb(0x443644),
        border_subtle: rgb(0x3a2e3a),
        foreground: rgb(0xf0e4ee),
        muted: rgb(0xb898b0),
        subtle: rgb(0x8a7088),
        success: rgb(0x5ecf8a),
        danger: rgb(0xf07090),
        warning: rgb(0xd4a85a),
        accent: rgb(0xdb5a9a),
        diff_added: rgb(0x5ecf8a),
        diff_added_bg: rgba(0x1a2e28ff),
        diff_deleted: rgb(0xf07090),
        diff_deleted_bg: rgba(0x3a1e28ff),
        diff_hunk_bg: rgba(0x221822ff),
        gutter: rgb(0x1c161c),
        indent_guide: rgba(0x4a3a4a55),
        folder: rgb(0xa080a0),
        git_modified: rgb(0xd4a85a),
        git_added: rgb(0x5ecf8a),
        git_deleted: rgb(0xf07090),
    }
}

fn bloom_light() -> Theme {
    Theme {
        background: rgb(0xfbf4f9),
        terminal: rgb(0xfef8fc),
        titlebar: rgb(0xfbf4f9),
        sidebar: rgb(0xf3e4ef),
        panel: rgb(0xffffff),
        elevated: rgb(0xffffff),
        hover: rgb(0xead6e4),
        selection: rgb(0xe0c8da),
        border_subtle: rgb(0xd8bcd0),
        foreground: rgb(0x3a1840),
        muted: rgb(0x7a4068),
        subtle: rgb(0xa07090),
        success: rgb(0x1f8a4c),
        danger: rgb(0xc23060),
        warning: rgb(0x9a6f18),
        accent: rgb(0xc02670),
        diff_added: rgb(0x1f8a4c),
        diff_added_bg: rgba(0xd2efdcff),
        diff_deleted: rgb(0xc23060),
        diff_deleted_bg: rgba(0xf5d6e4ff),
        diff_hunk_bg: rgba(0xf2e0ecff),
        gutter: rgb(0xf6eaf2),
        indent_guide: rgba(0xa080a055),
        folder: rgb(0x8a5080),
        git_modified: rgb(0x9a6f18),
        git_added: rgb(0x1f8a4c),
        git_deleted: rgb(0xc23060),
    }
}

fn build_catalog() -> [ThemeFamily; 6] {
    [
        ThemeFamily {
            id: "midnight",
            label: "Midnight",
            light: midnight_light(),
            dark: midnight_dark(),
        },
        ThemeFamily {
            id: "moss",
            label: "Moss",
            light: moss_light(),
            dark: moss_dark(),
        },
        ThemeFamily {
            id: "harbor",
            label: "Harbor",
            light: harbor_light(),
            dark: harbor_dark(),
        },
        ThemeFamily {
            id: "cinder",
            label: "Cinder",
            light: cinder_light(),
            dark: cinder_dark(),
        },
        ThemeFamily {
            id: "violet",
            label: "Violet",
            light: violet_light(),
            dark: violet_dark(),
        },
        ThemeFamily {
            id: "bloom",
            label: "Bloom",
            light: bloom_light(),
            dark: bloom_dark(),
        },
    ]
}

static CATALOG: LazyLock<[ThemeFamily; 6]> = LazyLock::new(build_catalog);

/// Built-in dual-mode palettes shown in Settings.
pub fn built_in_themes() -> &'static [ThemeFamily] {
    CATALOG.as_slice()
}

fn family_by_id(id: &str) -> ThemeFamily {
    built_in_themes()
        .iter()
        .copied()
        .find(|family| family.id == id)
        .unwrap_or(built_in_themes()[0])
}

pub fn canonicalize_theme_id(id: &str) -> &'static str {
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
pub fn colors() -> Theme {
    *active_slot()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn set_active(theme: Theme) {
    *active_slot()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = theme;
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
    fn catalog_ids_are_unique_and_default_resolves() {
        let mut seen = std::collections::HashSet::new();
        for family in built_in_themes() {
            assert!(seen.insert(family.id), "duplicate theme id {}", family.id);
            assert!(!family.label.is_empty());
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
    }

    #[test]
    fn active_palette_round_trips() {
        let before = colors();
        set_active(moss_dark());
        assert_eq!(colors().accent, moss_dark().accent);
        set_active(before);
    }
}
