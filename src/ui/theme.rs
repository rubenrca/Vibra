//! App color roles and built-in palettes.
//!
//! UI code reads the active palette through [`colors`]. Preference resolution
//! (theme id + light/dark/system) lives in [`resolve`] / [`apply_preference`].
//! User Warp YAML / Ghostty files load from `~/.vibra/themes`.

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, OnceLock, RwLock};

use gpui::{Rgba, WindowAppearance, rgb, rgba};
use serde::{Deserialize, Serialize};

use crate::ports::terminal::TerminalRgb;
use crate::ui::theme_import::{self, ImportedScheme};

/// Family name of the bundled JetBrains Mono Variable font.
pub const MONO_FONT: &str = "JetBrains Mono";

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

/// Default PTY colors derived from a [`Theme`] so the grid matches chrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalPalette {
    pub background: TerminalRgb,
    pub foreground: TerminalRgb,
    pub cursor: TerminalRgb,
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
    cursor: Option<u32>,
    ansi: Option<[u32; 16]>,
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
        cursor: None,
        ansi: None,
    }
}

#[allow(clippy::too_many_arguments)]
const fn ansi(
    n0: u32,
    n1: u32,
    n2: u32,
    n3: u32,
    n4: u32,
    n5: u32,
    n6: u32,
    n7: u32,
    b0: u32,
    b1: u32,
    b2: u32,
    b3: u32,
    b4: u32,
    b5: u32,
    b6: u32,
    b7: u32,
) -> [u32; 16] {
    [
        n0, n1, n2, n3, n4, n5, n6, n7, b0, b1, b2, b3, b4, b5, b6, b7,
    ]
}

impl ThemeSeed {
    fn with_terminal(self, ansi: [u32; 16]) -> Self {
        Self {
            ansi: Some(ansi),
            ..self
        }
    }

