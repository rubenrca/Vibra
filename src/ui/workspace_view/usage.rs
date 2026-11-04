//! Subscription usage: a group at the start of the status bar with each
//! provider's mark and the share used of the quota closest to its limit.
//! Hovering one lists its quotas in a card above the bar, which eases in and
//! glides between providers; clicking opens a modal listing every subscription.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt, AnyElement, Bounds, Context, Corner, Div, Font, FontFeatures,
    FontStyle, FontWeight, Hsla, MouseButton, Pixels, Point, Rgba, SharedString, Stateful, Task,
    Timer, Transformation, Window, div, ease_out_quint, percentage, point, prelude::*, px,
    relative, svg,
};

use crate::domain::usage::{
    ProviderUsage, UsageSnapshot, now_timestamp, plan_label, reset_label, resource_label, timestamp,
};
use crate::infrastructure::clock::local_utc_offset;
use crate::infrastructure::usage::UsageMonitor;
use crate::ui::agent_marks::{agent_tile, icon_tile};
use crate::ui::dialog::{DIALOG_RADIUS, dialog_shadow};
use crate::ui::menu::{menu_border, menu_hover, menu_panel, menu_surface};
use crate::ui::theme::{colors, frame_hover, mix};

use super::chrome::ease_out_cubic;
use super::context_menu::MENU_EDGE_MARGIN;
use super::{WorkspaceView, sidebar_tooltip};

const POLL_INTERVAL: Duration = Duration::from_secs(30);
/// Brand marks fill their box, so they sit a step under the bar's 14 px icons.
const CHIP_MARK_SIZE: f32 = 13.0;
/// Matches the other status bar items.
const CHIP_HEIGHT: f32 = 22.0;
/// Space between the hover card or modal and the chips that opened it.
const POPOVER_GAP: f32 = 8.0;
const HOVER_CARD_WIDTH: f32 = 248.0;
const MODAL_WIDTH: f32 = 360.0;
/// Space between the modal's edge and the cards inside it.
const MODAL_INSET: f32 = 10.0;
/// Subscription cards keep their corners concentric with the modal's.
const CARD_RADIUS: f32 = DIALOG_RADIUS - MODAL_INSET / 2.0;
/// The agent tile beside each provider's name.
const HEADING_TILE_SIZE: f32 = 28.0;
const QUOTA_BAR_HEIGHT: f32 = 4.0;
/// The hover card fades in while rising this far into place.
const HOVER_CARD_RISE: f32 = 4.0;
const HOVER_CARD_ENTER: Duration = Duration::from_millis(100);
/// Quotas count up from zero when the card opens…
const HOVER_COUNT_UP: Duration = Duration::from_millis(400);
/// …and glide to the next provider's values when the pointer moves on.
const HOVER_SWITCH: Duration = Duration::from_millis(240);
/// The modal fades in while rising this far into place, like other dialogs.
const MODAL_RISE: f32 = 6.0;
const MODAL_ENTER: Duration = Duration::from_millis(200);
/// One turn of the refresh icon while quotas are being checked.
const REFRESH_SPIN: Duration = Duration::from_millis(900);

#[derive(Default)]
pub(super) struct UsageState {
    monitor: Arc<Mutex<UsageMonitor>>,
    snapshot: UsageSnapshot,
    error: Option<String>,
    loading: bool,
    /// Whether a first reading has finished; until then there is nothing to say.
    loaded: bool,
    pub open: bool,
    /// Bottom-left corner of the modal, just above the chips that opened it.
    anchor: Point<Pixels>,
    /// Provider whose chip the pointer last entered, until it leaves the group.
    hovered: Option<String>,
    /// How the hover card got to what it shows, while it shows anything.
    hover_motion: Option<HoverMotion>,
    /// Gives each hover motion an animation of its own.
    hover_motions: u64,
    /// Where the chips sit in the window, to hang the hover card and modal above them.
    tracker_bounds: Rc<Cell<Bounds<Pixels>>>,
    _poll_task: Option<Task<()>>,
    _refresh_task: Option<Task<()>>,
}

