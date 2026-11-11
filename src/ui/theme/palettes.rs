//! Built-in palettes and the compact seed used to derive chrome colors.

use std::sync::LazyLock;

use gpui::{Hsla, Rgba, hsla, rgb, rgba};

use super::legibility::legible;
use super::{
    FRAME_WASH, SyntaxColors, Theme, ThemeFamily, mix, relative_luminance, subtle_border,
    with_alpha, with_floating_frame,
};
use crate::ports::terminal::TerminalRgb;

/// Compact terminal/UI seed. Chrome surfaces are derived so a new palette
/// stays consistent without listing every role by hand.
#[derive(Debug, Clone, Copy)]
pub(super) struct ThemeSeed {
    background: u32,
    foreground: u32,
    accent: u32,
    danger: u32,
    success: u32,
    warning: u32,
    cursor: Option<u32>,
    ansi: Option<[u32; 16]>,
}

pub(super) const fn seed(
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

impl ThemeSeed {
    pub(super) const fn with_terminal(self, ansi: [u32; 16]) -> Self {
        Self {
            ansi: Some(ansi),
            ..self
        }
    }

    pub(super) const fn with_cursor(self, cursor: u32) -> Self {
        Self {
            cursor: Some(cursor),
            ..self
        }
    }
}

pub(super) fn theme_from_seed(spec: ThemeSeed) -> Theme {
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
    let border = subtle_border(fg);
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
        syntax: None,
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
        light: legible(with_floating_frame(theme_from_seed(light))),
        dark: legible(with_floating_frame(theme_from_seed(dark))),
    }
}

fn rgb_u32_to_terminal(color: u32) -> TerminalRgb {
    TerminalRgb::new(
        ((color >> 16) & 0xff) as u8,
        ((color >> 8) & 0xff) as u8,
        (color & 0xff) as u8,
    )
}

pub(crate) const DEFAULT_THEME_ID: &str = "midnight";

struct ProductChrome {
    sidebar: u32,
    panel: u32,
    elevated: u32,
    hover: u32,
    selection: u32,
    muted: u32,
    subtle: u32,
    gutter: u32,
    folder: u32,
    indent_guide: u32,
    terminal: Option<u32>,
}

fn apply_chrome(mut theme: Theme, chrome: ProductChrome) -> Theme {
    theme.sidebar = rgb(chrome.sidebar);
    theme.panel = rgb(chrome.panel);
    theme.elevated = rgb(chrome.elevated);
    theme.hover = rgb(chrome.hover);
    theme.selection = rgb(chrome.selection);
    theme.muted = rgb(chrome.muted);
    theme.subtle = rgb(chrome.subtle);
    theme.gutter = rgb(chrome.gutter);
    theme.folder = rgb(chrome.folder);
    theme.indent_guide = rgba(chrome.indent_guide);
    if let Some(terminal) = chrome.terminal {
        theme.terminal = rgb(terminal);
    }
    theme
}

// ---------------------------------------------------------------------------
// Built-in palettes
// ---------------------------------------------------------------------------

fn product_theme(seed: ThemeSeed, chrome: ProductChrome) -> Theme {
    apply_chrome(theme_from_seed(seed), chrome)
}

fn with_diff_surfaces(mut theme: Theme, added: u32, deleted: u32, hunk: u32) -> Theme {
    theme.diff_added_bg = rgba(added);
    theme.diff_deleted_bg = rgba(deleted);
    theme.diff_hunk_bg = rgba(hunk);
    theme
}

/// Light rows need deeper counts and line numbers than the status colors.
fn with_diff_text(mut theme: Theme, added: u32, deleted: u32) -> Theme {
    theme.diff_added = rgb(added);
    theme.diff_deleted = rgb(deleted);
    theme
}

/// Shared by the light product palettes; clears 4.5:1 on every added row.
const LIGHT_DIFF_ADDED: u32 = 0x17763f;

pub(crate) fn midnight_dark() -> Theme {
    let mut theme = product_theme(
        seed(0x0c0c0c, 0xe6e6e6, 0x528bff, 0xdd6b6b, 0x58b87a, 0xd7ad61),
        ProductChrome {
            sidebar: 0x0a0a0a,
            panel: 0x101010,
            elevated: 0x191919,
            hover: 0x1e1e1e,
            selection: 0x262626,
            muted: 0x999999,
            subtle: 0x787878,
            gutter: 0x0e0e0e,
            folder: 0x999999,
            indent_guide: 0x38383855,
            terminal: None,
        },
    );
    theme.diff_added = rgb(0x6ee7b7);
    theme.diff_added_bg = rgb(0x162f2b);
    theme.diff_deleted = rgb(0xfda4af);
    theme.diff_deleted_bg = rgb(0x381d22);
    theme.diff_hunk_bg = rgb(0x181818);
    theme.git_modified = rgb(0xdcb67a);
    theme.git_added = rgb(0x6bcf8e);
    theme.git_deleted = rgb(0xe06c75);
    theme
}

pub(crate) fn midnight_light() -> Theme {
    let mut theme = product_theme(
        seed(0xfafafa, 0x202020, 0x3b6fd4, 0xc23b3b, 0x219452, 0xad761c),
        ProductChrome {
            sidebar: 0xf4f4f4,
            panel: 0xffffff,
            elevated: 0xffffff,
            hover: 0xededed,
            selection: 0xe4e4e4,
            muted: 0x616161,
            subtle: 0x818181,
            gutter: 0xeeeef1,
            folder: 0x6a6a88,
            indent_guide: 0x9a9aa855,
            terminal: Some(0xfafafa),
        },
    );
    theme = with_diff_surfaces(theme, 0xd8f0e0ff, 0xf5d8daff, 0xe8ecf5ff);
    theme = with_diff_text(theme, LIGHT_DIFF_ADDED, 0xb52e31);
    theme.git_modified = rgb(0xa07830);
    theme
}

