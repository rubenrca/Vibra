//! App color roles and built-in palettes.
//!
//! UI code reads the active palette through [`colors`]. Preference resolution
//! (theme id + light/dark/system) lives in [`resolve`] / [`apply_preference`].

use std::sync::{LazyLock, OnceLock, RwLock};

use gpui::{Rgba, WindowAppearance, rgb, rgba};

use crate::ports::terminal::TerminalRgb;

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

/// Default PTY colors derived from a [`Theme`] so the grid matches chrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalPalette {
    pub background: TerminalRgb,
    pub foreground: TerminalRgb,
    pub cursor: TerminalRgb,
    pub selection: TerminalRgb,
    pub selection_foreground: TerminalRgb,
    pub ansi: [TerminalRgb; 16],
}

impl Theme {
    pub fn is_dark(self) -> bool {
        relative_luminance(self.background) < 0.5
    }

    pub fn overlay(self) -> Rgba {
        if self.is_dark() {
            rgba(0x08080acc)
        } else {
            rgba(0x1a1a2288)
        }
    }

    pub fn scrollbar_thumb(self) -> Rgba {
        let mut color = self.muted;
        color.a = if self.is_dark() { 0.45 } else { 0.40 };
        color
    }

    pub fn terminal_palette(self) -> TerminalPalette {
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
            selection: to_terminal_rgb(self.selection),
            selection_foreground: to_terminal_rgb(self.foreground),
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

fn relative_luminance(color: Rgba) -> f32 {
    0.2126 * color.r + 0.7152 * color.g + 0.0722 * color.b
}

fn with_alpha(mut color: Rgba, alpha: f32) -> Rgba {
    color.a = alpha.clamp(0.0, 1.0);
    color
}

/// Compact terminal/UI seed. Chrome surfaces are derived so a new palette
/// stays consistent without listing every role by hand.
#[derive(Debug, Clone, Copy)]
struct ThemeSeed {
    background: u32,
    foreground: u32,
    accent: u32,
    danger: u32,
    success: u32,
    warning: u32,
}

const fn seed(
    background: u32,
    foreground: u32,
    accent: u32,
    danger: u32,
    success: u32,
    warning: u32,
) -> ThemeSeed {
    ThemeSeed {
        background,
        foreground,
        accent,
        danger,
        success,
        warning,
    }
}

fn theme_from_seed(spec: ThemeSeed) -> Theme {
    let bg = rgb(spec.background);
    let fg = rgb(spec.foreground);
    let accent = rgb(spec.accent);
    let danger = rgb(spec.danger);
    let success = rgb(spec.success);
    let warning = rgb(spec.warning);
    let dark = relative_luminance(bg) < 0.5;
    let surface = |amount: f32| mix(bg, fg, amount);
    let sidebar = surface(if dark { 0.035 } else { 0.07 });
    let panel = if dark {
        surface(0.075)
    } else {
        mix(bg, rgb(0xffffff), 0.55)
    };
    let elevated = if dark {
        surface(0.11)
    } else {
        mix(bg, rgb(0xffffff), 0.8)
    };
    let hover = surface(if dark { 0.14 } else { 0.10 });
    let selection = mix(bg, accent, if dark { 0.22 } else { 0.14 });
    let border = surface(if dark { 0.16 } else { 0.18 });
    let muted = mix(fg, bg, 0.32);
    let subtle = mix(fg, bg, 0.52);
    let terminal = if dark {
        bg
    } else {
        mix(bg, rgb(0xffffff), 0.32)
    };
    let gutter = surface(0.02);
    let folder = mix(accent, muted, 0.4);
    let diff_added_bg = mix(bg, success, if dark { 0.22 } else { 0.16 });
    let diff_deleted_bg = mix(bg, danger, if dark { 0.22 } else { 0.16 });
    let diff_hunk_bg = mix(bg, accent, if dark { 0.14 } else { 0.10 });
    Theme {
        background: bg,
        terminal,
        titlebar: bg,
        sidebar,
        panel,
        elevated,
        hover,
        selection,
        border_subtle: border,
        foreground: fg,
        muted,
        subtle,
        success,
        danger,
        warning,
        accent,
        diff_added: success,
        diff_added_bg,
        diff_deleted: danger,
        diff_deleted_bg,
        diff_hunk_bg,
        gutter,
        indent_guide: with_alpha(mix(fg, bg, 0.55), 0.33),
        folder,
        git_modified: warning,
        git_added: success,
        git_deleted: danger,
    }
}

fn family(id: &'static str, label: &'static str, light: ThemeSeed, dark: ThemeSeed) -> ThemeFamily {
    ThemeFamily {
        id,
        label,
        light: theme_from_seed(light),
        dark: theme_from_seed(dark),
    }
}

pub fn to_terminal_rgb(color: Rgba) -> TerminalRgb {
    TerminalRgb::new(
        (color.r * 255.0).round().clamp(0.0, 255.0) as u8,
        (color.g * 255.0).round().clamp(0.0, 255.0) as u8,
        (color.b * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

pub fn terminal_palette() -> TerminalPalette {
    colors().terminal_palette()
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

const DEFAULT_THEME_ID: &str = "midnight";

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

fn build_catalog() -> Vec<ThemeFamily> {
    let mut catalog = vec![
        ThemeFamily {
            id: DEFAULT_THEME_ID,
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
    ];
    // Popular palettes, color-mapped from the Warp themes repo (Apache-2.0)
    // and their upstream schemes so the picker feels like a real terminal.
    catalog.extend([
        family(
            "nord",
            "Nord",
            seed(0xeceff4, 0x2e3440, 0x5e81ac, 0xbf616a, 0xa3be8c, 0xd08770),
            seed(0x2e3440, 0xd8dee9, 0x81a1c1, 0xbf616a, 0xa3be8c, 0xebcb8b),
        ),
        family(
            "gruvbox",
            "Gruvbox",
            seed(0xfbf1c7, 0x3c3836, 0xaf3a03, 0x9d0006, 0x79740e, 0xb57614),
            seed(0x282828, 0xebdbb2, 0xfe8019, 0xfb4934, 0xb8bb26, 0xfabd2f),
        ),
        family(
            "solarized",
            "Solarized",
            seed(0xfdf6e3, 0x586e75, 0x268bd2, 0xdc322f, 0x859900, 0xb58900),
            seed(0x002b36, 0x839496, 0x268bd2, 0xdc322f, 0x859900, 0xb58900),
        ),
        family(
            "dracula",
            "Dracula",
            seed(0xf8f8f2, 0x282a36, 0x6272a4, 0xc41e3a, 0x2e7d32, 0x9a7b0a),
            seed(0x282a36, 0xf8f8f2, 0xff79c6, 0xff5555, 0x50fa7b, 0xf1fa8c),
        ),
        family(
            "catppuccin",
            "Catppuccin",
            seed(0xeff1f5, 0x4c4f69, 0x1e66f5, 0xd20f39, 0x40a02b, 0xdf8e1d),
            seed(0x1e1e2e, 0xcdd6f4, 0x89b4fa, 0xf38ba8, 0xa6e3a1, 0xf9e2af),
        ),
        family(
            "tokyo",
            "Tokyo Night",
            seed(0xe1e2e7, 0x3760bf, 0x2e7de9, 0xf52a65, 0x587539, 0x8c6c3e),
            seed(0x1a1b26, 0xc0caf5, 0x7aa2f7, 0xf7768e, 0x9ece6a, 0xe0af68),
        ),
        family(
            "one",
            "One Dark",
            seed(0xfafafa, 0x383a42, 0x4078f2, 0xe45649, 0x50a14f, 0xc18401),
            seed(0x282c34, 0xabb2bf, 0x61afef, 0xe06c75, 0x98c379, 0xe5c07b),
        ),
        family(
            "github",
            "GitHub",
            seed(0xffffff, 0x1f2328, 0x0969da, 0xcf222e, 0x1a7f37, 0x9a6700),
            seed(0x0d1117, 0xe6edf3, 0x58a6ff, 0xff7b72, 0x3fb950, 0xd29922),
        ),
        family(
            "ayu",
            "Ayu",
            seed(0xf8f9fa, 0x5c6166, 0xffaa33, 0xf07171, 0x86b300, 0xf2ae49),
            seed(0x0a0e14, 0xb3b1ad, 0x53bdfa, 0xf07178, 0xc2d94c, 0xffb454),
        ),
        family(
            "everforest",
            "Everforest",
            seed(0xfffbef, 0x5c6a72, 0x3a94c5, 0xf85552, 0x8da101, 0xdfa000),
            seed(0x2b3339, 0xd3c6aa, 0x7fbbb3, 0xe67e80, 0xa7c080, 0xdbbc7f),
        ),
        family(
            "kanagawa",
            "Kanagawa",
            seed(0xf2ecbc, 0x545464, 0x4d699b, 0xc84053, 0x6f894e, 0x77713f),
            seed(0x1f1f28, 0xdcd7ba, 0x7e9cd8, 0xc34043, 0x76946a, 0xe6c384),
        ),
        family(
            "rosepine",
            "Rosé Pine",
            seed(0xfaf4ed, 0x575279, 0x907aa9, 0xb4637a, 0x286983, 0xea9d34),
            seed(0x191724, 0xe0def4, 0xc4a7e7, 0xeb6f92, 0x31748f, 0xf6c177),
        ),
        family(
            "monokai",
            "Monokai",
            seed(0xfaf4ed, 0x2d2a2e, 0xab9df2, 0xe14775, 0x4d7c0f, 0xb45309),
            seed(0x2d2a2e, 0xfcfcfa, 0xab9df2, 0xff6188, 0xa9dc76, 0xffd866),
        ),
        family(
            "warp",
            "Warp",
            seed(0xffffff, 0x111111, 0x008ec4, 0xc30771, 0x10a778, 0xa89c14),
            seed(0x0b0d10, 0xf1f1f1, 0x00c2ff, 0xff8272, 0xb4fa72, 0xfefdc2),
        ),
    ]);
    catalog
}

static CATALOG: LazyLock<Vec<ThemeFamily>> = LazyLock::new(build_catalog);

/// Built-in dual-mode palettes shown in Settings.
pub fn built_in_themes() -> &'static [ThemeFamily] {
    CATALOG.as_slice()
}

pub fn is_known_theme_id(id: &str) -> bool {
    built_in_themes().iter().any(|family| family.id == id)
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
    theme_generation().fetch_add(1, std::sync::atomic::Ordering::Release);
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
        assert!(built_in_themes().len() >= 20);
        assert!(is_known_theme_id("nord"));
        assert!(is_known_theme_id("gruvbox"));
        assert!(is_known_theme_id("catppuccin"));
        assert!(is_known_theme_id("tokyo"));
        assert!(is_known_theme_id("warp"));
        assert!(resolve("nord", AppearanceMode::Dark, true).is_dark());
        assert!(!resolve("nord", AppearanceMode::Light, false).is_dark());
        assert!(resolve("gruvbox", AppearanceMode::Dark, true).is_dark());
        assert!(!resolve("solarized", AppearanceMode::Light, false).is_dark());
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