impl UsageState {
    fn apply(&mut self, result: anyhow::Result<UsageSnapshot>) {
        self.loading = false;
        self.loaded = true;
        match result {
            Ok(snapshot) => {
                self.snapshot = snapshot;
                self.error = None;
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    /// Points the hover card at `id`, or closes it with `None`. Returns
    /// whether anything changed.
    fn hover(&mut self, id: Option<&str>, now: Instant) -> bool {
        if self.hovered.as_deref() == id {
            return false;
        }
        // Moving to another provider glides from what the card shows now.
        let shown = self.hover_frame(now);
        self.hovered = id.map(str::to_owned);
        self.hover_motion = match id {
            None => None,
            Some(_) => {
                self.hover_motions += 1;
                Some(HoverMotion {
                    id: self.hover_motions,
                    opened: shown.as_ref().map_or(now, |(opened, _)| *opened),
                    changed: now,
                    from: shown.map(|(_, frame)| frame),
                })
            }
        };
        true
    }

    /// The hover card as drawn at `now`, with when it opened.
    fn hover_frame(&self, now: Instant) -> Option<(Instant, CardFrame)> {
        let motion = self.hover_motion.as_ref()?;
        let id = self.hovered.as_deref()?;
        let provider = self.snapshot.providers.get(id)?;
        let timestamp = now_timestamp();
        let card = self.card(id, provider, timestamp, local_utc_offset(timestamp));
        Some((motion.opened, motion.frame(&card, now)))
    }

    fn provider_stale(&self, id: &str, provider: &ProviderUsage, now: i64) -> bool {
        self.error.is_some()
            || provider.is_stale(now)
            || self
                .snapshot
                .errors
                .iter()
                .any(|error| error.provider_id == id)
    }

    fn card(&self, id: &str, provider: &ProviderUsage, now: i64, utc_offset: i32) -> UsageCard {
        // Bounded quotas first; balances are context, not a limit.
        let bounded = provider
            .resources
            .iter()
            .filter(|(_, resource)| resource.kind != "balance");
        let balances = provider
            .resources
            .iter()
            .filter(|(_, resource)| resource.kind == "balance");
        let rows = bounded
            .chain(balances)
            .map(|(key, resource)| UsageCardRow {
                label: resource_label(key).to_owned(),
                value: resource.compact_value_label(),
                percent: resource.percent_used(),
                reset: resource
                    .resets_at
                    .as_deref()
                    .map(|reset| reset_label(reset, now, utc_offset)),
            })
            .collect();
        let warnings = self
            .snapshot
            .errors
            .iter()
            .filter(|error| error.provider_id == id)
            .map(|error| error.message.clone())
            .collect();
        let status = if self.loading {
            Some("Checking quotas…".to_owned())
        } else {
            provider
                .fetched_at
                .as_deref()
                .and_then(timestamp)
                .map(|fetched| match now.saturating_sub(fetched).max(0) / 60 {
                    0 => "Updated just now".to_owned(),
                    minutes => format!("Updated {minutes} min ago"),
                })
        };
        UsageCard {
            id: id.to_owned(),
            name: provider.display_name.clone(),
            plan: provider.plan.as_deref().map(plan_label),
            stale: self.provider_stale(id, provider, now),
            rows,
            warnings,
            status,
        }
    }
}

impl WorkspaceView {
    pub(super) fn start_usage_poll(&mut self, cx: &mut Context<Self>) {
        // Workspace tests must not consult the user's installed apps.
        if cfg!(test) {
            return;
        }
        self.refresh_usage(false, cx);
        self.usage._poll_task = Some(cx.spawn(async move |this, cx| {
            loop {
                Timer::after(POLL_INTERVAL).await;
                if this
                    .update(cx, |this, cx| this.refresh_usage(false, cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    fn refresh_usage(&mut self, manual: bool, cx: &mut Context<Self>) {
        if self.usage.loading {
            return;
        }
        self.usage.loading = true;
        let monitor = self.usage.monitor.clone();
        cx.notify();
        self.usage._refresh_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    monitor
                        .lock()
                        .map_err(|_| anyhow::anyhow!("Could not refresh the quota monitor."))
                        .map(|mut monitor| monitor.refresh(manual))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.usage.apply(result);
                cx.notify();
            });
        }));
    }

    /// Where the hover card and the modal hang from: just above the chips,
    /// level with their left edge.
    fn usage_popover_anchor(&self) -> Point<Pixels> {
        let bounds = self.usage.tracker_bounds.get();
        point(bounds.left(), bounds.top() - px(POPOVER_GAP))
    }

    /// Opens the modal, rising from the status bar above the chips.
    fn open_usage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.usage.anchor = self.usage_popover_anchor();
        self.usage.open = true;
        self.usage.hover(None, Instant::now());
        self.context_menu = None;
        self.ide_menu_open = false;
        self.focus_handle.focus(window);
        self.refresh_usage(false, cx);
        cx.notify();
    }

    pub(super) fn close_usage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.usage.open = false;
        self.focus_selected_terminal(window, cx);
        cx.notify();
    }

    /// The status bar's usage group. Each provider shows its mark and the share
    /// used of its tightest window; hovering one lists every quota and clicking
    /// any of them opens every subscription.
    pub(super) fn usage_tracker(&self, cx: &mut Context<Self>) -> AnyElement {
        let now = now_timestamp();
        let snapshot = &self.usage.snapshot;
        div()
            .id("status-usage")
            .relative()
            .flex_none()
            .min_w(px(0.0))
            .flex()
            .items_center()
            .gap(px(2.0))
            .overflow_hidden()
            // The card stays up between chips and goes once the pointer leaves the group.
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered && this.usage.hover(None, Instant::now()) {
                    cx.notify();
                }
            }))
            .child(super::measure_bounds(self.usage.tracker_bounds.clone()))
            .when(
                snapshot.providers.is_empty() && self.usage.loaded,
                |group| {
                    group.child(
                        usage_chip("status-usage-empty".into())
                            .child(
                                svg()
                                    .path("chrome-icons/sparkles.svg")
                                    .size(px(CHIP_MARK_SIZE))
                                    .flex_none()
                                    .text_color(colors().subtle),
                            )
                            .child(div().min_w(px(0.0)).truncate().child("No usage data"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_usage(window, cx)),
                            ),
                    )
                },
            )
            .children(snapshot.providers.iter().map(|(id, provider)| {
                let stale = self.usage.provider_stale(id, provider, now);
                let percent = headline_percent(provider);
                // Only a quota close to blocking work stands out; an outdated reading fades.
                let alert = (!stale).then(|| quota_alert(percent)).flatten();
                let color = if stale {
                    colors().subtle
                } else {
                    alert.unwrap_or(colors().muted)
                };
                let hover_id = id.clone();
                usage_chip(SharedString::from(format!("status-usage-{id}")))
                    .text_color(color)
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered && this.usage.hover(Some(&hover_id), Instant::now()) {
                            cx.notify();
                        }
                    }))
                    .child(
                        svg()
                            .path(provider_mark(id))
                            .size(px(CHIP_MARK_SIZE))
                            .flex_none()
                            .text_color(color),
                    )
                    .child(
                        tabular(div())
                            .flex_none()
                            .font_weight(if alert.is_some() {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::MEDIUM
                            })
                            .child(percent_label(percent)),
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.open_usage(window, cx)))
            }))
            .into_any_element()
    }

    /// Quotas of the provider under the pointer, in a card hanging above the
    /// status bar, level with the usage group.
    pub(super) fn usage_hover_card(&self) -> Option<AnyElement> {
        if self.usage.open
            || self.context_menu.is_some()
            || self.palette_mode.is_some()
            || self.rename_prompt.is_some()
        {
            return None;
        }
        let id = self.usage.hovered.as_deref()?;
        let provider = self.usage.snapshot.providers.get(id)?;
        let motion = self.usage.hover_motion.clone()?;
        // The group has not been laid out yet.
        if self.usage.tracker_bounds.get().size.width <= px(0.0) {
            return None;
        }
        let now = now_timestamp();
        let card = self.usage.card(id, provider, now, local_utc_offset(now));
        let width = HOVER_CARD_WIDTH;
        let position = self.usage_popover_anchor();
        Some(
            div()
                .absolute()
                .inset_0()
                .child(
                    gpui::anchored()
                        .anchor(Corner::BottomLeft)
                        .position(position)
                        .snap_to_window_with_margin(px(MENU_EDGE_MARGIN))
                        // The card rises inside this shell, so the anchor never sees it move.
                        .child(div().with_animation(
                            SharedString::from(format!("usage-hover-card-{}", motion.id)),
                            Animation::new(motion.duration()),
                            move |shell, _| {
                                shell.child(usage_hover_content(
                                    &card,
                                    &motion,
                                    Instant::now(),
                                    width,
                                ))
                            },
                        )),
                )
                .into_any_element(),
        )
    }

    pub(super) fn usage_modal(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.usage.open || self.palette_mode.is_some() || self.rename_prompt.is_some() {
            return None;
        }
        let now = now_timestamp();
        let utc_offset = local_utc_offset(now);
        // It rises from its anchor, so only the room above limits its height.
        let available: f32 = self.usage.anchor.y.into();
        let snapshot = &self.usage.snapshot;
        let loading = self.usage.loading;
        let refresh_tooltip = if loading {
            "Checking quotas…"
        } else {
            "Refresh usage · May request Keychain access"
        };
        let empty = snapshot.providers.is_empty() && self.usage.error.is_none();
        let content = div()
            .id("usage-provider-list")
            .min_h(px(0.0))
            .overflow_y_scroll()
            .px(px(MODAL_INSET))
            .pb(px(MODAL_INSET))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .when_some(self.usage.error.clone(), |content, error| {
                content.child(warning_callout(None, error))
            })
            .when(empty, |content| content.child(usage_empty_state(loading)))
            .children(snapshot.providers.iter().map(|(id, provider)| {
                provider_section(&self.usage.card(id, provider, now, utc_offset))
            }))
            // Failures for a provider that has nothing to list would otherwise vanish.
            .children(
                snapshot
                    .errors
                    .iter()
                    .filter(|error| !snapshot.providers.contains_key(&error.provider_id))
                    .map(|error| {
                        warning_callout(
                            Some(&error.provider_id),
                            format!("{}: {}", provider_name(&error.provider_id), error.message),
                        )
                    }),
            );

        let header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.0))
            .pl(px(MODAL_INSET + 4.0))
            .pr(px(MODAL_INSET - 2.0))
            .pt(px(10.0))
            .pb(px(8.0))
            .child(
                div()
                    .id("usage-title")
                    .flex_1()
                    .text_size(px(14.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .tooltip(|_, cx| {
                        sidebar_tooltip("Subscription usage · Refreshes every 5 min", cx)
                    })
                    .child("Usage"),
            )
            .child(
                div()
                    .id("usage-refresh")
                    .group("usage-refresh")
                    .size(px(26.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.0))
                    .when(!loading, |button| {
                        button
                            .cursor_pointer()
                            .hover(|button| button.bg(menu_hover()))
                    })
                    .tooltip(move |_, cx| sidebar_tooltip(refresh_tooltip, cx))
                    .on_click(cx.listener(|this, _, _, cx| this.refresh_usage(true, cx)))
                    .child(refresh_icon(loading)),
            );

        // The same lifted panel as the other dialogs, with a card per subscription.
        let panel = div()
            .id("usage-modal")
            .w(px(MODAL_WIDTH))
            .max_h(px((available - MENU_EDGE_MARGIN).max(100.0)))
            .flex()
            .flex_col()
            .rounded(px(DIALOG_RADIUS))
            .border_1()
            .border_color(menu_border())
            .bg(menu_surface())
            .shadow(dialog_shadow())
            .text_size(px(12.0))
            .text_color(colors().foreground)
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(header)
            .child(content)
            // Settles into place like the other dialogs; the anchor never sees it move.
            .with_animation(
                "usage-modal-enter",
                Animation::new(MODAL_ENTER).with_easing(ease_out_quint()),
                |panel, delta| {
                    panel
                        .opacity(delta)
                        .relative()
                        .top(px(MODAL_RISE * (1.0 - delta)))
                },
            );

        Some(
            div()
                .absolute()
                .inset_0()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_usage(window, cx)),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, window, cx| this.close_usage(window, cx)),
                )
                .child(
                    gpui::anchored()
                        .anchor(Corner::BottomLeft)
                        .position(self.usage.anchor)
                        .snap_to_window_with_margin(px(MENU_EDGE_MARGIN))
                        .child(panel),
                )
                .into_any_element(),
        )
    }
}