// ---------------------------------------------------------------------------
// Workspace presets
// ---------------------------------------------------------------------------

/// A workspace preset. Its dark surface sets the hue, saturation and dark
/// lightness of the window; each tone brings its ink, accent and code colors.
#[derive(Debug, Clone, Copy)]
struct Workspace {
    surface: u32,
    light: Tone,
    dark: Tone,
}

#[derive(Debug, Clone, Copy)]
struct Tone {
    content: u32,
    accent: u32,
    /// Links and headings in Markdown.
    link: u32,
    heading: u32,
    /// Keyword, function, string, type, tag, number, comment, property, meta.
    syntax: [u32; 9],
}

/// The window color a preset paints: its surface's hue and saturation in
/// whole degrees and percents, at `light` lightness, or at the surface's own
/// whole-percent lightness (capped at 30%) in dark.
fn shell(surface: u32, light: Option<f32>) -> Rgba {
    let color: Hsla = rgb(surface).into();
    let lightness = light.unwrap_or_else(|| (color.l * 100.0).round().min(30.0) / 100.0);
    hsla(
        (color.h * 360.0).round() / 360.0,
        (color.s * 100.0).round() / 100.0,
        lightness,
        1.0,
    )
    .to_rgb()
}

/// Dark cards are the window color itself and the frame is the same color
/// with 5% ink over it. Light windows sit at 97% lightness under that same
/// 5% ink, with white cards. Status colors stay one set per tone, and diff
/// rows are emerald and rose at 15% over the card.
fn workspace_theme(preset: Workspace, light: bool) -> Theme {
    let tone = if light { preset.light } else { preset.dark };
    let card = if light {
        rgb(0xffffff)
    } else {
        shell(preset.surface, None)
    };
    let (danger, success, warning) = if light {
        (0xc23b3b, 0x219452, 0xad761c)
    } else {
        (0xdd6b6b, 0x58b87a, 0xd7ad61)
    };
    let mut theme = theme_from_seed(seed(
        rgba_to_u32(card),
        tone.content,
        tone.accent,
        danger,
        success,
        warning,
    ));
    theme.terminal = theme.background;
    let base = if light {
        shell(preset.surface, Some(0.97))
    } else {
        theme.background
    };
    theme.sidebar = mix(base, theme.foreground, FRAME_WASH);
    theme.diff_added_bg = mix(theme.background, rgb(0x10b981), 0.15);
    theme.diff_deleted_bg = mix(theme.background, rgb(0xf43f5e), 0.15);
    (theme.diff_added, theme.diff_deleted) = if light {
        (rgb(0x047857), rgb(0xbe123c))
    } else {
        (rgb(0x6ee7b7), rgb(0xfda4af))
    };
    let [
        keyword,
        function,
        string,
        type_name,
        tag,
        number,
        comment,
        property,
        meta,
    ] = tone.syntax.map(rgb);
    theme.syntax = Some(SyntaxColors {
        keyword,
        function,
        string,
        type_name,
        tag,
        number,
        comment,
        property,
        meta,
        heading: rgb(tone.heading),
        link: rgb(tone.link),
    });
    theme
}

fn rgba_to_u32(color: Rgba) -> u32 {
    let channel = |value: f32| (value * 255.0).round().clamp(0.0, 255.0) as u32;
    (channel(color.r) << 16) | (channel(color.g) << 8) | channel(color.b)
}

fn workspace_family(id: &str, label: &str, preset: Workspace) -> ThemeFamily {
    ThemeFamily {
        id: id.to_string(),
        label: label.to_string(),
        light: legible(workspace_theme(preset, true)),
        dark: legible(workspace_theme(preset, false)),
    }
}

#[cfg(test)]
pub(crate) fn moss_dark() -> Theme {
    workspace_family("moss", "Moss", MOSS).dark
}

/// Settings order, after Midnight.
const WORKSPACE_PRESETS: [(&str, &str, Workspace); 10] = [
    ("harbor", "Harbor", HARBOR),
    ("moss", "Moss", MOSS),
    ("ember", "Ember", EMBER),
    ("orchid", "Orchid", ORCHID),
    ("tidepool", "Tidepool", TIDEPOOL),
    ("dune", "Dune", DUNE),
    ("rosewood", "Rosewood", ROSEWOOD),
    ("glacier", "Glacier", GLACIER),
    ("arcade", "Arcade", ARCADE),
    ("ink", "Ink", INK),
];