    fn with_cursor(self, cursor: u32) -> Self {
        Self {
            cursor: Some(cursor),
            ..self
        }
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
    let authentic = spec.ansi.is_some();
    let terminal = if authentic || dark {
        bg
    } else {
        mix(bg, rgb(0xffffff), 0.32)
    };
    let gutter = surface(0.02);
    let folder = mix(accent, muted, 0.4);
    let diff_added_bg = mix(bg, success, if dark { 0.22 } else { 0.16 });
    let diff_deleted_bg = mix(bg, danger, if dark { 0.22 } else { 0.16 });
    let diff_hunk_bg = mix(bg, accent, if dark { 0.14 } else { 0.10 });
    let mut theme = Theme {
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
        terminal_colors: None,
    };
    if let Some(ansi) = spec.ansi {
        let mut palette = theme.synthesize_terminal_palette();
        palette.background = rgb_u32_to_terminal(spec.background);
        palette.foreground = rgb_u32_to_terminal(spec.foreground);
        palette.cursor = rgb_u32_to_terminal(spec.cursor.unwrap_or(spec.foreground));
        for (index, color) in ansi.into_iter().enumerate() {
            palette.ansi[index] = rgb_u32_to_terminal(color);
        }
        theme.terminal_colors = Some(palette);
    }
    theme
}

fn family(id: &'static str, label: &'static str, light: ThemeSeed, dark: ThemeSeed) -> ThemeFamily {
    ThemeFamily {
        id: id.to_string(),
        label: label.to_string(),
        light: theme_from_seed(light),
        dark: theme_from_seed(dark),
    }
}

fn rgb_u32_to_terminal(color: u32) -> TerminalRgb {
    TerminalRgb::new(
        ((color >> 16) & 0xff) as u8,
        ((color >> 8) & 0xff) as u8,
        (color & 0xff) as u8,
    )
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppearanceMode {
    Light,
    Dark,
    #[default]
    #[serde(other)]
    System,
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
        terminal_colors: None,
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
        terminal_colors: None,
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
        terminal_colors: None,
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
        terminal_colors: None,
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
        terminal_colors: None,
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
        terminal_colors: None,
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
        terminal_colors: None,
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
        terminal_colors: None,
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
        terminal_colors: None,
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
        terminal_colors: None,
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
        terminal_colors: None,
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
        terminal_colors: None,
    }
}

fn product_family(id: &'static str, label: &'static str, light: Theme, dark: Theme) -> ThemeFamily {
    ThemeFamily {
        id: id.to_string(),
        label: label.to_string(),
        light,
        dark,
    }
}

fn build_catalog() -> Vec<ThemeFamily> {
    let mut catalog = vec![
        product_family(
            DEFAULT_THEME_ID,
            "Midnight",
            midnight_light(),
            midnight_dark(),
        ),
        product_family("moss", "Moss", moss_light(), moss_dark()),
        product_family("harbor", "Harbor", harbor_light(), harbor_dark()),
        product_family("cinder", "Cinder", cinder_light(), cinder_dark()),
        product_family("violet", "Violet", violet_light(), violet_dark()),
        product_family("bloom", "Bloom", bloom_light(), bloom_dark()),
    ];
    // Popular palettes with authentic ANSI from upstream schemes / Warp YAML.
    catalog.extend([
        family(
            "nord",
            "Nord",
            seed(0xeceff4, 0x2e3440, 0x5e81ac, 0xbf616a, 0xa3be8c, 0xd08770).with_terminal(ansi(
                0xeceff4, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0x4c566a,
                0xd8dee9, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x8fbcbb, 0x2e3440,
            )),
            seed(0x2e3440, 0xd8dee9, 0x81a1c1, 0xbf616a, 0xa3be8c, 0xebcb8b).with_terminal(ansi(
                0x3b4252, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0xe5e9f0,
                0x4c566a, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x8fbcbb, 0xeceff4,
            )),
        ),
        family(
            "gruvbox",
            "Gruvbox",
            seed(0xfbf1c7, 0x3c3836, 0xaf3a03, 0x9d0006, 0x79740e, 0xb57614).with_terminal(ansi(
                0xfbf1c7, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0x7c6f64,
                0x928374, 0x9d0006, 0x79740e, 0xb57614, 0x076678, 0x8f3f71, 0x427b58, 0x3c3836,
            )),
            seed(0x282828, 0xebdbb2, 0xfe8019, 0xfb4934, 0xb8bb26, 0xfabd2f).with_terminal(ansi(
                0x282828, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0xa89984,
                0x928374, 0xfb4934, 0xb8bb26, 0xfabd2f, 0x83a598, 0xd3869b, 0x8ec07c, 0xebdbb2,
            )),
        ),
        family(
            "solarized",
            "Solarized",
            seed(0xfdf6e3, 0x657b83, 0x268bd2, 0xdc322f, 0x859900, 0xb58900).with_terminal(ansi(
                0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5,
                0x002b36, 0xcb4b16, 0x586e75, 0x657b83, 0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3,
            )),
            seed(0x002b36, 0x839496, 0x268bd2, 0xdc322f, 0x859900, 0xb58900).with_terminal(ansi(
                0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5,
                0x002b36, 0xcb4b16, 0x586e75, 0x657b83, 0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3,
            )),
        ),
        family(
            "dracula",
            "Dracula",
            seed(0xf8f8f2, 0x282a36, 0x6272a4, 0xc41e3a, 0x2e7d32, 0x9a7b0a),
            seed(0x282a36, 0xf8f8f2, 0xbd93f9, 0xff5555, 0x50fa7b, 0xf1fa8c)
                .with_cursor(0xf8f8f2)
                .with_terminal(ansi(
                    0x21222c, 0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xf8f8f2,
                    0x6272a4, 0xff6e6e, 0x69ff94, 0xffffa5, 0xd6acff, 0xff92d0, 0xa4ffff, 0xffffff,
                )),
        ),
        family(
            "catppuccin",
            "Catppuccin",
            seed(0xeff1f5, 0x4c4f69, 0x1e66f5, 0xd20f39, 0x40a02b, 0xdf8e1d).with_terminal(ansi(
                0x5c5f77, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xacb0be,
                0x6c6f85, 0xde293e, 0x49af3d, 0xeea02d, 0x456eff, 0xfe85d8, 0x2d9fa8, 0xbcc0cc,
            )),
            seed(0x1e1e2e, 0xcdd6f4, 0x89b4fa, 0xf38ba8, 0xa6e3a1, 0xf9e2af).with_terminal(ansi(
                0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8,
                0x585b70, 0xf37799, 0x89d88b, 0xebd391, 0x74a8fc, 0xf2aede, 0x6bd7ca, 0xbac2de,
            )),
        ),
        family(
            "tokyo",
            "Tokyo Night",
            seed(0xe1e2e7, 0x3760bf, 0x2e7de9, 0xf52a65, 0x587539, 0x8c6c3e)
                .with_cursor(0x3760bf)
                .with_terminal(ansi(
                    0xb4b5b9, 0xf52a65, 0x587539, 0x8c6c3e, 0x2e7de9, 0x9854f1, 0x007197, 0x6172b0,
                    0xa1a6c5, 0xff4774, 0x5c8524, 0xa27629, 0x358aff, 0xa463ff, 0x007ea8, 0x3760bf,
                )),
            seed(0x1a1b26, 0xc0caf5, 0x7aa2f7, 0xf7768e, 0x9ece6a, 0xe0af68)
                .with_cursor(0xc0caf5)
                .with_terminal(ansi(
                    0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6,
                    0x414868, 0xff899d, 0x9fe044, 0xfaba4a, 0x8db0ff, 0xc7a9ff, 0xa4daff, 0xc0caf5,
                )),
        ),
        family(
            "one",
            "One Dark",
            seed(0xfafafa, 0x383a42, 0x4078f2, 0xe45649, 0x50a14f, 0xc18401).with_terminal(ansi(
                0x383a42, 0xe45649, 0x50a14f, 0xc18401, 0x4078f2, 0xa626a4, 0x0184bc, 0xa0a1a7,
                0x696c77, 0xe45649, 0x50a14f, 0xc18401, 0x4078f2, 0xa626a4, 0x0184bc, 0x383a42,
            )),
            seed(0x282c34, 0xabb2bf, 0x61afef, 0xe06c75, 0x98c379, 0xe5c07b).with_terminal(ansi(
                0x1e2127, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf,
                0x5c6370, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xffffff,
            )),
        ),
        family(
            "github",
            "GitHub",
            seed(0xffffff, 0x1f2328, 0x0969da, 0xcf222e, 0x1a7f37, 0x9a6700).with_terminal(ansi(
                0x24292f, 0xcf222e, 0x116329, 0x4d2d00, 0x0969da, 0x8250df, 0x1b7c83, 0x6e7781,
                0x57606a, 0xa40e26, 0x1a7f37, 0x633c01, 0x218bff, 0xa475f9, 0x3192aa, 0x8c959f,
            )),
            seed(0x0d1117, 0xe6edf3, 0x58a6ff, 0xff7b72, 0x3fb950, 0xd29922).with_terminal(ansi(
                0x484f58, 0xff7b72, 0x3fb950, 0xd29922, 0x58a6ff, 0xbc8cff, 0x39c5cf, 0xb1bac4,
                0x6e7681, 0xffa198, 0x56d364, 0xe3b341, 0x79c0ff, 0xd2a8ff, 0x56d4dd, 0xffffff,
            )),
        ),
        family(
            "ayu",
            "Ayu",
            seed(0xf8f9fa, 0x5c6166, 0xffaa33, 0xf07171, 0x86b300, 0xf2ae49).with_terminal(ansi(
                0x000000, 0xf07171, 0x86b300, 0xf2ae49, 0x399ee6, 0xa37acc, 0x4cbf99, 0xabb0b6,
                0x828a8f, 0xf51818, 0x86b300, 0xffaa33, 0x478acc, 0xff7383, 0x55b4d4, 0x5c6166,
            )),
            seed(0x0a0e14, 0xb3b1ad, 0x53bdfa, 0xf07178, 0xc2d94c, 0xffb454).with_terminal(ansi(
                0x01060e, 0xea6c73, 0x91b362, 0xf9af4f, 0x53bdfa, 0xfae994, 0x90e1c6, 0xc7c7c7,
                0x686868, 0xf07178, 0xc2d94c, 0xffb454, 0x59c2ff, 0xffee99, 0x95e6cb, 0xffffff,
            )),
        ),
        family(
            "everforest",
            "Everforest",
            seed(0xfffbef, 0x5c6a72, 0x3a94c5, 0xf85552, 0x8da101, 0xdfa000).with_terminal(ansi(
                0x5c6a72, 0xf85552, 0x8da101, 0xdfa000, 0x3a94c5, 0xdf69ba, 0x35a77c, 0xdfddc8,
                0x939f91, 0xe67e80, 0xa7c080, 0xdbbc7f, 0x7fbbb3, 0xd699b6, 0x83c092, 0x5c6a72,
            )),
            seed(0x2b3339, 0xd3c6aa, 0x7fbbb3, 0xe67e80, 0xa7c080, 0xdbbc7f).with_terminal(ansi(
                0x4b565c, 0xe67e80, 0xa7c080, 0xdbbc7f, 0x7fbbb3, 0xd699b6, 0x83c092, 0xd3c6aa,
                0x4b565c, 0xe67e80, 0xa7c080, 0xdbbc7f, 0x7fbbb3, 0xd699b6, 0x83c092, 0xd3c6aa,
            )),
        ),
        family(
            "kanagawa",
            "Kanagawa",
            seed(0xf2ecbc, 0x545464, 0x4d699b, 0xc84053, 0x6f894e, 0x77713f).with_terminal(ansi(
                0xe7dba0, 0xc84053, 0x6f894e, 0x77713f, 0x4d699b, 0xb35b79, 0x597b75, 0x545464,
                0x8a8980, 0xd7474b, 0x6e915f, 0x83640a, 0x6693bf, 0x624c83, 0x5e857a, 0x43436c,
            )),
            seed(0x1f1f28, 0xdcd7ba, 0x7e9cd8, 0xc34043, 0x76946a, 0xe6c384).with_terminal(ansi(
                0x090618, 0xc34043, 0x76946a, 0xc0a36e, 0x7e9cd8, 0x957fb8, 0x6a9589, 0xc8c093,
                0x727169, 0xe82424, 0x98bb6c, 0xe6c384, 0x7fb4ca, 0x938aa9, 0x7aa89f, 0xdcd7ba,
            )),
        ),
        family(
            "rosepine",
            "Rosé Pine",
            seed(0xfaf4ed, 0x575279, 0x907aa9, 0xb4637a, 0x286983, 0xea9d34).with_terminal(ansi(
                0xf2e9e1, 0xb4637a, 0x286983, 0xea9d34, 0x56949f, 0x907aa9, 0xd7827e, 0x575279,
                0x9893a5, 0xb4637a, 0x286983, 0xea9d34, 0x56949f, 0x907aa9, 0xd7827e, 0x575279,
            )),
            seed(0x191724, 0xe0def4, 0xc4a7e7, 0xeb6f92, 0x31748f, 0xf6c177).with_terminal(ansi(
                0x26233a, 0xeb6f92, 0x31748f, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xebbcba, 0xe0def4,
                0x6e6a86, 0xeb6f92, 0x31748f, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xebbcba, 0xe0def4,
            )),
        ),
        family(
            "monokai",
            "Monokai",
            seed(0xfaf4ed, 0x2d2a2e, 0xab9df2, 0xe14775, 0x4d7c0f, 0xb45309),
            seed(0x2d2a2e, 0xfcfcfa, 0xab9df2, 0xff6188, 0xa9dc76, 0xffd866).with_terminal(ansi(
                0x2d2a2e, 0xff6188, 0xa9dc76, 0xffd866, 0xfc9867, 0xab9df2, 0x78dce8, 0xfcfcfa,
                0x727072, 0xff6188, 0xa9dc76, 0xffd866, 0xfc9867, 0xab9df2, 0x78dce8, 0xfcfcfa,
            )),
        ),
        family(
            "warp",
            "Warp",
            seed(0xffffff, 0x111111, 0x008ec4, 0xc30771, 0x10a778, 0xa89c14).with_terminal(ansi(
                0x111111, 0xc30771, 0x10a778, 0xa89c14, 0x008ec4, 0x523c79, 0x20a5ba, 0xe0e0e0,
                0x777777, 0xfb007a, 0x98e024, 0xfff114, 0x00a6fb, 0x9b37ff, 0x4de8e8, 0xffffff,
            )),
            seed(0x0b0d10, 0xf1f1f1, 0x00c2ff, 0xff8272, 0xb4fa72, 0xfefdc2).with_terminal(ansi(
                0x616161, 0xff8272, 0xb4fa72, 0xfefdc2, 0xa5d5fe, 0xff8ffd, 0xd0d1fe, 0xf1f1f1,
                0x8e8e8e, 0xffc4bd, 0xd6fcb9, 0xfefdd5, 0xc1e3fe, 0xffb1fe, 0xe5e6fe, 0xfeffff,
            )),
        ),
        family(
            "catppuccin-frappe",
            "Catppuccin Frappé",
            seed(0xeff1f5, 0x4c4f69, 0x1e66f5, 0xd20f39, 0x40a02b, 0xdf8e1d).with_terminal(ansi(
                0x5c5f77, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xacb0be,
                0x6c6f85, 0xde293e, 0x49af3d, 0xeea02d, 0x456eff, 0xfe85d8, 0x2d9fa8, 0xbcc0cc,
            )),
            seed(0x303446, 0xc6d0f5, 0x8caaee, 0xe78284, 0xa6d189, 0xe5c890).with_terminal(ansi(
                0x51576d, 0xe78284, 0xa6d189, 0xe5c890, 0x8caaee, 0xf4b8e4, 0x81c8be, 0xa5adce,
                0x626880, 0xe67172, 0x8ec772, 0xd9ba73, 0x7b9ef0, 0xf2a4db, 0x5abfb5, 0xb5bfe2,
            )),
        ),
        family(
            "catppuccin-macchiato",
            "Catppuccin Macchiato",
            seed(0xeff1f5, 0x4c4f69, 0x1e66f5, 0xd20f39, 0x40a02b, 0xdf8e1d).with_terminal(ansi(
                0x5c5f77, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xacb0be,
                0x6c6f85, 0xde293e, 0x49af3d, 0xeea02d, 0x456eff, 0xfe85d8, 0x2d9fa8, 0xbcc0cc,
            )),
            seed(0x24273a, 0xcad3f5, 0x8aadf4, 0xed8796, 0xa6da95, 0xeed49f).with_terminal(ansi(
                0x494d64, 0xed8796, 0xa6da95, 0xeed49f, 0x8aadf4, 0xf5bde6, 0x8bd5ca, 0xa5adcb,
                0x5b6078, 0xec7486, 0x8ccf7f, 0xe1c682, 0x78a1f6, 0xf2a9dd, 0x63cbc0, 0xb8c0e0,
            )),
        ),
        family(
            "tokyo-storm",
            "Tokyo Night Storm",
            seed(0xe1e2e7, 0x3760bf, 0x2e7de9, 0xf52a65, 0x587539, 0x8c6c3e)
                .with_cursor(0x3760bf)
                .with_terminal(ansi(
                    0xb4b5b9, 0xf52a65, 0x587539, 0x8c6c3e, 0x2e7de9, 0x9854f1, 0x007197, 0x6172b0,
                    0xa1a6c5, 0xff4774, 0x5c8524, 0xa27629, 0x358aff, 0xa463ff, 0x007ea8, 0x3760bf,
                )),
            seed(0x24283b, 0xc0caf5, 0x7aa2f7, 0xf7768e, 0x9ece6a, 0xe0af68)
                .with_cursor(0xc0caf5)
                .with_terminal(ansi(
                    0x1d202f, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6,
                    0x414868, 0xff899d, 0x9fe044, 0xfaba4a, 0x8db0ff, 0xc7a9ff, 0xa4daff, 0xc0caf5,
                )),
        ),
        family(
            "tokyo-moon",
            "Tokyo Night Moon",
            seed(0xe1e2e7, 0x3760bf, 0x2e7de9, 0xf52a65, 0x587539, 0x8c6c3e)
                .with_cursor(0x3760bf)
                .with_terminal(ansi(
                    0xb4b5b9, 0xf52a65, 0x587539, 0x8c6c3e, 0x2e7de9, 0x9854f1, 0x007197, 0x6172b0,
                    0xa1a6c5, 0xff4774, 0x5c8524, 0xa27629, 0x358aff, 0xa463ff, 0x007ea8, 0x3760bf,
                )),
            seed(0x222436, 0xc8d3f5, 0x82aaff, 0xff757f, 0xc3e88d, 0xffc777)
                .with_cursor(0xc8d3f5)
                .with_terminal(ansi(
                    0x1b1d2b, 0xff757f, 0xc3e88d, 0xffc777, 0x82aaff, 0xc099ff, 0x86e1fc, 0x828bb8,
                    0x444a73, 0xff8d94, 0xc7fb6d, 0xffd8ab, 0x9ab8ff, 0xcaabff, 0xb2ebff, 0xc8d3f5,
                )),
        ),
        family(
            "flexoki",
            "Flexoki",
            seed(0xfffcf0, 0x100f0f, 0x205ea6, 0xaf3029, 0x66800b, 0xad8301)
                .with_cursor(0x100f0f)
                .with_terminal(ansi(
                    0x100f0f, 0xaf3029, 0x66800b, 0xad8301, 0x205ea6, 0xa02f6f, 0x24837b, 0x6f6e69,
                    0xb7b5ac, 0xd14d41, 0x879a39, 0xd0a215, 0x4385be, 0xce5d97, 0x3aa99f, 0xcecdc3,
                )),
            seed(0x100f0f, 0xcecdc3, 0x4385be, 0xd14d41, 0x879a39, 0xd0a215)
                .with_cursor(0xcecdc3)
                .with_terminal(ansi(
                    0x100f0f, 0xaf3029, 0x66800b, 0xad8301, 0x205ea6, 0xa02f6f, 0x24837b, 0x878580,
                    0x6f6e69, 0xd14d41, 0x879a39, 0xd0a215, 0x4385be, 0xce5d97, 0x3aa99f, 0xcecdc3,
                )),
        ),
        family(
            "horizon",
            "Horizon",
            seed(0xfdf0ed, 0x1c1e26, 0x26bbd9, 0xe95678, 0x29d398, 0xfab795).with_terminal(ansi(
                0xf2e6e4, 0xe95678, 0x29d398, 0xfab795, 0x26bbd9, 0xee64ac, 0x59e1e3, 0x1c1e26,
                0xbdb3b1, 0xec6a88, 0x3fdaa4, 0xfbc3a7, 0x3fc4de, 0xf075b5, 0x6be4e6, 0x06060c,
            )),
            seed(0x1c1e26, 0xd5d8da, 0x26bbd9, 0xe95678, 0x29d398, 0xfab795).with_terminal(ansi(
                0x16161c, 0xe95678, 0x29d398, 0xfab795, 0x26bbd9, 0xee64ac, 0x59e1e3, 0xd5d8da,
                0x5b5858, 0xec6a88, 0x3fdaa4, 0xfbc3a7, 0x3fc4de, 0xf075b5, 0x6be4e6, 0xffffff,
            )),
        ),
        family(
            "oxocarbon",
            "Oxocarbon",
            seed(0xffffff, 0x161616, 0x0f62fe, 0xee5396, 0x42be65, 0xff7eb6).with_terminal(ansi(
                0xffffff, 0xee5396, 0x42be65, 0xff7eb6, 0x0f62fe, 0xbe95ff, 0x08bdba, 0x525252,
                0x161616, 0xee5396, 0x42be65, 0xff7eb6, 0x78a9ff, 0xbe95ff, 0x3ddbd9, 0x161616,
            )),
            seed(0x161616, 0xf2f4f8, 0x78a9ff, 0xee5396, 0x42be65, 0xffe97b).with_terminal(ansi(
                0x262626, 0xee5396, 0x42be65, 0xffe97b, 0x33b1ff, 0xbe95ff, 0x3ddbd9, 0xdde1e6,
                0x393939, 0xee5396, 0x42be65, 0xffe97b, 0x33b1ff, 0xbe95ff, 0x08bdba, 0xffffff,
            )),
        ),
        family(
            "kanagawa-dragon",
            "Kanagawa Dragon",
            seed(0xf2ecbc, 0x545464, 0x4d699b, 0xc84053, 0x6f894e, 0x77713f).with_terminal(ansi(
                0xe7dba0, 0xc84053, 0x6f894e, 0x77713f, 0x4d699b, 0xb35b79, 0x597b75, 0x545464,
                0x8a8980, 0xd7474b, 0x6e915f, 0x83640a, 0x6693bf, 0x624c83, 0x5e857a, 0x43436c,
            )),
            seed(0x181616, 0xc5c9c5, 0x8ba4b0, 0xc4746e, 0x87a987, 0xc4b28a).with_terminal(ansi(
                0x0d0c0c, 0xc4746e, 0x87a987, 0xc4b28a, 0x8ba4b0, 0xa292a3, 0x8ea4a2, 0xc5c9c5,
                0xa6a69c, 0xe46876, 0x87a987, 0xe6c384, 0x7fb4ca, 0x938aa9, 0x7aa89f, 0xc5c9c5,
            )),
        ),
        family(
            "rosepine-moon",
            "Rosé Pine Moon",
            seed(0xfaf4ed, 0x575279, 0x907aa9, 0xb4637a, 0x286983, 0xea9d34).with_terminal(ansi(
                0xf2e9e1, 0xb4637a, 0x286983, 0xea9d34, 0x56949f, 0x907aa9, 0xd7827e, 0x575279,
                0x9893a5, 0xb4637a, 0x286983, 0xea9d34, 0x56949f, 0x907aa9, 0xd7827e, 0x575279,
            )),
            seed(0x232136, 0xe0def4, 0xc4a7e7, 0xeb6f92, 0x3e8fb0, 0xf6c177).with_terminal(ansi(
                0x393552, 0xeb6f92, 0x3e8fb0, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xea9a97, 0xe0def4,
                0x6e6a86, 0xeb6f92, 0x3e8fb0, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xea9a97, 0xe0def4,
            )),
        ),
        family(
            "gruvbox-hard",
            "Gruvbox Hard",
            seed(0xf9f5d7, 0x3c3836, 0xaf3a03, 0x9d0006, 0x79740e, 0xb57614).with_terminal(ansi(
                0xf9f5d7, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0x7c6f64,
                0x928374, 0x9d0006, 0x79740e, 0xb57614, 0x076678, 0x8f3f71, 0x427b58, 0x3c3836,
            )),
            seed(0x1d2021, 0xebdbb2, 0xfe8019, 0xfb4934, 0xb8bb26, 0xfabd2f).with_terminal(ansi(
                0x1d2021, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0xa89984,
                0x928374, 0xfb4934, 0xb8bb26, 0xfabd2f, 0x83a598, 0xd3869b, 0x8ec07c, 0xebdbb2,
            )),
        ),
        family(
            "palenight",
            "Palenight",
            seed(0x292d3e, 0xa6accd, 0x82aaff, 0xff5370, 0xc3e88d, 0xffcb6b).with_terminal(ansi(
                0x292d3e, 0xf07178, 0xc3e88d, 0xffcb6b, 0x82aaff, 0xc792ea, 0x89ddff, 0xd0d0d0,
                0x676e95, 0xff5370, 0xc3e88d, 0xffcb6b, 0x82aaff, 0xc792ea, 0x89ddff, 0xffffff,
            )),
            seed(0x292d3e, 0xa6accd, 0x82aaff, 0xff5370, 0xc3e88d, 0xffcb6b).with_terminal(ansi(
                0x292d3e, 0xf07178, 0xc3e88d, 0xffcb6b, 0x82aaff, 0xc792ea, 0x89ddff, 0xd0d0d0,
                0x676e95, 0xff5370, 0xc3e88d, 0xffcb6b, 0x82aaff, 0xc792ea, 0x89ddff, 0xffffff,
            )),
        ),
        family(
            "vesper",
            "Vesper",
            seed(0x101010, 0xffffff, 0xffc799, 0xff8080, 0x99ffe4, 0xffc799).with_terminal(ansi(
                0x101010, 0xff8080, 0x99ffe4, 0xffc799, 0xaca1ff, 0xffc799, 0x99ffe4, 0xffffff,
                0xa0a0a0, 0xff8080, 0x99ffe4, 0xffc799, 0xaca1ff, 0xffc799, 0x99ffe4, 0xffffff,
            )),
            seed(0x101010, 0xffffff, 0xffc799, 0xff8080, 0x99ffe4, 0xffc799).with_terminal(ansi(
                0x101010, 0xff8080, 0x99ffe4, 0xffc799, 0xaca1ff, 0xffc799, 0x99ffe4, 0xffffff,
                0xa0a0a0, 0xff8080, 0x99ffe4, 0xffc799, 0xaca1ff, 0xffc799, 0x99ffe4, 0xffffff,
            )),
        ),
        family(
            "ayu-mirage",
            "Ayu Mirage",
            seed(0xf8f9fa, 0x5c6166, 0xffaa33, 0xf07171, 0x86b300, 0xf2ae49).with_terminal(ansi(
                0x000000, 0xf07171, 0x86b300, 0xf2ae49, 0x399ee6, 0xa37acc, 0x4cbf99, 0xabb0b6,
                0x828a8f, 0xf51818, 0x86b300, 0xffaa33, 0x478acc, 0xff7383, 0x55b4d4, 0x5c6166,
            )),
            seed(0x1f2430, 0xcccac2, 0xffcc66, 0xf28779, 0xbae67e, 0xffd580).with_terminal(ansi(
                0x191e2a, 0xed8274, 0xa6cc70, 0xfad07b, 0x6dcbfa, 0xcfbafa, 0x90e1c6, 0xc7c7c7,
                0x686868, 0xf28779, 0xbae67e, 0xffd580, 0x73d0ff, 0xd4bfff, 0x95e6cb, 0xffffff,
            )),
        ),
    ]);
    catalog
}

static CATALOG: LazyLock<Vec<ThemeFamily>> = LazyLock::new(build_catalog);

/// Built-in dual-mode palettes shown in Settings.
pub fn built_in_themes() -> &'static [ThemeFamily] {
    CATALOG.as_slice()
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

pub fn is_known_theme_id(id: &str) -> bool {
    built_in_themes().iter().any(|family| family.id == id)
        || user_themes().iter().any(|family| family.id == id)
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
        assert!(is_known_theme_id("nord"));
        assert!(is_known_theme_id("gruvbox"));
        assert!(is_known_theme_id("catppuccin"));
        assert!(is_known_theme_id("tokyo"));
        assert!(is_known_theme_id("warp"));
        assert!(is_known_theme_id("flexoki"));
        assert!(is_known_theme_id("tokyo-storm"));
        assert!(is_known_theme_id("catppuccin-frappe"));
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