/// One provider in the status bar, shaped like the bar's other items.
fn usage_chip(id: SharedString) -> Stateful<Div> {
    div()
        .id(id)
        .relative()
        .h(px(CHIP_HEIGHT))
        .flex_none()
        .px(px(6.0))
        .rounded(px(6.0))
        .flex()
        .items_center()
        .gap(px(5.0))
        .cursor_pointer()
        .text_size(px(11.0))
        .text_color(colors().muted)
        .hover(|chip| chip.bg(frame_hover()))
}

/// The share the status bar shows for a provider: whichever of its session
/// and weekly windows is closer to blocking work. Providers that report
/// neither show their tightest quota, or nothing without data.
fn headline_percent(provider: &ProviderUsage) -> Option<f64> {
    ["session", "weekly"]
        .into_iter()
        .filter_map(|key| provider.resources.get(key)?.percent_used())
        .max_by(f64::total_cmp)
        .or_else(|| {
            provider
                .tightest_quota()
                .and_then(|(_, resource)| resource.percent_used())
        })
}

/// A whole percentage, or a dash while the provider reports none.
fn percent_label(percent: Option<f64>) -> String {
    percent.map_or_else(|| "–".to_owned(), |percent| format!("{percent:.0}%"))
}

/// Only quotas close to blocking work stand out: in the text color as they
/// near the limit, in the danger color once it is imminent.
fn quota_alert(percent: Option<f64>) -> Option<Rgba> {
    match percent {
        Some(value) if value >= 95.0 => Some(colors().danger),
        Some(value) if value >= 80.0 => Some(colors().foreground),
        _ => None,
    }
}