const HARBOR: Workspace = Workspace {
    surface: 0x0b1018,
    light: Tone {
        content: 0x1b2533,
        accent: 0x2f6fd0,
        link: 0x1f5fbf,
        heading: 0x3b4fb8,
        syntax: [
            0x7847bd, 0x2e7de9, 0x587539, 0x007197, 0xc64343, 0xb15c00, 0x6c7594, 0x387068,
            0x006a83,
        ],
    },
    dark: Tone {
        content: 0xdbe4f0,
        accent: 0x5b9cf2,
        link: 0x8cc4ff,
        heading: 0xa3b6ff,
        syntax: [
            0xbb9af7, 0x7aa2f7, 0x9ece6a, 0x2ac3de, 0xf7768e, 0xff9e64, 0x6b7599, 0x73daca,
            0x89ddff,
        ],
    },
};
const MOSS: Workspace = Workspace {
    surface: 0x0d110e,
    light: Tone {
        content: 0x1f2a20,
        accent: 0x3d8a45,
        link: 0x2e7a3a,
        heading: 0x5d7a1c,
        syntax: [
            0xe0444a, 0x5f7a00, 0x9a7400, 0x2b8a65, 0xc4621a, 0xa3529b, 0x7a8778, 0x2f7fae,
            0x6e7a70,
        ],
    },
    dark: Tone {
        content: 0xdde5dc,
        accent: 0x6fbf73,
        link: 0x9ad6a0,
        heading: 0xc3d98a,
        syntax: [
            0xe67e80, 0xa7c080, 0xdbbc7f, 0x83c092, 0xe69875, 0xd699b6, 0x859289, 0x7fbbb3,
            0x9da9a0,
        ],
    },
};
const EMBER: Workspace = Workspace {
    surface: 0x120e0c,
    light: Tone {
        content: 0x2d211b,
        accent: 0xc75c1e,
        link: 0xb4501a,
        heading: 0xa8461f,
        syntax: [
            0xaf3a03, 0x427b58, 0x79740e, 0xb57614, 0x9d0006, 0x8f3f71, 0x7c6f64, 0x076678,
            0x665c54,
        ],
    },
    dark: Tone {
        content: 0xecdfd6,
        accent: 0xf08a4b,
        link: 0xf6b07c,
        heading: 0xf3a37a,
        syntax: [
            0xfe8019, 0x8ec07c, 0xb8bb26, 0xfabd2f, 0xfb4934, 0xd3869b, 0x928374, 0x83a598,
            0xa89984,
        ],
    },
};
const ORCHID: Workspace = Workspace {
    surface: 0x110d16,
    light: Tone {
        content: 0x261e33,
        accent: 0x8048c8,
        link: 0x6f3bb8,
        heading: 0xa83a85,
        syntax: [
            0x8839ef, 0x1e66f5, 0x2f7d1e, 0xb06a00, 0xd20f39, 0xc94f05, 0x7c7f93, 0x5a65d6,
            0x137a85,
        ],
    },
    dark: Tone {
        content: 0xe6def0,
        accent: 0xb57cf0,
        link: 0xcfa6ff,
        heading: 0xf2a2d6,
        syntax: [
            0xcba6f7, 0x89b4fa, 0xa6e3a1, 0xf9e2af, 0xf38ba8, 0xfab387, 0x7f849c, 0xb4befe,
            0x94e2d5,
        ],
    },
};
const TIDEPOOL: Workspace = Workspace {
    surface: 0x0a1213,
    light: Tone {
        content: 0x162a2b,
        accent: 0x138a80,
        link: 0x0f7a71,
        heading: 0x1d6f8f,
        syntax: [
            0xc2502b, 0x0b7a8a, 0x3c7d2a, 0x8a6500, 0x0f7a71, 0x6b4fc4, 0x6b8285, 0x1d6f8f,
            0x5b7072,
        ],
    },
    dark: Tone {
        content: 0xd8e8e8,
        accent: 0x3fc1b5,
        link: 0x7fdcd3,
        heading: 0x8ad0e8,
        syntax: [
            0xf28f6b, 0x5fd4e0, 0xa8e6a1, 0xffd580, 0x7fdcd3, 0xc4a7ff, 0x6f8d8f, 0x8ad0e8,
            0x8aa3a4,
        ],
    },
};
const DUNE: Workspace = Workspace {
    surface: 0x13110d,
    light: Tone {
        content: 0x2e2819,
        accent: 0xa57d1e,
        link: 0x8c6914,
        heading: 0x9c5a1c,
        syntax: [
            0x624c83, 0x4d699b, 0x6f894e, 0x597b75, 0xc84053, 0xb35b79, 0x7d7465, 0x8c6914,
            0x716e61,
        ],
    },
    dark: Tone {
        content: 0xebe3d2,
        accent: 0xd9b25c,
        link: 0xe8cc8a,
        heading: 0xe5a86c,
        syntax: [
            0x957fb8, 0x7e9cd8, 0x98bb6c, 0x7aa89f, 0xe46876, 0xd27e99, 0x8a8170, 0xe6c384,
            0xa6a69c,
        ],
    },
};
const ROSEWOOD: Workspace = Workspace {
    surface: 0x140e10,
    light: Tone {
        content: 0x2f1f24,
        accent: 0xc0405c,
        link: 0xa8344f,
        heading: 0xa3502a,
        syntax: [
            0xb4637a, 0xb0605c, 0xa86a0f, 0x286983, 0xc0405c, 0x9a4d2a, 0x797593, 0x7a5f9b,
            0x6e6a86,
        ],
    },
    dark: Tone {
        content: 0xefdfe3,
        accent: 0xe7798f,
        link: 0xf2a6b4,
        heading: 0xf0b38a,
        syntax: [
            0xeb6f92, 0xebbcba, 0xf6c177, 0x9ccfd8, 0xe7798f, 0xea9a97, 0x7d7891, 0xc4a7e7,
            0x908caa,
        ],
    },
};
const GLACIER: Workspace = Workspace {
    surface: 0x0e1114,
    light: Tone {
        content: 0x20262d,
        accent: 0x3a7aa8,
        link: 0x2d6a96,
        heading: 0x44598f,
        syntax: [
            0x3a6d9f, 0x2b7a8c, 0x4f7a34, 0x2a7a76, 0xb25a2e, 0x7b4f8a, 0x6b7689, 0x44598f,
            0x5a6a80,
        ],
    },
    dark: Tone {
        content: 0xe1e7ec,
        accent: 0x7fb4d9,
        link: 0xa7d0ec,
        heading: 0xb9c8e8,
        syntax: [
            0x81a1c1, 0x88c0d0, 0xa3be8c, 0x8fbcbb, 0xd08770, 0xb48ead, 0x7b88a1, 0xa7c2dc,
            0x7f97b8,
        ],
    },
};
const ARCADE: Workspace = Workspace {
    surface: 0x0f0b1a,
    light: Tone {
        content: 0x231a3a,
        accent: 0xc2189a,
        link: 0x0a7f90,
        heading: 0x9a6608,
        syntax: [
            0xc2189a, 0x0a7f90, 0x8a5a00, 0x6a3fd1, 0xc2410c, 0xc2304a, 0x7a6f99, 0x0f7a55,
            0x6b6390,
        ],
    },
    dark: Tone {
        content: 0xece6ff,
        accent: 0xff5fd2,
        link: 0x5ff2ff,
        heading: 0xffd35f,
        syntax: [
            0xff5fd2, 0x5ff2ff, 0xffd35f, 0xb38cff, 0xff8f5f, 0xf97e72, 0x7d71a8, 0x72f1b8,
            0x8d86c9,
        ],
    },
};
const INK: Workspace = Workspace {
    surface: 0x000000,
    light: Tone {
        content: 0x000000,
        accent: 0x0050d8,
        link: 0x0040b0,
        heading: 0x000000,
        syntax: [
            0x0050d8, 0x000000, 0x5a5a5a, 0x333333, 0x0050d8, 0x0040b0, 0x767676, 0x2a2a2a,
            0x6a6a6a,
        ],
    },
    dark: Tone {
        content: 0xf5f5f5,
        accent: 0x7aa7ff,
        link: 0x9cc0ff,
        heading: 0xffffff,
        syntax: [
            0x7aa7ff, 0xffffff, 0xa8a8a8, 0xd6d6d6, 0x7aa7ff, 0x9cc0ff, 0x7a7a7a, 0xd6d6d6,
            0x8f8f8f,
        ],
    },
};

