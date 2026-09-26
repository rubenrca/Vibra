use std::borrow::Cow;

use gpui::{
    AnyElement, AssetSource, IntoElement, ParentElement, Rgba, SharedString, Styled, div, img,
    prelude::*, px, svg,
};

use crate::infrastructure::automation::{AgentAttention, AgentRuntimeState};
use crate::ui::theme::{MONO_FONT, colors};

/// Glyph used when a pane has no detected agent mark.
pub const TERMINAL_GLYPH: &str = ">_";
macro_rules! bundled_assets {
    ($(($key:literal, $rel:literal)),+ $(,)?) => {
        impl AssetSource for VibraAssets {
            fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
                Ok(match path {
                    $($key => Some(Cow::Borrowed(include_bytes!(concat!("../../Resources/", $rel)))),)+
                    _ => None,
                })
            }

            fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
                const ASSETS: &[&str] = &[$($key),+];
                let prefix = path.trim_matches('/');
                Ok(ASSETS
                    .iter()
                    .copied()
                    .filter(|asset| prefix.is_empty() || asset.starts_with(prefix))
                    .map(SharedString::from)
                    .collect())
            }
        }
    };
}

/// Bundled agent brand marks served through GPUI's asset source.
pub struct VibraAssets;

bundled_assets! {
    ("agent-marks/aider.svg", "AgentMarks/aider.svg"),
    ("agent-marks/amp.svg", "AgentMarks/amp.svg"),
    ("agent-marks/claude.svg", "AgentMarks/claude.svg"),
    ("agent-marks/codex.svg", "AgentMarks/codex.svg"),
    ("agent-marks/cursor.svg", "AgentMarks/cursor.svg"),
    ("agent-marks/gemini.svg", "AgentMarks/gemini.svg"),
    ("agent-marks/goose.svg", "AgentMarks/goose.svg"),
    ("agent-marks/grok.svg", "AgentMarks/grok.svg"),
    ("agent-marks/opencode.svg", "AgentMarks/opencode.svg"),
    ("agent-marks/pi.svg", "AgentMarks/pi.svg"),
    ("file-icons/folder.svg", "FileIcons/folder.svg"),
    ("file-icons/folder-open.svg", "FileIcons/folder-open.svg"),
    ("file-icons/file.svg", "FileIcons/file.svg"),
    ("chrome-icons/files.svg", "ChromeIcons/files.svg"),
    ("chrome-icons/folder.svg", "ChromeIcons/folder.svg"),
    ("chrome-icons/plus.svg", "ChromeIcons/plus.svg"),
    ("chrome-icons/ellipsis.svg", "ChromeIcons/ellipsis.svg"),
    ("chrome-icons/git-branch.svg", "ChromeIcons/git-branch.svg"),
    ("chrome-icons/open-external.svg", "ChromeIcons/open-external.svg"),
    ("chrome-icons/chevron-down.svg", "ChromeIcons/chevron-down.svg"),
    ("chrome-icons/chevron-up.svg", "ChromeIcons/chevron-up.svg"),
    ("chrome-icons/fold-vertical.svg", "ChromeIcons/fold-vertical.svg"),
    ("chrome-icons/unfold-vertical.svg", "ChromeIcons/unfold-vertical.svg"),
    ("chrome-icons/chevrons-left.svg", "ChromeIcons/chevrons-left.svg"),
    ("chrome-icons/chevrons-right.svg", "ChromeIcons/chevrons-right.svg"),
    ("chrome-icons/chevron-right.svg", "ChromeIcons/chevron-right.svg"),
    ("chrome-icons/chevron-left.svg", "ChromeIcons/chevron-left.svg"),
    ("chrome-icons/diff-split.svg", "ChromeIcons/diff-split.svg"),
    ("chrome-icons/diff-unified.svg", "ChromeIcons/diff-unified.svg"),
    ("chrome-icons/wrap.svg", "ChromeIcons/wrap.svg"),
    ("chrome-icons/comment.svg", "ChromeIcons/comment.svg"),
    ("chrome-icons/send.svg", "ChromeIcons/send.svg"),
    ("chrome-icons/close.svg", "ChromeIcons/close.svg"),
    ("chrome-icons/search.svg", "ChromeIcons/search.svg"),
    ("chrome-icons/inbox.svg", "ChromeIcons/inbox.svg"),
    ("chrome-icons/notes.svg", "ChromeIcons/notes.svg"),
    ("chrome-icons/automations.svg", "ChromeIcons/automations.svg"),
    ("chrome-icons/settings.svg", "ChromeIcons/settings.svg"),
    ("chrome-icons/project.svg", "ChromeIcons/project.svg"),
    ("chrome-icons/check.svg", "ChromeIcons/check.svg"),
    ("chrome-icons/git-pull-request.svg", "ChromeIcons/git-pull-request.svg"),
    ("chrome-icons/git-commit.svg", "ChromeIcons/git-commit.svg"),
    ("chrome-icons/maximize.svg", "ChromeIcons/maximize.svg"),
    ("chrome-icons/minimize.svg", "ChromeIcons/minimize.svg"),
    ("chrome-icons/refresh.svg", "ChromeIcons/refresh.svg"),
    ("chrome-icons/sparkles.svg", "ChromeIcons/sparkles.svg"),
    ("chrome-icons/minus.svg", "ChromeIcons/minus.svg"),
    ("chrome-icons/file-plus.svg", "ChromeIcons/file-plus.svg"),
    ("chrome-icons/folder-plus.svg", "ChromeIcons/folder-plus.svg"),
    ("chrome-icons/collapse-all.svg", "ChromeIcons/collapse-all.svg"),
    ("chrome-icons/grip.svg", "ChromeIcons/grip.svg"),
    ("chrome-icons/split-view.svg", "ChromeIcons/split-view.svg"),

}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentMarkStyle {
    /// Single-color SVG recolored with the chrome foreground.
    Template,
    /// Multicolor asset rendered with original colors.
    Original,
}