/// The agent a provider's subscription is for, named as its tabs name it.
fn provider_kind(id: &str) -> Option<&'static str> {
    [("claude", "Claude"), ("codex", "Codex"), ("grok", "Grok")]
        .into_iter()
        .find(|(prefix, _)| id.starts_with(prefix))
        .map(|(_, kind)| kind)
}

fn provider_mark(id: &str) -> &'static str {
    match provider_kind(id) {
        Some("Claude") => "agent-marks/claude.svg",
        Some("Codex") => "agent-marks/codex.svg",
        Some("Grok") => "agent-marks/grok.svg",
        _ => "chrome-icons/sparkles.svg",
    }
}

/// A provider's mark on its agent's tile, as tabs show it; a provider
/// without one gets a neutral tile.
fn provider_tile(id: &str, size: f32) -> Div {
    match provider_kind(id) {
        Some(kind) => agent_tile(Some(kind), size),
        None => icon_tile("chrome-icons/sparkles.svg", size),
    }
}

#[derive(Clone, Debug, PartialEq)]
struct UsageCardRow {
    label: String,
    value: String,
    percent: Option<f64>,
    reset: Option<String>,
}

/// Everything the modal shows about one provider.
#[derive(Clone, Debug, PartialEq)]
struct UsageCard {
    id: String,
    name: String,
    plan: Option<String>,
    stale: bool,
    rows: Vec<UsageCardRow>,
    warnings: Vec<String>,
    status: Option<String>,
}

impl UsageCard {
    /// The plan and when the reading was taken, for the line under the name.
    fn detail(&self) -> Option<String> {
        let parts = [self.plan.as_deref(), self.status.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    /// Quotas close to blocking work stand out; an outdated reading fades.
    fn bar_color(&self, percent: f64) -> Rgba {
        if self.stale {
            colors().subtle
        } else {
            quota_alert(Some(percent)).unwrap_or(colors().muted)
        }
    }
}

/// A quota's consumed share over a faint track.
fn quota_bar(percent: f64, color: Rgba, height: f32) -> impl IntoElement {
    div()
        .h(px(height))
        .w_full()
        .rounded_full()
        .overflow_hidden()
        .bg(colors().border_subtle)
        .child(
            div()
                .h_full()
                .rounded_full()
                .w(relative((percent / 100.0).clamp(0.0, 1.0) as f32))
                .bg(color),
        )
}

/// When the hover card opened, when it last moved to another provider, and
/// what it showed just before.
#[derive(Clone, Debug)]
struct HoverMotion {
    id: u64,
    opened: Instant,
    changed: Instant,
    /// The card as drawn when the pointer moved to another provider; none
    /// right after opening, when every quota counts up from zero.
    from: Option<CardFrame>,
}

impl HoverMotion {
    /// How long the card keeps moving after its latest change.
    fn duration(&self) -> Duration {
        if self.from.is_some() {
            HOVER_SWITCH
        } else {
            HOVER_COUNT_UP
        }
    }

    /// How far the card has faded and risen in.
    fn entrance(&self, now: Instant) -> f32 {
        ease_out_cubic(elapsed_share(self.opened, now, HOVER_CARD_ENTER))
    }

    /// How far the quotas have moved toward the card's values.
    fn progress(&self, now: Instant) -> f32 {
        ease_out_cubic(elapsed_share(self.changed, now, self.duration()))
    }

    /// `card` as drawn at `now`.
    fn frame(&self, card: &UsageCard, now: Instant) -> CardFrame {
        let progress = self.progress(now);
        CardFrame {
            id: card.id.clone(),
            name: card.name.clone(),
            detail: card.detail(),
            rows: card
                .rows
                .iter()
                .map(|row| {
                    let previous = self.from.as_ref().and_then(|from| from.row(&row.label));
                    QuotaFrame::of(card, row).advanced_from(previous, progress)
                })
                .collect(),
        }
    }
}

/// Share of `duration` elapsed since `start`, capped at the whole of it.
fn elapsed_share(start: Instant, now: Instant, duration: Duration) -> f32 {
    (now.saturating_duration_since(start).as_secs_f32() / duration.as_secs_f32()).min(1.0)
}

/// The hover card as drawn at one instant.
#[derive(Clone, Debug, PartialEq)]
struct CardFrame {
    id: String,
    name: String,
    detail: Option<String>,
    rows: Vec<QuotaFrame>,
}

impl CardFrame {
    fn row(&self, label: &str) -> Option<&QuotaFrame> {
        self.rows.iter().find(|row| row.label == label)
    }
}

/// One quota as drawn at one instant.
#[derive(Clone, Debug, PartialEq)]
struct QuotaFrame {
    label: String,
    value: String,
    percent: Option<f64>,
    color: Rgba,
    emphasized: bool,
    reset: Option<String>,
}

impl QuotaFrame {
    /// `row` at rest.
    fn of(card: &UsageCard, row: &UsageCardRow) -> Self {
        Self {
            label: row.label.clone(),
            value: row.value.clone(),
            percent: row.percent,
            color: row
                .percent
                .map_or(colors().muted, |percent| card.bar_color(percent)),
            emphasized: !card.stale && quota_alert(row.percent).is_some(),
            reset: row.reset.clone(),
        }
    }

    /// This quota `progress` of the way from `previous`, or from zero without
    /// one. A percentage counts along with its bar.
    fn advanced_from(self, previous: Option<&QuotaFrame>, progress: f32) -> Self {
        let Some(target) = self.percent.filter(|_| progress < 1.0) else {
            return self;
        };
        let start = previous
            .and_then(|previous| previous.percent)
            .unwrap_or(0.0);
        let percent = start + (target - start) * f64::from(progress);
        let counts = self.value == format!("{target:.0}%");
        Self {
            value: if counts {
                format!("{percent:.0}%")
            } else {
                self.value
            },
            percent: Some(percent),
            color: previous.map_or(self.color, |previous| {
                mix(previous.color, self.color, progress)
            }),
            emphasized: match previous {
                Some(previous) if progress < 0.5 => previous.emphasized,
                _ => self.emphasized,
            },
            ..self
        }
    }
}

/// `text` fading in over `previous` while `progress` runs to 1.
fn crossfade(text: String, previous: Option<String>, progress: f32) -> Div {
    let previous = previous.filter(|previous| *previous != text && progress < 1.0);
    let fading = previous.is_some();
    div()
        .relative()
        .min_w(px(0.0))
        .child(
            div()
                .truncate()
                .when(fading, |current| current.opacity(progress))
                .child(text),
        )
        .when_some(previous, |label, previous| {
            label.child(
                div()
                    .absolute()
                    .inset_0()
                    .truncate()
                    .opacity(1.0 - progress)
                    .child(previous),
            )
        })
}

/// `id`'s tile fading in over `previous`'s while `progress` runs to 1.
fn tile_crossfade(id: &str, previous: Option<&str>, progress: f32, size: f32) -> Div {
    let previous = previous.filter(|previous| *previous != id && progress < 1.0);
    let fading = previous.is_some();
    div()
        .relative()
        .size(px(size))
        .flex_none()
        .child(
            div()
                .absolute()
                .inset_0()
                .when(fading, |current| current.opacity(progress))
                .child(provider_tile(id, size)),
        )
        .when_some(previous, |tiles, previous| {
            tiles.child(
                div()
                    .absolute()
                    .inset_0()
                    .opacity(1.0 - progress)
                    .child(provider_tile(previous, size)),
            )
        })
}

/// A provider's tile beside its name, with its plan and when it was read
/// below. While `progress` runs to 1 it crossfades from `from`.
fn provider_heading(card: &UsageCard, from: Option<&CardFrame>, progress: f32) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(10.0))
        .child(tile_crossfade(
            &card.id,
            from.map(|from| from.id.as_str()),
            progress,
            HEADING_TILE_SIZE,
        ))
        .child(
            div()
                .min_w(px(0.0))
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(1.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(crossfade(
                            card.name.clone(),
                            from.map(|from| from.name.clone()),
                            progress,
                        )),
                )
                .when_some(card.detail(), |text, detail| {
                    text.child(div().text_size(px(11.0)).text_color(colors().subtle).child(
                        crossfade(detail, from.and_then(|from| from.detail.clone()), progress),
                    ))
                }),
        )
        .when(card.stale, |heading| heading.child(outdated_badge()))
}