fn product_family(id: &'static str, label: &'static str, light: Theme, dark: Theme) -> ThemeFamily {
    ThemeFamily {
        id: id.to_string(),
        label: label.to_string(),
        light: legible(with_floating_frame(light)),
        dark: legible(with_floating_frame(dark)),
    }
}

const CATPPUCCIN_LATTE: ThemeSeed =
    seed(0xeff1f5, 0x4c4f69, 0x1e66f5, 0xd20f39, 0x40a02b, 0xdf8e1d).with_terminal([
        0x5c5f77, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xacb0be, 0x6c6f85,
        0xde293e, 0x49af3d, 0xeea02d, 0x456eff, 0xfe85d8, 0x2d9fa8, 0xbcc0cc,
    ]);
const TOKYO_LIGHT: ThemeSeed = seed(0xe1e2e7, 0x3760bf, 0x2e7de9, 0xf52a65, 0x587539, 0x8c6c3e)
    .with_cursor(0x3760bf)
    .with_terminal([
        0xb4b5b9, 0xf52a65, 0x587539, 0x8c6c3e, 0x2e7de9, 0x9854f1, 0x007197, 0x6172b0, 0xa1a6c5,
        0xff4774, 0x5c8524, 0xa27629, 0x358aff, 0xa463ff, 0x007ea8, 0x3760bf,
    ]);
const KANAGAWA_LIGHT: ThemeSeed = seed(0xf2ecbc, 0x545464, 0x4d699b, 0xc84053, 0x6f894e, 0x77713f)
    .with_terminal([
        0xe7dba0, 0xc84053, 0x6f894e, 0x77713f, 0x4d699b, 0xb35b79, 0x597b75, 0x545464, 0x8a8980,
        0xd7474b, 0x6e915f, 0x83640a, 0x6693bf, 0x624c83, 0x5e857a, 0x43436c,
    ]);
const ROSEPINE_LIGHT: ThemeSeed = seed(0xfaf4ed, 0x575279, 0x907aa9, 0xb4637a, 0x286983, 0xea9d34)
    .with_terminal([
        0xf2e9e1, 0xb4637a, 0x286983, 0xea9d34, 0x56949f, 0x907aa9, 0xd7827e, 0x575279, 0x9893a5,
        0xb4637a, 0x286983, 0xea9d34, 0x56949f, 0x907aa9, 0xd7827e, 0x575279,
    ]);
const AYU_LIGHT: ThemeSeed = seed(0xf8f9fa, 0x5c6166, 0xffaa33, 0xf07171, 0x86b300, 0xf2ae49)
    .with_terminal([
        0x000000, 0xf07171, 0x86b300, 0xf2ae49, 0x399ee6, 0xa37acc, 0x4cbf99, 0xabb0b6, 0x828a8f,
        0xf51818, 0x86b300, 0xffaa33, 0x478acc, 0xff7383, 0x55b4d4, 0x5c6166,
    ]);
const PALENIGHT: ThemeSeed = seed(0x292d3e, 0xa6accd, 0x82aaff, 0xff5370, 0xc3e88d, 0xffcb6b)
    .with_terminal([
        0x292d3e, 0xf07178, 0xc3e88d, 0xffcb6b, 0x82aaff, 0xc792ea, 0x89ddff, 0xd0d0d0, 0x676e95,
        0xff5370, 0xc3e88d, 0xffcb6b, 0x82aaff, 0xc792ea, 0x89ddff, 0xffffff,
    ]);
