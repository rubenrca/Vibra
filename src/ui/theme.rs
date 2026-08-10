use std::sync::LazyLock;

use gpui::{Rgba, rgb, rgba};

#[derive(Debug, Clone, Copy)]
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
}

pub static DARK: LazyLock<Theme> = LazyLock::new(|| Theme {
    background: rgb(0x101011),
    terminal: rgb(0x101011),
    titlebar: rgb(0x101011),
    sidebar: rgb(0x141415),
    panel: rgb(0x141415),
    elevated: rgb(0x1a1a1b),
    hover: rgb(0x1e1e20),
    selection: rgb(0x202022),
    border_subtle: rgb(0x29292b),
    foreground: rgb(0xe8e8e8),
    muted: rgb(0xa0a0a4),
    subtle: rgb(0x6c6c71),
    success: rgb(0x58b87a),
    danger: rgb(0xdd6b6b),
    warning: rgb(0xd7ad61),
    accent: rgb(0x82aaff),
    diff_added: rgb(0x6bcf8e),
    diff_added_bg: rgba(0x14261aff),
    diff_deleted: rgb(0xe06c75),
    diff_deleted_bg: rgba(0x2a1618ff),
    diff_hunk_bg: rgba(0x161b26ff),
});