/// Bundled SVG, render style, and optical scale within the shared icon slot.
/// Dense silhouettes need less space than thin marks to appear equally sized.
fn agent_mark(kind: &str) -> Option<(&'static str, AgentMarkStyle, f32)> {
    match kind {
        "Aider" => Some(("agent-marks/aider.svg", AgentMarkStyle::Template, 0.86)),
        "Amp" => Some(("agent-marks/amp.svg", AgentMarkStyle::Template, 0.88)),
        "Claude" => Some(("agent-marks/claude.svg", AgentMarkStyle::Template, 1.0)),
        "Codex" => Some(("agent-marks/codex.svg", AgentMarkStyle::Template, 1.0)),
        "Cursor" => Some(("agent-marks/cursor.svg", AgentMarkStyle::Template, 1.0)),
        "Gemini" => Some(("agent-marks/gemini.svg", AgentMarkStyle::Template, 1.1)),
        "Goose" => Some(("agent-marks/goose.svg", AgentMarkStyle::Original, 0.92)),
        // Template silhouette (no black square background) so it recolors with chrome.
        "Grok" => Some(("agent-marks/grok.svg", AgentMarkStyle::Template, 1.1)),
        "OpenCode" => Some(("agent-marks/opencode.svg", AgentMarkStyle::Template, 0.96)),
        "Pi" => Some(("agent-marks/pi.svg", AgentMarkStyle::Template, 1.1)),
        _ => None,
    }
}

pub fn agent_status_color(
    state: Option<AgentRuntimeState>,
    attention: Option<AgentAttention>,
) -> Option<Rgba> {
    match (state, attention) {
        (Some(AgentRuntimeState::Waiting), Some(AgentAttention::Permission)) => {
            Some(colors().danger)
        }
        (Some(AgentRuntimeState::Waiting), _) => Some(colors().warning),
        (Some(AgentRuntimeState::Working), _) => Some(colors().accent),
        (Some(AgentRuntimeState::Idle), _) => Some(colors().subtle),
        (None, _) => None,
    }
}

// Callers center the artwork in a fixed-size slot, keeping text and status aligned.
fn brand_mark(kind: Option<&str>, mark_color: Rgba, size: f32) -> AnyElement {
    match kind.and_then(agent_mark) {
        Some((path, AgentMarkStyle::Template, scale)) => svg()
            .path(path)
            .size(px(size * scale))
            .flex_none()
            .text_color(mark_color)
            .into_any_element(),
        Some((path, AgentMarkStyle::Original, scale)) => img(path)
            .size(px(size * scale))
            .flex_none()
            .into_any_element(),
        None => div()
            .size(px(size))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .font_family(MONO_FONT)
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_size(px((size * 9.5 / 16.0).max(8.0)))
            .text_color(mark_color)
            .child(TERMINAL_GLYPH)
            .into_any_element(),
    }
}

fn badge_mark_color(selected: bool) -> Rgba {
    if selected {
        colors().foreground
    } else {
        colors().muted
    }
}

/// Compact mark + runtime state used by pane headers and other dense chrome.
pub fn agent_compact_badge(
    kind: Option<&str>,
    state: Option<AgentRuntimeState>,
    attention: Option<AgentAttention>,
    selected: bool,
) -> AnyElement {
    let mark_color = badge_mark_color(selected);
    let status = agent_status_color(state, attention);

    div()
        .h(px(18.0))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(5.0))
        .child(
            div()
                .size(px(18.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(brand_mark(kind, mark_color, 16.0)),
        )
        .when_some(status, |badge, color| {
            badge.child(div().size(px(5.0)).flex_none().rounded_full().bg(color))
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_agents_resolve_to_bundled_marks() {
        for kind in [
            "Aider", "Amp", "Claude", "Codex", "Cursor", "Gemini", "Goose", "Grok", "OpenCode",
            "Pi",
        ] {
            assert!(agent_mark(kind).is_some(), "{kind}");
        }
        assert!(agent_mark("Agent").is_none());
        assert_eq!(
            agent_mark("Grok").map(|(_, style, _)| style),
            Some(AgentMarkStyle::Template)
        );
    }

    #[test]
    fn permission_uses_danger_instead_of_waiting_warning() {
        assert_eq!(
            agent_status_color(
                Some(AgentRuntimeState::Waiting),
                Some(AgentAttention::Permission)
            ),
            Some(colors().danger)
        );
        assert_eq!(
            agent_status_color(Some(AgentRuntimeState::Waiting), None),
            Some(colors().warning)
        );
    }

    #[test]
    fn assets_load_embedded_marks() {
        let assets = VibraAssets;
        let bytes = assets
            .load("agent-marks/claude.svg")
            .unwrap()
            .expect("claude mark should be embedded");
        assert!(bytes.starts_with(b"<svg") || bytes.starts_with(b"<?xml"));
        let files = assets
            .load("chrome-icons/files.svg")
            .unwrap()
            .expect("files tab icon should be embedded");
        assert!(files.starts_with(b"<svg") || files.starts_with(b"<?xml"));
        assert!(assets.load("missing.svg").unwrap().is_none());
    }
}