/// A reading that may no longer be current.
fn outdated_badge() -> Div {
    let warning = colors().warning;
    div()
        .flex_none()
        .h(px(18.0))
        .px(px(6.0))
        .flex()
        .items_center()
        .rounded(px(5.0))
        .bg(Rgba { a: 0.12, ..warning })
        .text_size(px(10.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(warning)
        .child("Outdated")
}

/// Where a quota says when it resets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResetPlacement {
    /// Between the label and the value, to keep a long list short.
    Inline,
    /// Under the bar, where a narrow card has room for it.
    Below,
}

/// A provider's quotas: the ones with a limit over their bars, then balances
/// and other plain values below a hairline. Resets crossfade from `from`
/// while `progress` runs to 1.
fn quota_list(
    rows: Vec<QuotaFrame>,
    from: Option<&CardFrame>,
    progress: f32,
    reset: ResetPlacement,
) -> Div {
    // Plain values trail the quotas, so the hairline goes after the last bar.
    let first_plain = rows
        .iter()
        .rposition(|row| row.percent.is_some())
        .map_or(0, |last| last + 1);
    div()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .when(rows.is_empty(), |list| {
            list.child(
                div()
                    .text_color(colors().muted)
                    .child("No quotas available"),
            )
        })
        .children(rows.into_iter().enumerate().map(|(index, row)| {
            let previous_reset = from
                .and_then(|from| from.row(&row.label))
                .and_then(|previous| previous.reset.clone());
            quota_row(row, previous_reset, progress, reset).when(
                index > 0 && index == first_plain,
                |row| {
                    row.pt(px(12.0))
                        .border_t_1()
                        .border_color(colors().border_subtle)
                },
            )
        }))
}

/// One quota: its label and value, its bar when it has a limit, and when it
/// resets. A value close to blocking work takes the bar's color.
fn quota_row(
    row: QuotaFrame,
    previous_reset: Option<String>,
    progress: f32,
    placement: ResetPlacement,
) -> Div {
    let reset = row.reset.map(|reset| {
        div()
            .min_w(px(0.0))
            .text_size(px(11.0))
            .text_color(colors().subtle)
            .child(crossfade(reset, previous_reset, progress))
    });
    let (inline_reset, reset_below) = match placement {
        ResetPlacement::Inline => (reset, None),
        ResetPlacement::Below => (None, reset),
    };
    let inline = inline_reset.is_some();
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .min_w(px(0.0))
                        .when(!inline, |label| label.flex_1())
                        .truncate()
                        .text_color(colors().muted)
                        .child(row.label),
                )
                .when_some(inline_reset, |line, reset| line.child(reset.flex_1()))
                .child(
                    tabular(div())
                        .flex_none()
                        .font_weight(if row.emphasized {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::MEDIUM
                        })
                        .text_color(if row.emphasized {
                            row.color
                        } else {
                            colors().foreground
                        })
                        .child(row.value),
                ),
        )
        .when_some(row.percent, |quota, percent| {
            quota.child(quota_bar(percent, row.color, QUOTA_BAR_HEIGHT))
        })
        .when_some(reset_below, |quota, reset| quota.child(reset))
}

