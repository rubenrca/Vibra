use std::borrow::Cow;

use gpui::{
    AnyElement, AssetSource, IntoElement, ParentElement, Rgba, SharedString, Styled, div, img,
    prelude::*, px, svg,
};

use crate::infrastructure::automation::{AgentAttention, AgentRuntimeState};
use crate::ui::theme::colors;

/// Bundled agent brand marks served through GPUI's asset source.
pub struct VibraAssets;

impl AssetSource for VibraAssets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        Ok(match path {
            "agent-marks/aider.svg" => Some(Cow::Borrowed(include_bytes!(
                "../../Resources/AgentMarks/aider.svg"
            ))),
            "agent-marks/amp.svg" => Some(Cow::Borrowed(include_bytes!(
                "../../Resources/AgentMarks/amp.svg"
            ))),
            "agent-marks/claude.svg" => Some(Cow::Borrowed(include_bytes!(
                "../../Resources/AgentMarks/claude.svg"
            ))),
            "agent-marks/codex.svg" => Some(Cow::Borrowed(include_bytes!(
                "../../Resources/AgentMarks/codex.svg"
            ))),
            "agent-marks/cursor.svg" => Some(Cow::Borrowed(include_bytes!(
                "../../Resources/AgentMarks/cursor.svg"
            ))),
            "agent-marks/gemini.svg" => Some(Cow::Borrowed(include_bytes!(
                "../../Resources/AgentMarks/gemini.svg"
            ))),
            "agent-marks/goose.svg" => Some(Cow::Borrowed(include_bytes!(
                "../../Resources/AgentMarks/goose.svg"
            ))),
            "agent-marks/grok.svg" => Some(Cow::Borrowed(include_bytes!(
                "../../Resources/AgentMarks/grok.svg"
            ))),
            "agent-marks/opencode.svg" => Some(Cow::Borrowed(include_bytes!(
                "../../Resources/AgentMarks/opencode.svg"
            ))),
            "agent-marks/pi.svg" => Some(Cow::Borrowed(include_bytes!(
                "../../Resources/AgentMarks/pi.svg"
            ))),
            "file-icons/folder.svg" => Some(Cow::Borrowed(include_bytes!(
                "../../Resources/FileIcons/folder.svg"
            ))),
            "file-icons/folder-open.svg" => Some(Cow::Borrowed(include_bytes!(
                "../../Resources/FileIcons/folder-open.svg"
            ))),
            "file-icons/file.svg" => Some(Cow::Borrowed(include_bytes!(
                "../../Resources/FileIcons/file.svg"
            ))),
            "chrome-icons/files.svg" => Some(Cow::Borrowed(include_bytes!(
                "../../Resources/ChromeIcons/files.svg"
            ))),
            "chrome-icons/git-branch.svg" => Some(Cow::Borrowed(include_bytes!(
                "../../Resources/ChromeIcons/git-branch.svg"
            ))),
            _ => None,
        })
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let prefix = path.trim_matches('/');
        let assets = [
            "agent-marks/aider.svg",
            "agent-marks/amp.svg",
            "agent-marks/claude.svg",
            "agent-marks/codex.svg",
            "agent-marks/cursor.svg",
            "agent-marks/gemini.svg",
            "agent-marks/goose.svg",
            "agent-marks/grok.svg",
            "agent-marks/opencode.svg",
            "agent-marks/pi.svg",
            "file-icons/folder.svg",
            "file-icons/folder-open.svg",
            "file-icons/file.svg",
            "chrome-icons/files.svg",
            "chrome-icons/git-branch.svg",
        ];
        Ok(assets
            .into_iter()
            .filter(|asset| prefix.is_empty() || asset.starts_with(prefix))
            .map(SharedString::from)
            .collect())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentMarkStyle {
    /// Single-color SVG recolored with the chrome foreground.
    Template,
    /// Multicolor asset rendered with original colors.
    Original,
}

/// Map a detected agent display name to its bundled SVG path and render style.
fn agent_mark(kind: &str) -> Option<(&'static str, AgentMarkStyle)> {
    match kind {
        "Aider" => Some(("agent-marks/aider.svg", AgentMarkStyle::Template)),
        "Amp" => Some(("agent-marks/amp.svg", AgentMarkStyle::Template)),
        "Claude" => Some(("agent-marks/claude.svg", AgentMarkStyle::Template)),
        "Codex" => Some(("agent-marks/codex.svg", AgentMarkStyle::Template)),
        "Cursor" => Some(("agent-marks/cursor.svg", AgentMarkStyle::Template)),
        "Gemini" => Some(("agent-marks/gemini.svg", AgentMarkStyle::Template)),
        "Goose" => Some(("agent-marks/goose.svg", AgentMarkStyle::Original)),
        // Template silhouette (no black square background) so it recolors with chrome.
        "Grok" => Some(("agent-marks/grok.svg", AgentMarkStyle::Template)),
        "OpenCode" => Some(("agent-marks/opencode.svg", AgentMarkStyle::Template)),
        "Pi" => Some(("agent-marks/pi.svg", AgentMarkStyle::Template)),
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

fn brand_mark(kind: Option<&str>, mark_color: Rgba) -> AnyElement {
    match kind.and_then(agent_mark) {
        Some((path, AgentMarkStyle::Template)) => svg()
            .path(path)
            .size(px(16.0))
            .text_color(mark_color)
            .into_any_element(),
        Some((path, AgentMarkStyle::Original)) => img(path).size(px(16.0)).into_any_element(),
        None => div()
            .font_family("JetBrains Mono")
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_size(px(9.5))
            .text_color(mark_color)
            .child(">_")
            .into_any_element(),
    }
}

/// Sidebar badge: agent brand mark (or terminal glyph) plus optional status dot.
pub fn agent_sidebar_badge(
    kind: Option<&str>,
    state: Option<AgentRuntimeState>,
    attention: Option<AgentAttention>,
    selected: bool,
) -> AnyElement {
    let mark_color = if selected {
        colors().foreground
    } else {
        colors().muted
    };
    let status =
        agent_status_color(state, attention).or_else(|| selected.then_some(colors().accent));
    let needs_permission =
        attention == Some(AgentAttention::Permission) && state == Some(AgentRuntimeState::Waiting);

    div()
        .size(px(32.0))
        .flex_none()
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(8.0))
        .bg(colors().elevated)
        .when(needs_permission, |badge| {
            badge.border_1().border_color(colors().danger)
        })
        .child(
            div()
                .size(px(18.0))
                .flex()
                .items_center()
                .justify_center()
                .child(brand_mark(kind, mark_color)),
        )
        .when_some(status, |badge, color| {
            badge.child(
                div()
                    .absolute()
                    .right_0()
                    .bottom_0()
                    .size(px(6.0))
                    .rounded_full()
                    .border_1()
                    .border_color(colors().elevated)
                    .bg(color),
            )
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
            agent_mark("Grok").map(|(_, style)| style),
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
        assert!(assets.load("missing.svg").unwrap().is_none());
    }
}