const VESPER: ThemeSeed = seed(0x101010, 0xffffff, 0xffc799, 0xff8080, 0x99ffe4, 0xffc799)
    .with_terminal([
        0x101010, 0xff8080, 0x99ffe4, 0xffc799, 0xaca1ff, 0xffc799, 0x99ffe4, 0xffffff, 0xa0a0a0,
        0xff8080, 0x99ffe4, 0xffc799, 0xaca1ff, 0xffc799, 0x99ffe4, 0xffffff,
    ]);

fn build_catalog() -> Vec<ThemeFamily> {
    let mut catalog = vec![product_family(
        DEFAULT_THEME_ID,
        "Midnight",
        midnight_light(),
        midnight_dark(),
    )];
    catalog.extend(
        WORKSPACE_PRESETS
            .into_iter()
            .map(|(id, label, tones)| workspace_family(id, label, tones)),
    );
    // Popular palettes with authentic ANSI from upstream schemes / Warp YAML.
    catalog.extend([
        family(
            "nord",
            "Nord",
            seed(0xeceff4, 0x2e3440, 0x5e81ac, 0xbf616a, 0xa3be8c, 0xd08770).with_terminal([
                0xeceff4, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0x4c566a,
                0xd8dee9, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x8fbcbb, 0x2e3440,
            ]),
            seed(0x2e3440, 0xd8dee9, 0x81a1c1, 0xbf616a, 0xa3be8c, 0xebcb8b).with_terminal([
                0x3b4252, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0xe5e9f0,
                0x4c566a, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x8fbcbb, 0xeceff4,
            ]),
        ),
        family(
            "gruvbox",
            "Gruvbox",
            seed(0xfbf1c7, 0x3c3836, 0xaf3a03, 0x9d0006, 0x79740e, 0xb57614).with_terminal([
                0xfbf1c7, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0x7c6f64,
                0x928374, 0x9d0006, 0x79740e, 0xb57614, 0x076678, 0x8f3f71, 0x427b58, 0x3c3836,
            ]),
            seed(0x282828, 0xebdbb2, 0xfe8019, 0xfb4934, 0xb8bb26, 0xfabd2f).with_terminal([
                0x282828, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0xa89984,
                0x928374, 0xfb4934, 0xb8bb26, 0xfabd2f, 0x83a598, 0xd3869b, 0x8ec07c, 0xebdbb2,
            ]),
        ),
        family(
            "solarized",
            "Solarized",
            seed(0xfdf6e3, 0x657b83, 0x268bd2, 0xdc322f, 0x859900, 0xb58900).with_terminal([
                0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5,
                0x002b36, 0xcb4b16, 0x586e75, 0x657b83, 0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3,
            ]),
            seed(0x002b36, 0x839496, 0x268bd2, 0xdc322f, 0x859900, 0xb58900).with_terminal([
                0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5,
                0x002b36, 0xcb4b16, 0x586e75, 0x657b83, 0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3,
            ]),
        ),
        family(
            "dracula",
            "Dracula",
            seed(0xf8f8f2, 0x282a36, 0x6272a4, 0xc41e3a, 0x2e7d32, 0x9a7b0a),
            seed(0x282a36, 0xf8f8f2, 0xbd93f9, 0xff5555, 0x50fa7b, 0xf1fa8c)
                .with_cursor(0xf8f8f2)
                .with_terminal([
                    0x21222c, 0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xf8f8f2,
                    0x6272a4, 0xff6e6e, 0x69ff94, 0xffffa5, 0xd6acff, 0xff92d0, 0xa4ffff, 0xffffff,
                ]),
        ),
        family(
            "catppuccin",
            "Catppuccin",
            CATPPUCCIN_LATTE,
            seed(0x1e1e2e, 0xcdd6f4, 0x89b4fa, 0xf38ba8, 0xa6e3a1, 0xf9e2af).with_terminal([
                0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8,
                0x585b70, 0xf37799, 0x89d88b, 0xebd391, 0x74a8fc, 0xf2aede, 0x6bd7ca, 0xbac2de,
            ]),
        ),
        family(
            "tokyo",
            "Tokyo Night",
            TOKYO_LIGHT,
            seed(0x1a1b26, 0xc0caf5, 0x7aa2f7, 0xf7768e, 0x9ece6a, 0xe0af68)
                .with_cursor(0xc0caf5)
                .with_terminal([
                    0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6,
                    0x414868, 0xff899d, 0x9fe044, 0xfaba4a, 0x8db0ff, 0xc7a9ff, 0xa4daff, 0xc0caf5,
                ]),
        ),
        family(
            "one",
            "One Dark",
            seed(0xfafafa, 0x383a42, 0x4078f2, 0xe45649, 0x50a14f, 0xc18401).with_terminal([
                0x383a42, 0xe45649, 0x50a14f, 0xc18401, 0x4078f2, 0xa626a4, 0x0184bc, 0xa0a1a7,
                0x696c77, 0xe45649, 0x50a14f, 0xc18401, 0x4078f2, 0xa626a4, 0x0184bc, 0x383a42,
            ]),
            seed(0x282c34, 0xabb2bf, 0x61afef, 0xe06c75, 0x98c379, 0xe5c07b).with_terminal([
                0x1e2127, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf,
                0x5c6370, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xffffff,
            ]),
        ),
        family(
            "github",
            "GitHub",
            seed(0xffffff, 0x1f2328, 0x0969da, 0xcf222e, 0x1a7f37, 0x9a6700).with_terminal([
                0x24292f, 0xcf222e, 0x116329, 0x4d2d00, 0x0969da, 0x8250df, 0x1b7c83, 0x6e7781,
                0x57606a, 0xa40e26, 0x1a7f37, 0x633c01, 0x218bff, 0xa475f9, 0x3192aa, 0x8c959f,
            ]),
            seed(0x0d1117, 0xe6edf3, 0x58a6ff, 0xff7b72, 0x3fb950, 0xd29922).with_terminal([
                0x484f58, 0xff7b72, 0x3fb950, 0xd29922, 0x58a6ff, 0xbc8cff, 0x39c5cf, 0xb1bac4,
                0x6e7681, 0xffa198, 0x56d364, 0xe3b341, 0x79c0ff, 0xd2a8ff, 0x56d4dd, 0xffffff,
            ]),
        ),
        family(
            "ayu",
            "Ayu",
            AYU_LIGHT,
            seed(0x0a0e14, 0xb3b1ad, 0x53bdfa, 0xf07178, 0xc2d94c, 0xffb454).with_terminal([
                0x01060e, 0xea6c73, 0x91b362, 0xf9af4f, 0x53bdfa, 0xfae994, 0x90e1c6, 0xc7c7c7,
                0x686868, 0xf07178, 0xc2d94c, 0xffb454, 0x59c2ff, 0xffee99, 0x95e6cb, 0xffffff,
            ]),
        ),
        family(
            "everforest",
            "Everforest",
            seed(0xfffbef, 0x5c6a72, 0x3a94c5, 0xf85552, 0x8da101, 0xdfa000).with_terminal([
                0x5c6a72, 0xf85552, 0x8da101, 0xdfa000, 0x3a94c5, 0xdf69ba, 0x35a77c, 0xdfddc8,
                0x939f91, 0xe67e80, 0xa7c080, 0xdbbc7f, 0x7fbbb3, 0xd699b6, 0x83c092, 0x5c6a72,
            ]),
            seed(0x2b3339, 0xd3c6aa, 0x7fbbb3, 0xe67e80, 0xa7c080, 0xdbbc7f).with_terminal([
                0x4b565c, 0xe67e80, 0xa7c080, 0xdbbc7f, 0x7fbbb3, 0xd699b6, 0x83c092, 0xd3c6aa,
                0x4b565c, 0xe67e80, 0xa7c080, 0xdbbc7f, 0x7fbbb3, 0xd699b6, 0x83c092, 0xd3c6aa,
            ]),
        ),
        family(
            "kanagawa",
            "Kanagawa",
            KANAGAWA_LIGHT,
            seed(0x1f1f28, 0xdcd7ba, 0x7e9cd8, 0xc34043, 0x76946a, 0xe6c384).with_terminal([
                0x090618, 0xc34043, 0x76946a, 0xc0a36e, 0x7e9cd8, 0x957fb8, 0x6a9589, 0xc8c093,
                0x727169, 0xe82424, 0x98bb6c, 0xe6c384, 0x7fb4ca, 0x938aa9, 0x7aa89f, 0xdcd7ba,
            ]),
        ),
        family(
            "rosepine",
            "Rosé Pine",
            ROSEPINE_LIGHT,
            seed(0x191724, 0xe0def4, 0xc4a7e7, 0xeb6f92, 0x31748f, 0xf6c177).with_terminal([
                0x26233a, 0xeb6f92, 0x31748f, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xebbcba, 0xe0def4,
                0x6e6a86, 0xeb6f92, 0x31748f, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xebbcba, 0xe0def4,
            ]),
        ),
        family(
            "monokai",
            "Monokai",
            seed(0xfaf4ed, 0x2d2a2e, 0xab9df2, 0xe14775, 0x4d7c0f, 0xb45309),
            seed(0x2d2a2e, 0xfcfcfa, 0xab9df2, 0xff6188, 0xa9dc76, 0xffd866).with_terminal([
                0x2d2a2e, 0xff6188, 0xa9dc76, 0xffd866, 0xfc9867, 0xab9df2, 0x78dce8, 0xfcfcfa,
                0x727072, 0xff6188, 0xa9dc76, 0xffd866, 0xfc9867, 0xab9df2, 0x78dce8, 0xfcfcfa,
            ]),
        ),
        family(
            "warp",
            "Warp",
            seed(0xffffff, 0x111111, 0x008ec4, 0xc30771, 0x10a778, 0xa89c14).with_terminal([
                0x111111, 0xc30771, 0x10a778, 0xa89c14, 0x008ec4, 0x523c79, 0x20a5ba, 0xe0e0e0,
                0x777777, 0xfb007a, 0x98e024, 0xfff114, 0x00a6fb, 0x9b37ff, 0x4de8e8, 0xffffff,
            ]),
            seed(0x0b0d10, 0xf1f1f1, 0x00c2ff, 0xff8272, 0xb4fa72, 0xfefdc2).with_terminal([
                0x616161, 0xff8272, 0xb4fa72, 0xfefdc2, 0xa5d5fe, 0xff8ffd, 0xd0d1fe, 0xf1f1f1,
                0x8e8e8e, 0xffc4bd, 0xd6fcb9, 0xfefdd5, 0xc1e3fe, 0xffb1fe, 0xe5e6fe, 0xfeffff,
            ]),
        ),
        family(
            "catppuccin-frappe",
            "Catppuccin Frappé",
            CATPPUCCIN_LATTE,
            seed(0x303446, 0xc6d0f5, 0x8caaee, 0xe78284, 0xa6d189, 0xe5c890).with_terminal([
                0x51576d, 0xe78284, 0xa6d189, 0xe5c890, 0x8caaee, 0xf4b8e4, 0x81c8be, 0xa5adce,
                0x626880, 0xe67172, 0x8ec772, 0xd9ba73, 0x7b9ef0, 0xf2a4db, 0x5abfb5, 0xb5bfe2,
            ]),
        ),
        family(
            "catppuccin-macchiato",
            "Catppuccin Macchiato",
            CATPPUCCIN_LATTE,
            seed(0x24273a, 0xcad3f5, 0x8aadf4, 0xed8796, 0xa6da95, 0xeed49f).with_terminal([
                0x494d64, 0xed8796, 0xa6da95, 0xeed49f, 0x8aadf4, 0xf5bde6, 0x8bd5ca, 0xa5adcb,
                0x5b6078, 0xec7486, 0x8ccf7f, 0xe1c682, 0x78a1f6, 0xf2a9dd, 0x63cbc0, 0xb8c0e0,
            ]),
        ),
        family(
            "tokyo-storm",
            "Tokyo Night Storm",
            TOKYO_LIGHT,
            seed(0x24283b, 0xc0caf5, 0x7aa2f7, 0xf7768e, 0x9ece6a, 0xe0af68)
                .with_cursor(0xc0caf5)
                .with_terminal([
                    0x1d202f, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6,
                    0x414868, 0xff899d, 0x9fe044, 0xfaba4a, 0x8db0ff, 0xc7a9ff, 0xa4daff, 0xc0caf5,
                ]),
        ),
        family(
            "tokyo-moon",
            "Tokyo Night Moon",
            TOKYO_LIGHT,
            seed(0x222436, 0xc8d3f5, 0x82aaff, 0xff757f, 0xc3e88d, 0xffc777)
                .with_cursor(0xc8d3f5)
                .with_terminal([
                    0x1b1d2b, 0xff757f, 0xc3e88d, 0xffc777, 0x82aaff, 0xc099ff, 0x86e1fc, 0x828bb8,
                    0x444a73, 0xff8d94, 0xc7fb6d, 0xffd8ab, 0x9ab8ff, 0xcaabff, 0xb2ebff, 0xc8d3f5,
                ]),
        ),
        family(
            "flexoki",
            "Flexoki",
            seed(0xfffcf0, 0x100f0f, 0x205ea6, 0xaf3029, 0x66800b, 0xad8301)
                .with_cursor(0x100f0f)
                .with_terminal([
                    0x100f0f, 0xaf3029, 0x66800b, 0xad8301, 0x205ea6, 0xa02f6f, 0x24837b, 0x6f6e69,
                    0xb7b5ac, 0xd14d41, 0x879a39, 0xd0a215, 0x4385be, 0xce5d97, 0x3aa99f, 0xcecdc3,
                ]),
            seed(0x100f0f, 0xcecdc3, 0x4385be, 0xd14d41, 0x879a39, 0xd0a215)
                .with_cursor(0xcecdc3)
                .with_terminal([
                    0x100f0f, 0xaf3029, 0x66800b, 0xad8301, 0x205ea6, 0xa02f6f, 0x24837b, 0x878580,
                    0x6f6e69, 0xd14d41, 0x879a39, 0xd0a215, 0x4385be, 0xce5d97, 0x3aa99f, 0xcecdc3,
                ]),
        ),
        family(
            "horizon",
            "Horizon",
            seed(0xfdf0ed, 0x1c1e26, 0x26bbd9, 0xe95678, 0x29d398, 0xfab795).with_terminal([
                0xf2e6e4, 0xe95678, 0x29d398, 0xfab795, 0x26bbd9, 0xee64ac, 0x59e1e3, 0x1c1e26,
                0xbdb3b1, 0xec6a88, 0x3fdaa4, 0xfbc3a7, 0x3fc4de, 0xf075b5, 0x6be4e6, 0x06060c,
            ]),
            seed(0x1c1e26, 0xd5d8da, 0x26bbd9, 0xe95678, 0x29d398, 0xfab795).with_terminal([
                0x16161c, 0xe95678, 0x29d398, 0xfab795, 0x26bbd9, 0xee64ac, 0x59e1e3, 0xd5d8da,
                0x5b5858, 0xec6a88, 0x3fdaa4, 0xfbc3a7, 0x3fc4de, 0xf075b5, 0x6be4e6, 0xffffff,
            ]),
        ),
        family(
            "oxocarbon",
            "Oxocarbon",
            seed(0xffffff, 0x161616, 0x0f62fe, 0xee5396, 0x42be65, 0xff7eb6).with_terminal([
                0xffffff, 0xee5396, 0x42be65, 0xff7eb6, 0x0f62fe, 0xbe95ff, 0x08bdba, 0x525252,
                0x161616, 0xee5396, 0x42be65, 0xff7eb6, 0x78a9ff, 0xbe95ff, 0x3ddbd9, 0x161616,
            ]),
            seed(0x161616, 0xf2f4f8, 0x78a9ff, 0xee5396, 0x42be65, 0xffe97b).with_terminal([
                0x262626, 0xee5396, 0x42be65, 0xffe97b, 0x33b1ff, 0xbe95ff, 0x3ddbd9, 0xdde1e6,
                0x393939, 0xee5396, 0x42be65, 0xffe97b, 0x33b1ff, 0xbe95ff, 0x08bdba, 0xffffff,
            ]),
        ),
        family(
            "kanagawa-dragon",
            "Kanagawa Dragon",
            KANAGAWA_LIGHT,
            seed(0x181616, 0xc5c9c5, 0x8ba4b0, 0xc4746e, 0x87a987, 0xc4b28a).with_terminal([
                0x0d0c0c, 0xc4746e, 0x87a987, 0xc4b28a, 0x8ba4b0, 0xa292a3, 0x8ea4a2, 0xc5c9c5,
                0xa6a69c, 0xe46876, 0x87a987, 0xe6c384, 0x7fb4ca, 0x938aa9, 0x7aa89f, 0xc5c9c5,
            ]),
        ),
        family(
            "rosepine-moon",
            "Rosé Pine Moon",
            ROSEPINE_LIGHT,
            seed(0x232136, 0xe0def4, 0xc4a7e7, 0xeb6f92, 0x3e8fb0, 0xf6c177).with_terminal([
                0x393552, 0xeb6f92, 0x3e8fb0, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xea9a97, 0xe0def4,
                0x6e6a86, 0xeb6f92, 0x3e8fb0, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xea9a97, 0xe0def4,
            ]),
        ),
        family(
            "gruvbox-hard",
            "Gruvbox Hard",
            seed(0xf9f5d7, 0x3c3836, 0xaf3a03, 0x9d0006, 0x79740e, 0xb57614).with_terminal([
                0xf9f5d7, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0x7c6f64,
                0x928374, 0x9d0006, 0x79740e, 0xb57614, 0x076678, 0x8f3f71, 0x427b58, 0x3c3836,
            ]),
            seed(0x1d2021, 0xebdbb2, 0xfe8019, 0xfb4934, 0xb8bb26, 0xfabd2f).with_terminal([
                0x1d2021, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0xa89984,
                0x928374, 0xfb4934, 0xb8bb26, 0xfabd2f, 0x83a598, 0xd3869b, 0x8ec07c, 0xebdbb2,
            ]),
        ),
        family("palenight", "Palenight", PALENIGHT, PALENIGHT),
        family("vesper", "Vesper", VESPER, VESPER),
        family(
            "ayu-mirage",
            "Ayu Mirage",
            AYU_LIGHT,
            seed(0x1f2430, 0xcccac2, 0xffcc66, 0xf28779, 0xbae67e, 0xffd580).with_terminal([
                0x191e2a, 0xed8274, 0xa6cc70, 0xfad07b, 0x6dcbfa, 0xcfbafa, 0x90e1c6, 0xc7c7c7,
                0x686868, 0xf28779, 0xbae67e, 0xffd580, 0x73d0ff, 0xd4bfff, 0x95e6cb, 0xffffff,
            ]),
        ),
    ]);
    catalog
}

