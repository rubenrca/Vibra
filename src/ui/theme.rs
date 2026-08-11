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
    /// Gutter behind line numbers in the diff viewer.
    pub gutter: Rgba,
    /// Soft vertical indent guides in the file tree.
    pub indent_guide: Rgba,
    /// Default folder icon tint (Zed-like cool gray-violet).
    pub folder: Rgba,
    /// Git-modified name tint (Zed gold).
    pub git_modified: Rgba,
    /// Git-added / untracked name tint.
    pub git_added: Rgba,
    /// Git-deleted / conflict name tint.
    pub git_deleted: Rgba,
}

pub static DARK: LazyLock<Theme> = LazyLock::new(|| Theme {
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
    // Slightly stronger full-row tints so inline diffs read like Warp.
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
});