fn provider_warning(warning: &str) -> Div {
    div()
        .text_size(px(11.5))
        .text_color(colors().warning)
        .child(warning.to_owned())
}

/// A tinted note for a failure, with the provider's tile when it has one.
fn warning_callout(provider: Option<&str>, message: impl Into<SharedString>) -> Div {
    let warning = colors().warning;
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(8.0))
        .px(px(10.0))
        .py(px(8.0))
        .rounded(px(CARD_RADIUS))
        .border_1()
        .border_color(Rgba { a: 0.25, ..warning })
        .bg(Rgba { a: 0.08, ..warning })
        .text_color(warning)
        .when_some(provider, |callout, id| {
            callout.child(provider_tile(id, 18.0))
        })
        .child(div().min_w(px(0.0)).flex_1().child(message.into()))
}

/// What the modal shows before any subscription has reported.
fn usage_empty_state(loading: bool) -> Div {
    div()
        .flex_none()
        .py(px(20.0))
        .px(px(12.0))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(8.0))
        .child(icon_tile("chrome-icons/sparkles.svg", 32.0))
        .child(
            div()
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .child(if loading {
                    "Checking quotas…"
                } else {
                    "No subscriptions found"
                }),
        )
        .when(!loading, |state| {
            state.child(
                div()
                    .text_center()
                    .text_color(colors().muted)
                    .child("Sign in with Claude Code, Codex, or Grok Build, then refresh."),
            )
        })
}

/// The hover card at `now`: the provider's heading, then each quota as label
/// and value over its bar and reset time. It fades and rises in as it opens,
/// counts its quotas up, and glides between providers.
fn usage_hover_content(
    card: &UsageCard,
    motion: &HoverMotion,
    now: Instant,
    width: f32,
) -> impl IntoElement + use<> {
    let entrance = motion.entrance(now);
    let progress = motion.progress(now);
    let from = motion.from.as_ref();
    let frame = motion.frame(card, now);
    menu_panel()
        .id("usage-hover-card")
        .relative()
        .top(px(HOVER_CARD_RISE * (1.0 - entrance)))
        .opacity(entrance)
        .min_w(px(0.0))
        .w(px(width))
        .p(px(14.0))
        .gap(px(14.0))
        .rounded(px(12.0))
        .shadow(dialog_shadow())
        .text_size(px(12.0))
        .text_color(colors().foreground)
        .child(provider_heading(card, from, progress))
        .child(quota_list(
            frame.rows,
            from,
            progress,
            ResetPlacement::Below,
        ))
        .children(
            card.warnings
                .iter()
                .map(|warning| provider_warning(warning)),
        )
}

/// One subscription in the modal: a card with its heading and quotas.
fn provider_section(card: &UsageCard) -> impl IntoElement + use<> {
    let rows = card
        .rows
        .iter()
        .map(|row| QuotaFrame::of(card, row))
        .collect();
    div()
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(14.0))
        .p(px(12.0))
        .rounded(px(CARD_RADIUS))
        .border_1()
        .border_color(foreground(0.07))
        .bg(foreground(0.03))
        .child(provider_heading(card, None, 1.0))
        .child(quota_list(rows, None, 1.0, ResetPlacement::Inline))
        .children(
            card.warnings
                .iter()
                .map(|warning| provider_warning(warning)),
        )
}

/// The foreground at `alpha`, for fills and strokes that tint whatever is below.
fn foreground(alpha: f32) -> Hsla {
    Hsla::from(colors().foreground).opacity(alpha)
}

/// The interface font with tabular figures, so values hold their width as
/// they count up and line up from one quota to the next.
fn tabular(element: Div) -> Div {
    element.font(Font {
        family: ".SystemUIFont".into(),
        features: FontFeatures(Arc::new(vec![("tnum".into(), 1)])),
        fallbacks: None,
        weight: FontWeight::NORMAL,
        style: FontStyle::Normal,
    })
}

/// The refresh glyph, turning while quotas are being checked.
fn refresh_icon(loading: bool) -> AnyElement {
    let icon = svg()
        .path("chrome-icons/refresh.svg")
        .size(px(13.0))
        .text_color(colors().muted)
        .group_hover("usage-refresh", |icon| icon.text_color(colors().foreground));
    if loading {
        icon.with_animation(
            "usage-refresh-spin",
            Animation::new(REFRESH_SPIN).repeat(),
            |icon, delta| icon.with_transformation(Transformation::rotate(percentage(delta))),
        )
        .into_any_element()
    } else {
        icon.into_any_element()
    }
}