static CATALOG: LazyLock<Vec<ThemeFamily>> = LazyLock::new(build_catalog);

/// Built-in dual-mode palettes shown in Settings.
pub fn built_in_themes() -> &'static [ThemeFamily] {
    CATALOG.as_slice()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_palettes_meet_the_floors_by_hand() {
        for (id, theme) in [
            ("midnight light", midnight_light()),
            ("midnight dark", midnight_dark()),
        ] {
            assert_eq!(
                legible(theme),
                theme,
                "{id} relies on automatic corrections"
            );
        }
    }

    fn preset(id: &str) -> &'static ThemeFamily {
        built_in_themes()
            .iter()
            .find(|family| family.id == id)
            .unwrap_or_else(|| panic!("missing preset {id}"))
    }

    /// The presets keep their own ink and code colors through the contrast
    /// floors, and every frame stands apart from its cards.
    #[test]
    fn workspace_presets_keep_their_ink_and_code() {
        for (id, _, workspace) in WORKSPACE_PRESETS {
            let family = preset(id);
            for (tone, theme) in [
                (workspace.light, family.light),
                (workspace.dark, family.dark),
            ] {
                assert_eq!(theme.foreground, rgb(tone.content), "{id} ink");
                let syntax = theme.syntax.expect("preset ships code colors");
                assert_eq!(syntax.keyword, rgb(tone.syntax[0]), "{id} keyword");
                assert_eq!(syntax.link, rgb(tone.link), "{id} link");
                assert_ne!(theme.sidebar, theme.terminal, "{id} frame");
            }
        }
    }

    /// Dark cards are the window color, the surface to within rounding; the
    /// frame is that color with 5% ink. Light cards are white on a 97% frame.
    #[test]
    fn presets_paint_the_window_with_their_surface() {
        for (id, _, workspace) in WORKSPACE_PRESETS {
            let family = preset(id);
            let card = rgba_to_u32(family.dark.terminal);
            for shift in [16, 8, 0] {
                let (card, surface) = ((card >> shift) & 0xff, (workspace.surface >> shift) & 0xff);
                assert!(card.abs_diff(surface) <= 2, "{id} dark card {card:06x}");
            }
            assert_eq!(family.dark.background, family.dark.terminal, "{id} base");
            assert_eq!(
                family.dark.sidebar,
                mix(family.dark.terminal, family.dark.foreground, FRAME_WASH),
                "{id} dark frame"
            );
            assert_eq!(family.light.terminal, rgb(0xffffff), "{id} light card");
            assert_eq!(
                family.light.sidebar,
                mix(
                    shell(workspace.surface, Some(0.97)),
                    family.light.foreground,
                    FRAME_WASH
                ),
                "{id} light frame"
            );
        }
        assert_eq!(rgba_to_u32(preset("harbor").dark.terminal), 0x0b1018);
        assert_eq!(rgba_to_u32(preset("ink").dark.terminal), 0x000000);
    }
}
