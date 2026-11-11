use std::borrow::Cow;

use gpui::{
    AnyElement, AssetSource, IntoElement, ParentElement, Rgba, SharedString, Styled, div, img,
    prelude::*, px, svg,
};

use crate::infrastructure::automation::{AgentAttention, AgentRuntimeState};
use crate::ui::theme::{colors, frame, lift, mix};

macro_rules! bundled_assets {
    ($(($key:literal, $rel:literal)),+ $(,)?) => {
        impl AssetSource for VibraAssets {
            fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
                Ok(match path {
                    $(
                        $key => Some(Cow::Borrowed(include_bytes!(concat!(
                            "../../Resources/",
                            $rel
                        )))),
                    )+
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
    ("chrome-icons/copy.svg", "ChromeIcons/copy.svg"),
    ("chrome-icons/coffee.svg", "ChromeIcons/coffee.svg"),
    ("chrome-icons/coffee-steam.svg", "ChromeIcons/coffee-steam.svg"),
    ("chrome-icons/git-merge.svg", "ChromeIcons/git-merge.svg"),
    ("chrome-icons/issue-canceled.svg", "ChromeIcons/issue-canceled.svg"),
    ("chrome-icons/issue-closed.svg", "ChromeIcons/issue-closed.svg"),
    ("chrome-icons/issue-open.svg", "ChromeIcons/issue-open.svg"),
    ("chrome-icons/filter.svg", "ChromeIcons/filter.svg"),
    ("chrome-icons/github.svg", "ChromeIcons/github.svg"),
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
    ("chrome-icons/terminal.svg", "ChromeIcons/terminal.svg"),
    ("chrome-icons/prompt.svg", "ChromeIcons/prompt.svg"),
    ("chrome-icons/folder.svg", "ChromeIcons/folder.svg"),
    ("chrome-icons/plus.svg", "ChromeIcons/plus.svg"),
    ("chrome-icons/ellipsis.svg", "ChromeIcons/ellipsis.svg"),
    ("chrome-icons/git-branch.svg", "ChromeIcons/git-branch.svg"),
    ("chrome-icons/open-external.svg", "ChromeIcons/open-external.svg"),
    ("chrome-icons/chevron-down.svg", "ChromeIcons/chevron-down.svg"),
    ("chrome-icons/chevron-up.svg", "ChromeIcons/chevron-up.svg"),
    ("chrome-icons/fold-vertical.svg", "ChromeIcons/fold-vertical.svg"),
    ("chrome-icons/unfold-vertical.svg", "ChromeIcons/unfold-vertical.svg"),
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
    ("chrome-icons/palette.svg", "ChromeIcons/palette.svg"),
    ("chrome-icons/shield.svg", "ChromeIcons/shield.svg"),
    ("chrome-icons/sun.svg", "ChromeIcons/sun.svg"),
    ("chrome-icons/moon.svg", "ChromeIcons/moon.svg"),
    ("chrome-icons/settings.svg", "ChromeIcons/settings.svg"),
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
    ("chrome-icons/panel-left.svg", "ChromeIcons/panel-left.svg"),
    ("chrome-icons/panel-right.svg", "ChromeIcons/panel-right.svg"),
    ("chrome-icons/pin.svg", "ChromeIcons/pin.svg"),
    ("chrome-icons/pin-off.svg", "ChromeIcons/pin-off.svg"),
    ("chrome-icons/pencil.svg", "ChromeIcons/pencil.svg"),
    ("chrome-icons/folder-open.svg", "ChromeIcons/folder-open.svg"),
    ("chrome-icons/trash.svg", "ChromeIcons/trash.svg"),
    ("chrome-icons/rows.svg", "ChromeIcons/rows.svg"),
    ("chrome-icons/worktree.svg", "ChromeIcons/worktree.svg"),
    ("chrome-icons/repository.svg", "ChromeIcons/repository.svg"),
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
            .size(px(size * scale * 0.75))
            .flex_none()
            .text_color(mark_color)
            .into_any_element(),
        Some((path, AgentMarkStyle::Original, scale)) => img(path)
            .size(px(size * scale * 0.75))
            .flex_none()
            .into_any_element(),
        None => svg()
            .path("chrome-icons/terminal.svg")
            .size(px(size))
            .flex_none()
            .text_color(mark_color)
            .into_any_element(),
    }
}

/// Marks whose identity is their color keep it in the chrome.
fn brand_tint(kind: &str) -> Option<Rgba> {
    match kind {
        "Claude" => Some(gpui::rgb(0xd97757)),
        _ => None,
    }
}

fn badge_mark_color(selected: bool) -> Rgba {
    if selected {
        colors().foreground
    } else {
        colors().muted
    }
}

/// Tile colors for marks that read as an app icon on a brand background.
fn tile_color(kind: &str) -> Option<Rgba> {
    match kind {
        "Gemini" => Some(gpui::rgb(0x3d7bf5)),
        "Codex" | "Cursor" | "Grok" | "OpenCode" => Some(gpui::rgb(0x1c1c1f)),
        _ => brand_tint(kind),
    }
}

/// A tile in `color` with the brand tiles' sheen and rim, holding `mark`.
/// Spaces use it so their marks sit beside the agents' as the same object.
pub fn brand_tile(color: Rgba, size: f32, mark: AnyElement) -> gpui::Div {
    tile(Some(color), size, mark)
}

/// A rounded tile, like an app icon, holding `mark`. Brand tiles get a soft
/// top-down sheen and a light rim; neutral tiles stay quiet on any surface.
fn tile(brand: Option<Rgba>, size: f32, mark: AnyElement) -> gpui::Div {
    let tile = div()
        .size(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(size * 0.3))
        .border_1();
    match brand {
        Some(color) => tile
            .bg(gpui::linear_gradient(
                180.0,
                gpui::linear_color_stop(
                    crate::ui::theme::mix(color, gpui::rgb(0xffffff), 0.16),
                    0.0,
                ),
                gpui::linear_color_stop(color, 1.0),
            ))
            .border_color(gpui::Hsla::from(gpui::rgb(0xffffff)).opacity(0.18)),
        // Opaque, so the tiles stacked behind never show through it.
        None => tile
            .bg(mix(lift(frame(), 1.0), colors().foreground, 0.08))
            .border_color(gpui::Hsla::from(colors().foreground).opacity(0.10)),
    }
    .child(mark)
}

/// An agent's mark on a tile of its own; a terminal without one gets a
/// neutral tile with a bare prompt glyph, since the tile is already its frame.
pub fn agent_tile(kind: Option<&str>, size: f32) -> gpui::Div {
    let brand = kind.and_then(tile_color);
    let mark = match kind.and_then(agent_mark) {
        Some((path, AgentMarkStyle::Template, scale)) => svg()
            .path(path)
            .size(px(size * scale * 0.62))
            .flex_none()
            .text_color(if brand.is_some() {
                gpui::rgb(0xffffff)
            } else {
                colors().foreground
            })
            .into_any_element(),
        Some((path, AgentMarkStyle::Original, scale)) => img(path)
            .size(px(size * scale * 0.66))
            .flex_none()
            .into_any_element(),
        None => svg()
            .path("chrome-icons/prompt.svg")
            .size(px(size * 0.7))
            .flex_none()
            .text_color(colors().muted)
            .into_any_element(),
    };
    tile(brand, size, mark)
}

/// A neutral tile around a chrome icon, for tabs that hold no terminal.
pub fn icon_tile(path: &'static str, size: f32) -> gpui::Div {
    tile(
        None,
        size,
        svg()
            .path(path)
            .size(px(size * 0.6))
            .flex_none()
            .text_color(colors().muted)
            .into_any_element(),
    )
}

/// Tiles a stack shows: the front one and up to two peeking out behind it.
const STACK_DEPTH: usize = 3;
/// How far each tile behind peeks out past the one in front of it.
const STACK_PEEK: f32 = 4.0;
/// How much smaller each tile behind is than the one in front of it.
const STACK_SHRINK: f32 = 3.0;
/// How high the leading tile arcs, against its size, on its way to the back.
const STACK_LIFT: f32 = 0.4;

/// A tile placed in a stack. Slot 0 is the front and 1 and 2 sit behind it;
/// a fractional slot is on its way between them. `lift` raises it.
struct PlacedTile<'a> {
    kind: Option<&'a str>,
    slot: f32,
    lift: f32,
}

/// Draws `tiles` back to front, `count` slots wide, with the status dot on
/// the front slot ringed in `surface`.
fn placed_stack(
    mut tiles: Vec<PlacedTile<'_>>,
    count: usize,
    size: f32,
    status: Option<(Rgba, f32)>,
    surface: gpui::Hsla,
) -> AnyElement {
    tiles.sort_by(|a, b| b.slot.total_cmp(&a.slot));
    let last = (count.max(1) - 1) as f32;
    let tiles = tiles.into_iter().map(|tile| {
        let tile_size = size - STACK_SHRINK * tile.slot;
        // Tiles behind are a little dimmer; one past the last slot fades out.
        let opacity = (1.0 - 0.2 * tile.slot.min(1.0)) * (1.0 + last - tile.slot).clamp(0.0, 1.0);
        agent_tile(tile.kind, tile_size)
            .absolute()
            .left(px(size + STACK_PEEK * tile.slot - tile_size))
            .top(px((size - tile_size) / 2.0 - tile.lift))
            .opacity(opacity)
    });
    div()
        .relative()
        .flex_none()
        .w(px(size + STACK_PEEK * last))
        .h(px(size))
        .children(tiles)
        .when_some(status, |stack, (color, opacity)| {
            stack.child(
                div()
                    .absolute()
                    .top(px(-2.0))
                    .left(px(size - 6.0))
                    .size(px(8.0))
                    .rounded_full()
                    .border_2()
                    .border_color(surface)
                    .bg(color)
                    .opacity(opacity),
            )
        })
        .into_any_element()
}

/// The dot for an agent's state; an idle agent shows none.
fn stack_status(
    state: Option<AgentRuntimeState>,
    attention: Option<AgentAttention>,
) -> Option<Rgba> {
    match state {
        Some(AgentRuntimeState::Idle) | None => None,
        _ => agent_status_color(state, attention),
    }
}

/// A tab's panes as a stack of tiles: the focused one in front, up to two
/// more peeking out behind it. A dot on the front tile tells what its agent
/// is doing, ringed in `surface` (the color the tab sits on).
pub fn agent_tile_stack(
    kinds: &[Option<String>],
    state: Option<AgentRuntimeState>,
    attention: Option<AgentAttention>,
    surface: impl Into<gpui::Hsla>,
    size: f32,
) -> AnyElement {
    let count = kinds.len().clamp(1, STACK_DEPTH);
    let tiles = (0..count)
        .map(|index| PlacedTile {
            kind: kinds.get(index).and_then(|kind| kind.as_deref()),
            slot: index as f32,
            lift: 0.0,
        })
        .collect();
    let status = stack_status(state, attention).map(|color| (color, 1.0));
    placed_stack(tiles, count, size, status, surface.into())
}

/// The stack `progress` (0 to 1) of the way from `from` to `to`, each a
/// tab's panes front first. Every tile slides from its old slot to its new
/// one, so the pane taking focus comes forward while the one that led arcs
/// up and over the others on its way back. A pane new to the stack rises in
/// from behind it.
pub fn agent_tile_stack_shuffle<Id: PartialEq>(
    from: &[(Id, Option<String>)],
    to: &[(Id, Option<String>)],
    progress: f32,
    state: Option<AgentRuntimeState>,
    attention: Option<AgentAttention>,
    surface: impl Into<gpui::Hsla>,
    size: f32,
) -> AnyElement {
    let count = to.len().clamp(1, STACK_DEPTH);
    let behind = STACK_DEPTH as f32;
    let tiles = to
        .iter()
        .take(count)
        .enumerate()
        .map(|(index, (id, kind))| {
            let end = index as f32;
            let start = from
                .iter()
                .position(|(previous, _)| previous == id)
                .map_or(behind, |slot| (slot as f32).min(behind));
            let leaving_front = start == 0.0 && end > 0.0;
            PlacedTile {
                kind: kind.as_deref(),
                slot: start + (end - start) * progress,
                lift: if leaving_front {
                    STACK_LIFT * size * (std::f32::consts::PI * progress).sin()
                } else {
                    0.0
                },
            }
        })
        .collect();
    let status = stack_status(state, attention).map(|color| (color, progress));
    placed_stack(tiles, count, size, status, surface.into())
}

/// Compact mark + runtime state used by pane headers and other dense chrome.
pub fn agent_compact_badge(
    kind: Option<&str>,
    state: Option<AgentRuntimeState>,
    attention: Option<AgentAttention>,
    selected: bool,
) -> AnyElement {
    let mark_color = kind
        .and_then(brand_tint)
        .unwrap_or_else(|| badge_mark_color(selected));
    let status = agent_status_color(state, attention);

    div()
        .h(px(18.0))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(4.0))
        .child(
            div()
                .size(px(16.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(brand_mark(kind, mark_color, 16.0)),
        )
        .when_some(status, |badge, color| {
            badge.child(div().size(px(4.0)).flex_none().rounded_full().bg(color))
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