/// A provider's name when it only reported a failure, without a reading to carry one.
fn provider_name(id: &str) -> &str {
    provider_kind(id).unwrap_or(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(json: &str) -> UsageSnapshot {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn card_lists_windows_like_the_provider_reports_them() {
        let mut state = UsageState::default();
        state.apply(Ok(snapshot(
            r#"{"providers":{"codex":{
                "displayName":"Codex","plan":"plus",
                "fetchedAt":"2026-09-26T11:57:00Z",
                "expiresAt":"2026-09-26T12:02:00Z",
                "resources":{
                    "credits":{"kind":"balance","unit":"credits","available":40},
                    "session":{"kind":"consumption","unit":"percent","used":84,"limit":100,
                               "resetsAt":"2026-09-26T18:12:00Z"},
                    "weekly":{"kind":"consumption","unit":"percent","used":27,"limit":100,
                              "resetsAt":"2026-09-30T15:00:00Z"}
                }}}}"#,
        )));
        let now = timestamp("2026-09-26T12:00:00Z").unwrap();
        let provider = &state.snapshot.providers["codex"];
        let card = state.card("codex", provider, now, 0);
        let row = |label: &str, value: &str, percent, reset: Option<&str>| UsageCardRow {
            label: label.into(),
            value: value.into(),
            percent,
            reset: reset.map(str::to_owned),
        };
        assert_eq!(
            card.rows,
            [
                row("5 hours", "84%", Some(84.0), Some("Resets 6:12 PM")),
                row("Week", "27%", Some(27.0), Some("Resets Wednesday")),
                // Balances follow the bounded quotas, whatever their key sorts as.
                row("Credits", "40 credits", None, None),
            ]
        );
        assert_eq!(card.plan.as_deref(), Some("Plus"));
        assert!(!card.stale);
        assert!(card.warnings.is_empty());
        assert_eq!(card.status.as_deref(), Some("Updated 3 min ago"));
        // The line under the name carries both; without either there is none.
        assert_eq!(card.detail().as_deref(), Some("Plus · Updated 3 min ago"));
        let bare = UsageCard {
            plan: None,
            status: None,
            ..card
        };
        assert_eq!(bare.detail(), None);
    }

    #[test]
    fn providers_borrow_their_agent_tile_and_name() {
        assert_eq!(provider_kind("claude"), Some("Claude"));
        assert_eq!(provider_kind("codex-enterprise"), Some("Codex"));
        assert_eq!(provider_kind("grok"), Some("Grok"));
        assert_eq!(provider_kind("copilot"), None);
        assert_eq!(provider_name("codex"), "Codex");
        assert_eq!(provider_name("copilot"), "copilot");
        assert_eq!(provider_mark("copilot"), "chrome-icons/sparkles.svg");
    }

    #[test]
    fn card_flags_outdated_data_and_carries_the_provider_error() {
        let mut state = UsageState::default();
        state.apply(Ok(snapshot(
            r#"{"providers":{"claude":{"displayName":"Claude","resources":{}}},
                "errors":[{"providerId":"claude","message":"Rate limited"},
                          {"providerId":"codex","message":"Elsewhere"}]}"#,
        )));
        let provider = &state.snapshot.providers["claude"];
        let card = state.card("claude", provider, now_timestamp(), 0);
        assert!(card.stale);
        assert!(card.rows.is_empty());
        assert_eq!(card.warnings, ["Rate limited"]);
        assert_eq!(card.status, None);
        state.loading = true;
        assert_eq!(
            state
                .card("claude", provider, now_timestamp(), 0)
                .status
                .as_deref(),
            Some("Checking quotas…")
        );
    }

    #[test]
    fn headline_follows_the_tighter_window_with_a_fallback() {
        let provider = |resources: &str| -> ProviderUsage {
            serde_json::from_str(&format!(r#"{{"displayName":"P","resources":{resources}}}"#))
                .unwrap()
        };
        let quota = |used: u32| {
            format!(r#"{{"kind":"consumption","unit":"percent","used":{used},"limit":100}}"#)
        };
        // Both windows: the one closer to its limit, ignoring model scopes.
        assert_eq!(
            headline_percent(&provider(&format!(
                r#"{{"opus":{},"weekly":{},"session":{}}}"#,
                quota(90),
                quota(40),
                quota(20)
            ))),
            Some(40.0)
        );
        assert_eq!(
            headline_percent(&provider(&format!(
                r#"{{"weekly":{},"session":{}}}"#,
                quota(12),
                quota(63)
            ))),
            Some(63.0)
        );
        // Without either, the tightest quota stands in for them.
        assert_eq!(
            headline_percent(&provider(&format!(r#"{{"monthly":{}}}"#, quota(65)))),
            Some(65.0)
        );
        // No data still leaves the provider visible, with a dash.
        assert_eq!(headline_percent(&provider("{}")), None);
        assert_eq!(percent_label(None), "–");
        assert_eq!(percent_label(Some(37.6)), "38%");
    }

    #[gpui::test]
    fn usage_tracker_builds_in_every_state(cx: &mut gpui::TestAppContext) {
        let (root, _, _, window) =
            super::super::tests::open_recording_workspace(cx, "usage-tracker");
        window
            .update(cx, |view, window, cx| {
                // Before the first reading, and again once it found nothing.
                for loaded in [false, true] {
                    view.usage.loaded = loaded;
                    for loading in [false, true] {
                        view.usage.loading = loading;
                        view.usage_tracker(cx);
                    }
                }
                view.usage.error = Some("Could not refresh the quota monitor.".into());
                view.usage_tracker(cx);
                view.usage.error = None;
                view.usage.snapshot = snapshot(
                    r#"{"providers":{"claude":{"displayName":"Claude","resources":{}},
                                     "codex":{"displayName":"Codex","resources":{}}}}"#,
                );
                view.usage_tracker(cx);
                window.remove_window();
            })
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn hover_card_counts_up_then_glides_between_providers() {
        let mut state = UsageState::default();
        state.apply(Ok(snapshot(
            r#"{"providers":{
                "claude":{"displayName":"Claude","resources":{
                    "session":{"kind":"consumption","unit":"percent","used":38,"limit":100},
                    "credits":{"kind":"balance","unit":"credits","available":40}}},
                "codex":{"displayName":"Codex","resources":{
                    "session":{"kind":"consumption","unit":"percent","used":84,"limit":100}}}}}"#,
        )));
        let shown = |state: &UsageState, now| state.hover_frame(now).unwrap().1;
        let opened = Instant::now();
        assert!(state.hover(Some("claude"), opened));
        assert!(!state.hover(Some("claude"), opened));

        // Opening counts each percentage up from zero and leaves other values alone.
        let counting = shown(&state, opened + HOVER_COUNT_UP / 4);
        let percent = counting.rows[0].percent.unwrap();
        assert!(percent > 0.0 && percent < 38.0);
        assert_eq!(counting.rows[0].value, format!("{percent:.0}%"));
        assert_eq!(counting.rows[1].value, "40 credits");
        assert_eq!(shown(&state, opened + HOVER_COUNT_UP).rows[0].value, "38%");

        // Moving on glides from what the card showed, which keeps it open.
        let switched = opened + HOVER_COUNT_UP;
        assert!(state.hover(Some("codex"), switched));
        let motion = state.hover_motion.clone().unwrap();
        assert_eq!(motion.opened, opened);
        assert_eq!(motion.from.map(|from| from.name), Some("Claude".to_owned()));
        let gliding = shown(&state, switched + HOVER_SWITCH / 4).rows[0]
            .percent
            .unwrap();
        assert!(gliding > 38.0 && gliding < 84.0);
        let settled = shown(&state, switched + HOVER_SWITCH);
        assert_eq!(settled.rows[0].value, "84%");
        assert_eq!(settled.rows[0].color, colors().foreground);
        assert!(settled.rows[0].emphasized);

        // Leaving the group closes the card at once.
        assert!(state.hover(None, switched));
        assert_eq!(state.hovered, None);
        assert!(state.hover_motion.is_none());
        assert!(!state.hover(None, switched));
    }

    #[test]
    fn only_quotas_near_their_limit_stand_out() {
        assert_eq!(quota_alert(None), None);
        assert_eq!(quota_alert(Some(79.0)), None);
        assert_eq!(quota_alert(Some(84.0)), Some(colors().foreground));
        assert_eq!(quota_alert(Some(97.0)), Some(colors().danger));
    }

    #[gpui::test]
    fn usage_hover_card_hangs_above_the_status_bar_for_the_hovered_provider(
        cx: &mut gpui::TestAppContext,
    ) {
        let (root, _, _, window) = super::super::tests::open_recording_workspace(cx, "usage-hover");
        window
            .update(cx, |view, window, cx| {
                view.usage.snapshot = snapshot(
                    r#"{"providers":{"codex":{"displayName":"Codex","resources":{
                        "session":{"kind":"consumption","unit":"percent","used":84,"limit":100}
                    }}}}"#,
                );
                view.usage.tracker_bounds.set(Bounds::new(
                    point(px(6.0), px(904.0)),
                    gpui::size(px(180.0), px(22.0)),
                ));
                // Nothing hovered, a provider the snapshot lacks, then the real one.
                assert!(view.usage_hover_card().is_none());
                view.usage.hover(Some("grok"), Instant::now());
                assert!(view.usage_hover_card().is_none());
                view.usage.hover(Some("codex"), Instant::now());
                assert!(view.usage_hover_card().is_some());
                // The group is not measured until it has been laid out.
                view.usage.tracker_bounds.set(Bounds::default());
                assert!(view.usage_hover_card().is_none());
                view.usage.tracker_bounds.set(Bounds::new(
                    point(px(6.0), px(904.0)),
                    gpui::size(px(180.0), px(22.0)),
                ));
                // The modal takes over from the card, and opening it clears the hover.
                view.usage.open = true;
                assert!(view.usage_hover_card().is_none());
                view.usage.loading = true;
                view.open_usage(window, cx);
                assert_eq!(view.usage.hovered, None);
                assert!(view.usage.hover_motion.is_none());
                window.remove_window();
            })
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[gpui::test]
    fn usage_modal_can_open_during_refresh(cx: &mut gpui::TestAppContext) {
        let (root, _, _, window) =
            super::super::tests::open_recording_workspace(cx, "usage-refresh");
        window
            .update(cx, |view, window, cx| {
                assert!(view.usage_modal(cx).is_none());
                view.usage.open = true;
                for loading in [false, true, false] {
                    view.usage.loading = loading;
                    assert!(view.usage_modal(cx).is_some());
                }
                view.usage.snapshot = snapshot(
                    r#"{"providers":{"codex":{"displayName":"Codex","resources":{}}},
                        "errors":[{"providerId":"grok","message":"Signed out"}]}"#,
                );
                assert!(view.usage_modal(cx).is_some());
                // Quotas with bars and resets, a balance below them, and a monitor failure.
                view.usage.snapshot = snapshot(
                    r#"{"providers":{"claude":{"displayName":"Claude","plan":"max",
                        "resources":{
                            "session":{"kind":"consumption","unit":"percent","used":97,
                                       "limit":100,"resetsAt":"2026-09-26T18:12:00Z"},
                            "credits":{"kind":"balance","unit":"credits","available":40}}}}}"#,
                );
                view.usage.error = Some("Could not refresh the quota monitor.".into());
                assert!(view.usage_modal(cx).is_some());
                view.close_usage(window, cx);
                assert!(view.usage_modal(cx).is_none());
                window.remove_window();
            })
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[gpui::test]
    fn usage_modal_rises_from_the_status_bar_chips(cx: &mut gpui::TestAppContext) {
        let (root, _, _, window) =
            super::super::tests::open_recording_workspace(cx, "usage-anchor");
        window
            .update(cx, |view, window, cx| {
                // A refresh in flight keeps the test away from the user's real logins.
                view.usage.loading = true;
                view.usage.tracker_bounds.set(Bounds::new(
                    point(px(6.0), px(1480.0)),
                    gpui::size(px(180.0), px(22.0)),
                ));
                view.open_usage(window, cx);
                assert!(view.usage.open);
                assert_eq!(view.usage.anchor, point(px(6.0), px(1480.0 - POPOVER_GAP)));
                assert!(view.usage_modal(cx).is_some());
                window.remove_window();
            })
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn connection_failure_retains_values_and_recovery_replaces_them() {
        let snapshot = serde_json::from_str(
            r#"{"providers":{"codex":{"displayName":"Codex","resources":{}}}}"#,
        )
        .unwrap();
        let mut state = UsageState::default();
        assert!(!state.loaded);
        state.apply(Ok(snapshot));
        assert!(state.loaded);
        state.apply(Err(anyhow::anyhow!("Sin conexión")));
        assert!(state.provider_stale("codex", &state.snapshot.providers["codex"], now_timestamp()));
        assert_eq!(state.snapshot.providers.len(), 1);
        state.apply(Ok(UsageSnapshot::default()));
        assert!(state.snapshot.providers.is_empty());
        assert!(state.error.is_none());
        assert!(!state.loading);
    }
}
