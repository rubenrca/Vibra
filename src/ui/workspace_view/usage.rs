//! Subscription usage: a group in the middle of the status bar with each
//! provider's mark and the share used of the quota closest to its limit.
//! Hovering one lists its quotas in a card above the bar, which eases in and
//! glides between providers; clicking opens Activity, which lists every
//! subscription in full.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt, AnyElement, Bounds, Context, Corner, Div, Font, FontFeatures,
    FontStyle, FontWeight, Hsla, Pixels, Point, Rgba, SharedString, Stateful, Task, Timer,
    Transformation, Window, div, percentage, point, prelude::*, px, relative, svg,
};

use crate::domain::usage::{
    ProviderUsage, UsageSnapshot, now_timestamp, plan_label, reset_label, resource_label, timestamp,
};
use crate::infrastructure::clock::local_utc_offset;
use crate::infrastructure::usage::UsageMonitor;
use crate::ui::agent_marks::{agent_tile, icon_tile};
use crate::ui::menu::menu_panel;
use crate::ui::theme::{colors, mix};

use super::chrome::ease_out_cubic;
use super::context_menu::MENU_EDGE_MARGIN;
use super::{WorkspaceSection, WorkspaceView, sidebar_tooltip};

const POLL_INTERVAL: Duration = Duration::from_secs(30);
/// The provider marks lead the strip, a step over the bar's 14 px icons.
const CHIP_MARK_SIZE: f32 = 15.0;
/// Matches the other status bar items.
const CHIP_HEIGHT: f32 = super::status_bar::STATUS_ITEM_HEIGHT;
const USAGE_CHIP_GROUP: &str = "status-usage-chip";
/// Space between the hover card and the chips it hangs over.
const POPOVER_GAP: f32 = 8.0;
const HOVER_CARD_WIDTH: f32 = 236.0;
/// The hover card keeps this far inside the strip that holds the chips, so
/// in a sidebar it never reaches over the pane beside it.
const POPOVER_INSET: f32 = 8.0;
/// It never gets narrower than this, however narrow the strip.
const POPOVER_MIN_WIDTH: f32 = 160.0;
/// Padding of the hover card.
const CARD_PADDING: f32 = 12.0;
/// A quota needs this much width to name its reset beside its value; with
/// less, the reset goes under the bar.
const INLINE_RESET_WIDTH: f32 = 200.0;
/// The card opens once the pointer rests on a chip this long, so crossing
/// the status bar does not flash it over the terminal.
const HOVER_CARD_DELAY: Duration = Duration::from_millis(350);
/// A subscription's card in Activity: they share a row while each has this
/// much, and one alone does not stretch its bars across the page.
const SECTION_CARD_MIN_WIDTH: f32 = 260.0;
const SECTION_CARD_MAX_WIDTH: f32 = 420.0;
/// Matches Activity's tiles.
const SECTION_CARD_RADIUS: f32 = 12.0;
/// The agent tile beside each provider's name.
const HEADING_TILE_SIZE: f32 = 24.0;
const QUOTA_BAR_HEIGHT: f32 = 4.0;
/// Percentages end on one edge and share this width, so they read as a column.
const QUOTA_VALUE_WIDTH: f32 = 34.0;
/// The hover card fades in while rising this far into place.
const HOVER_CARD_RISE: f32 = 4.0;
const HOVER_CARD_ENTER: Duration = Duration::from_millis(100);
/// Quotas count up from zero when the card opens…
const HOVER_COUNT_UP: Duration = Duration::from_millis(400);
/// …and glide to the next provider's values when the pointer moves on.
const HOVER_SWITCH: Duration = Duration::from_millis(240);
/// One turn of the refresh icon while quotas are being checked.
const REFRESH_SPIN: Duration = Duration::from_millis(900);

#[derive(Default)]
pub(super) struct UsageState {
    monitor: Arc<Mutex<UsageMonitor>>,
    snapshot: UsageSnapshot,
    error: Option<String>,
    loading: bool,
    /// Whether a reading the user waits for is in flight: the first one, or
    /// one they asked for. The poll's own readings change nothing on screen.
    checking: bool,
    /// Whether a first reading has finished; until then there is nothing to say.
    loaded: bool,
    /// Provider whose chip the pointer last entered, until it leaves the group.
    hovered: Option<String>,
    /// How the hover card got to what it shows, while it shows anything.
    hover_motion: Option<HoverMotion>,
    /// Gives each hover motion an animation of its own.
    hover_motions: u64,
    /// Bumped by every chip the pointer enters and by leaving the group, so
    /// a delayed opening only lands for the chip still under the pointer.
    hover_intent: u64,
    _hover_task: Option<Task<()>>,
    /// Where the chips sit in the window, to hang the hover card above them.
    tracker_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// The strip the chips sit in: a sidebar's foot, or the bar under the
    /// cards. The hover card stays within its sides.
    pub(super) strip_bounds: Rc<Cell<Bounds<Pixels>>>,
    _poll_task: Option<Task<()>>,
    _refresh_task: Option<Task<()>>,
}

impl UsageState {
    fn apply(&mut self, result: anyhow::Result<UsageSnapshot>) {
        self.loading = false;
        self.checking = false;
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

    /// Closes the hover card and forgets any opening still waiting. Returns
    /// whether anything changed.
    fn cancel_hover(&mut self, now: Instant) -> bool {
        self.hover_intent = self.hover_intent.wrapping_add(1);
        self._hover_task = None;
        self.hover(None, now)
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
        // The windows that block work first, shortest first, then the model
        // and spend quotas; balances are context, not a limit.
        let mut resources = provider.resources.iter().collect::<Vec<_>>();
        resources.sort_by_key(|(key, resource)| quota_rank(key, &resource.kind));
        let rows = resources
            .into_iter()
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
        // The refresh icon says a reading is in flight; this line keeps saying
        // how old the values on screen are.
        let status = provider
            .fetched_at
            .as_deref()
            .and_then(timestamp)
            .map(|fetched| match now.saturating_sub(fetched).max(0) / 60 {
                0 => "Updated just now".to_owned(),
                minutes => format!("Updated {minutes} min ago"),
            });
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

/// A provider's quota windows at a glance, for Activity.
pub(super) struct QuotaGlance {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) session: Option<f64>,
    pub(super) weekly: Option<f64>,
    pub(super) reset: Option<String>,
    pub(super) stale: bool,
}

impl UsageState {
    /// Each provider with a session or weekly window, in the status bar's order.
    pub(super) fn glances(&self) -> Vec<QuotaGlance> {
        let now = now_timestamp();
        let offset = local_utc_offset(now);
        self.snapshot
            .providers
            .iter()
            .filter_map(|(id, provider)| {
                let percent = |key: &str| {
                    provider
                        .resources
                        .get(key)
                        .and_then(|resource| resource.percent_used())
                };
                let (session, weekly) = (percent("session"), percent("weekly"));
                if session.is_none() && weekly.is_none() {
                    return None;
                }
                let window = if session.is_some() {
                    "session"
                } else {
                    "weekly"
                };
                Some(QuotaGlance {
                    id: id.clone(),
                    name: provider.display_name.clone(),
                    session,
                    weekly,
                    reset: provider
                        .resources
                        .get(window)
                        .and_then(|resource| resource.resets_at.as_deref())
                        .map(|reset| reset_label(reset, now, offset)),
                    stale: self.provider_stale(id, provider, now),
                })
            })
            .collect()
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
        // Only a reading someone waits for shows as one. A click during the
        // poll's reading adopts it rather than starting a second.
        if (manual || !self.usage.loaded) && !self.usage.checking {
            self.usage.checking = true;
            cx.notify();
        }
        if self.usage.loading {
            return;
        }
        self.usage.loading = true;
        let monitor = self.usage.monitor.clone();
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

    /// Where the hover card hangs, and how wide: just above the chips and
    /// centered over them like the group in its strip, but within the strip's
    /// sides, narrower than `width` when the strip is. In a sidebar that
    /// keeps it over the sidebar.
    fn usage_popover_frame(&self, width: f32) -> (Point<Pixels>, f32) {
        let chips = self.usage.tracker_bounds.get();
        let strip = self.usage.strip_bounds.get();
        let top = chips.top() - px(POPOVER_GAP);
        let center: f32 = chips.center().x.into();
        // The strip has not been laid out yet.
        if strip.size.width <= px(0.0) {
            return (point(px(center - width / 2.0), top), width);
        }
        let left = f32::from(strip.left()) + POPOVER_INSET;
        let right = f32::from(strip.right()) - POPOVER_INSET;
        let width = width.min(right - left).max(POPOVER_MIN_WIDTH);
        let x = (center - width / 2.0).clamp(left, (right - width).max(left));
        (point(px(x), top), width)
    }

    /// Opens Activity, which lists every subscription in full.
    fn show_usage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.usage.cancel_hover(Instant::now());
        self.select_section(WorkspaceSection::Activity, window, cx);
        self.refresh_usage(false, cx);
        cx.notify();
    }

    /// The pointer entered a provider's chip. The card opens after a short
    /// rest; once it is open it follows the pointer from chip to chip.
    fn hover_usage_chip(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.usage.hovered.is_some() {
            if self.usage.hover(Some(id), Instant::now()) {
                cx.notify();
            }
            return;
        }
        self.usage.hover_intent = self.usage.hover_intent.wrapping_add(1);
        let intent = self.usage.hover_intent;
        let id = id.to_owned();
        let delay = cx.background_executor().timer(HOVER_CARD_DELAY);
        self.usage._hover_task = Some(cx.spawn(async move |this, cx| {
            delay.await;
            let _ = this.update(cx, |this, cx| {
                if this.usage.hover_intent == intent && this.usage.hover(Some(&id), Instant::now())
                {
                    cx.notify();
                }
            });
        }));
    }

    /// The status bar's usage group. Each provider shows its mark and the share
    /// used of its tightest window; hovering one lists every quota and clicking
    /// any of them opens Activity, with every subscription.
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
                if !*hovered && this.usage.cancel_hover(Instant::now()) {
                    cx.notify();
                }
            }))
            .child(super::measure_bounds(self.usage.tracker_bounds.clone()))
            .when(
                snapshot.providers.is_empty() && self.usage.loaded,
                |group| {
                    group.child(
                        usage_chip("status-usage-empty".into())
                            .hover(|chip| chip.text_color(colors().foreground))
                            .child(
                                svg()
                                    .path("chrome-icons/sparkles.svg")
                                    .size(px(CHIP_MARK_SIZE))
                                    .flex_none()
                                    .text_color(colors().subtle)
                                    .group_hover(USAGE_CHIP_GROUP, |mark| {
                                        mark.text_color(colors().foreground)
                                    }),
                            )
                            .child(div().min_w(px(0.0)).truncate().child("No usage data"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.show_usage(window, cx)),
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
                    // A quota in alert is already lit; the rest light up on hover.
                    .when(alert.is_none(), |chip| {
                        chip.hover(|chip| chip.text_color(colors().foreground))
                    })
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered {
                            this.hover_usage_chip(&hover_id, cx);
                        }
                    }))
                    .child(
                        svg()
                            .path(provider_mark(id))
                            .size(px(CHIP_MARK_SIZE))
                            .flex_none()
                            .text_color(color)
                            .when(alert.is_none(), |mark| {
                                mark.group_hover(USAGE_CHIP_GROUP, |mark| {
                                    mark.text_color(colors().foreground)
                                })
                            }),
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
                    .on_click(cx.listener(|this, _, window, cx| this.show_usage(window, cx)))
            }))
            .into_any_element()
    }

    /// Quotas of the provider under the pointer, in a card hanging above the
    /// status bar, level with the usage group.
    pub(super) fn usage_hover_card(&self) -> Option<AnyElement> {
        if self.context_menu.is_some()
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
        let (position, width) = self.usage_popover_frame(HOVER_CARD_WIDTH);
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

    /// Every subscription in full, for Activity: a card each with its plan,
    /// when it was read, its quotas with their resets, and its balances, under
    /// a heading that carries the refresh control.
    pub(super) fn usage_section(&self, cx: &mut Context<Self>) -> Div {
        let now = now_timestamp();
        let utc_offset = local_utc_offset(now);
        let snapshot = &self.usage.snapshot;
        let checking = self.usage.checking;
        let refresh_tooltip = if checking {
            "Checking quotas…"
        } else {
            "Refresh usage · Reads every 5 min · May request Keychain access"
        };
        let empty = snapshot.providers.is_empty() && self.usage.error.is_none();
        let heading = super::activity::section_heading("Subscriptions", snapshot.providers.len())
            .child(div().flex_1())
            .child(
                div()
                    .id("usage-refresh")
                    .group("usage-refresh")
                    .size(px(20.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.0))
                    .when(!checking, |button| {
                        button
                            .cursor_pointer()
                            .hover(|button| button.bg(foreground(0.06)))
                    })
                    .tooltip(move |_, cx| sidebar_tooltip(refresh_tooltip, cx))
                    .on_click(cx.listener(|this, _, _, cx| this.refresh_usage(true, cx)))
                    .child(refresh_icon(checking)),
            );
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .text_size(px(12.0))
            .text_color(colors().foreground)
            .child(heading)
            .when_some(self.usage.error.clone(), |section, error| {
                section.child(usage_note(None, &error))
            })
            .when(empty, |section| {
                section.child(super::activity::empty_note(if checking {
                    "Checking quotas…"
                } else {
                    "No subscriptions found. Sign in with Claude Code, Codex, or Grok Build, \
                     then refresh."
                }))
            })
            .when(!snapshot.providers.is_empty(), |section| {
                section.child(div().flex().flex_wrap().gap(px(10.0)).children(
                    snapshot.providers.iter().map(|(id, provider)| {
                        provider_section(&self.usage.card(id, provider, now, utc_offset))
                    }),
                ))
            })
            // Failures for a provider that has nothing to list would otherwise vanish.
            .children(
                snapshot
                    .errors
                    .iter()
                    .filter(|error| !snapshot.providers.contains_key(&error.provider_id))
                    .map(|error| {
                        usage_note(
                            Some(&error.provider_id),
                            &format!("{}: {}", provider_name(&error.provider_id), error.message),
                        )
                    }),
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
        // Flat like the other status items: hover lights the mark, not a box.
        .group(USAGE_CHIP_GROUP)
        .flex()
        .items_center()
        .gap(px(5.0))
        .cursor_pointer()
        .text_size(px(11.0))
        .text_color(colors().muted)
}

/// Where a quota goes in its provider's list: the session, day, week and month
/// windows, then quotas scoped to a model or feature, then spend against a
/// cap, then balances. Equal ranks keep the order of their keys.
fn quota_rank(key: &str, kind: &str) -> u8 {
    match key {
        _ if kind == "balance" => 6,
        "session" => 0,
        "daily" => 1,
        "weekly" => 2,
        "monthly" => 3,
        "extraUsage" | "onDemand" | "totalUsage" => 5,
        _ => 4,
    }
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
pub(super) fn provider_kind(id: &str) -> Option<&'static str> {
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

/// Everything the hover card and Activity show about one provider.
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
    /// Whether the card shows values that may no longer be current. Without
    /// values there is nothing to call outdated; the note says what failed.
    fn outdated(&self) -> bool {
        self.stale && !self.rows.is_empty()
    }

    /// Whether the quota list has something to say: quotas, or that there are
    /// none when no note already explains why.
    fn lists_quotas(&self) -> bool {
        !self.rows.is_empty() || self.warnings.is_empty()
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
            plan: card.plan.clone(),
            status: card.status.clone(),
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
    plan: Option<String>,
    status: Option<String>,
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

/// A provider's tile beside its name and plan, with when it was read below,
/// in the warning color once the values are outdated. While `progress` runs
/// to 1 it crossfades from `from`.
fn provider_heading(card: &UsageCard, from: Option<&CardFrame>, progress: f32) -> Div {
    let status_color = if card.outdated() {
        colors().warning
    } else {
        colors().subtle
    };
    div()
        .flex()
        .items_center()
        .gap(px(9.0))
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
                        .flex()
                        .items_baseline()
                        .gap(px(6.0))
                        .child(
                            div()
                                .min_w(px(0.0))
                                .text_size(px(12.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(crossfade(
                                    card.name.clone(),
                                    from.map(|from| from.name.clone()),
                                    progress,
                                )),
                        )
                        .when_some(card.plan.clone(), |line, plan| {
                            line.child(
                                div()
                                    .min_w(px(0.0))
                                    .flex_1()
                                    .text_size(px(11.0))
                                    .text_color(colors().subtle)
                                    .child(crossfade(
                                        plan,
                                        from.and_then(|from| from.plan.clone()),
                                        progress,
                                    )),
                            )
                        }),
                )
                .when_some(card.status.clone(), |text, status| {
                    text.child(
                        div()
                            .text_size(px(11.0))
                            .text_color(status_color)
                            .child(crossfade(
                                status,
                                from.and_then(|from| from.status.clone()),
                                progress,
                            )),
                    )
                }),
        )
}

/// Whether a quota `width` wide puts its reset under its bar rather than
/// beside its value.
fn stacks_resets(width: f32) -> bool {
    width - 2.0 * (CARD_PADDING + 1.0) < INLINE_RESET_WIDTH
}

/// A provider's quotas: the ones with a limit over their bars, then balances
/// and other plain values in a tighter group below a hairline. Resets
/// crossfade from `from` while `progress` runs to 1.
fn quota_list(
    rows: Vec<QuotaFrame>,
    from: Option<&CardFrame>,
    progress: f32,
    stacked: bool,
) -> Div {
    let empty = rows.is_empty();
    let (bounded, plain): (Vec<_>, Vec<_>) =
        rows.into_iter().partition(|row| row.percent.is_some());
    let divided = !bounded.is_empty();
    let row = |row: QuotaFrame| {
        let previous_reset = from
            .and_then(|from| from.row(&row.label))
            .and_then(|previous| previous.reset.clone());
        quota_row(row, previous_reset, progress, stacked)
    };
    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .when(empty, |list| {
            list.child(
                div()
                    .text_color(colors().muted)
                    .child("No quotas available"),
            )
        })
        .children(bounded.into_iter().map(row))
        .when(!plain.is_empty(), |list| {
            list.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(5.0))
                    .when(divided, |group| {
                        group
                            .pt(px(9.0))
                            .border_t_1()
                            .border_color(colors().border_subtle)
                    })
                    .children(plain.into_iter().map(row)),
            )
        })
}

/// One quota. With a limit it reads as its name, when it resets, and the
/// share used in a column on the right, over its bar; a value close to
/// blocking work takes the bar's color. Without a limit it is a quieter
/// line of name and value. `stacked` moves the reset under the bar, for a
/// card too narrow to hold it beside the value.
fn quota_row(row: QuotaFrame, previous_reset: Option<String>, progress: f32, stacked: bool) -> Div {
    let bounded = row.percent.is_some();
    let reset = row.reset.map(|reset| {
        div()
            .min_w(px(0.0))
            .text_size(px(11.0))
            .text_color(colors().subtle)
            .child(crossfade(reset, previous_reset, progress))
    });
    // Beside the value, the reset takes the room the name leaves and ends there.
    let (inline_reset, stacked_reset) = if stacked {
        (None, reset)
    } else {
        (reset.map(|reset| reset.flex_1().text_right()), None)
    };
    let fills = inline_reset.is_none();
    div()
        .flex()
        .flex_col()
        .gap(px(5.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .when(!bounded, |line| line.text_size(px(11.5)))
                .child(
                    div()
                        .min_w(px(0.0))
                        .when(fills, |label| label.flex_1())
                        .truncate()
                        .text_color(if bounded {
                            foreground(0.86)
                        } else {
                            colors().muted.into()
                        })
                        .child(row.label),
                )
                .children(inline_reset)
                .child(
                    tabular(div())
                        .flex_none()
                        .when(bounded, |value| {
                            value.min_w(px(QUOTA_VALUE_WIDTH)).text_right()
                        })
                        .font_weight(if row.emphasized {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::MEDIUM
                        })
                        .text_color(if row.emphasized {
                            row.color.into()
                        } else if bounded {
                            Hsla::from(colors().foreground)
                        } else {
                            foreground(0.72)
                        })
                        .child(row.value),
                ),
        )
        .when_some(row.percent, |quota, percent| {
            quota.child(quota_bar(percent, row.color, QUOTA_BAR_HEIGHT))
        })
        .children(stacked_reset)
}

/// Why a reading is missing or old, and what to do about it: a quiet tinted
/// note with a warning dot, rather than a line of colored text. It closes a
/// provider's card, or stands alone with the provider's tile when the
/// provider has no card.
fn usage_note(provider: Option<&str>, message: &str) -> Div {
    let warning = colors().warning;
    div()
        .flex_none()
        .flex()
        .items_start()
        .gap(px(8.0))
        .px(px(9.0))
        .py(px(7.0))
        .rounded(px(7.0))
        .bg(Rgba { a: 0.09, ..warning })
        .text_size(px(11.5))
        .text_color(foreground(0.82))
        .child(match provider {
            Some(id) => provider_tile(id, 18.0),
            // Level with the first line of the message.
            None => div()
                .flex_none()
                .mt(px(6.0))
                .size(px(6.0))
                .rounded_full()
                .bg(warning),
        })
        .child(div().min_w(px(0.0)).flex_1().child(message.to_owned()))
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
        .p(px(CARD_PADDING))
        .gap(px(12.0))
        .text_size(px(12.0))
        .text_color(colors().foreground)
        .child(provider_heading(card, from, progress))
        .when(card.lists_quotas(), |content| {
            content.child(quota_list(frame.rows, from, progress, stacks_resets(width)))
        })
        .children(
            card.warnings
                .iter()
                .map(|warning| usage_note(None, warning)),
        )
}

/// One subscription in Activity: a card with its heading, its quotas with
/// the reset beside each value, its balances, and why a reading failed.
fn provider_section(card: &UsageCard) -> impl IntoElement + use<> {
    let rows = card
        .rows
        .iter()
        .map(|row| QuotaFrame::of(card, row))
        .collect();
    div()
        .flex_1()
        .min_w(px(SECTION_CARD_MIN_WIDTH))
        .max_w(px(SECTION_CARD_MAX_WIDTH))
        .flex()
        .flex_col()
        .gap(px(12.0))
        .p(px(14.0))
        .rounded(px(SECTION_CARD_RADIUS))
        .border_1()
        .border_color(colors().border_subtle)
        .bg(foreground(0.03))
        .child(provider_heading(card, None, 1.0))
        .when(card.lists_quotas(), |section| {
            section.child(quota_list(rows, None, 1.0, false))
        })
        .children(
            card.warnings
                .iter()
                .map(|warning| usage_note(None, warning)),
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
        // With no values, the note stands in for the badge and the empty list.
        assert!(!card.outdated());
        assert!(!card.lists_quotas());
        let kept = UsageCard {
            rows: vec![UsageCardRow {
                label: "Week".into(),
                value: "27%".into(),
                percent: Some(27.0),
                reset: None,
            }],
            ..card.clone()
        };
        assert!(kept.outdated() && kept.lists_quotas());
        let unread = UsageCard {
            warnings: Vec::new(),
            ..card
        };
        assert!(unread.lists_quotas());
        // A reading in flight never replaces how old the values are.
        state.loading = true;
        state.checking = true;
        assert_eq!(
            state.card("claude", provider, now_timestamp(), 0).status,
            None
        );
    }

    #[test]
    fn quotas_list_blocking_windows_first_then_scoped_ones_spend_and_balances() {
        let mut state = UsageState::default();
        let quota = r#"{"kind":"consumption","unit":"percent","used":10,"limit":100}"#;
        state.apply(Ok(snapshot(&format!(
            r#"{{"providers":{{"claude":{{"displayName":"Claude","resources":{{
                "Spark · 5 hours":{quota},"credits":{{"kind":"balance","unit":"credits","available":4}},
                "extraUsage":{{"kind":"consumption","unit":"usd","used":5,"limit":50}},
                "fable":{quota},"monthly":{quota},"session":{quota},"sonnet":{quota},
                "weekly":{quota},"daily":{quota}}}}}}}}}"#
        ))));
        let provider = &state.snapshot.providers["claude"];
        let labels = state
            .card("claude", provider, now_timestamp(), 0)
            .rows
            .into_iter()
            .map(|row| row.label)
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            [
                "5 hours",
                "Day",
                "Week",
                "Month",
                "Spark · 5 hours",
                "Fable",
                "Sonnet",
                "Extra usage",
                "Credits"
            ]
        );
    }

    #[gpui::test]
    fn only_readings_someone_waits_for_show_as_checking(cx: &mut gpui::TestAppContext) {
        let (root, _, _, window) =
            super::super::tests::open_recording_workspace(cx, "usage-checking");
        window
            .update(cx, |view, window, cx| {
                // A reading in flight keeps the test away from the user's real logins.
                view.usage.loading = true;
                view.usage.loaded = true;
                view.refresh_usage(false, cx);
                assert!(!view.usage.checking);
                // A click adopts the reading in flight, and its end clears the mark.
                view.refresh_usage(true, cx);
                assert!(view.usage.checking);
                view.usage.apply(Ok(UsageSnapshot::default()));
                assert!(!view.usage.checking && !view.usage.loading);
                // Before the first reading, there is nothing else to show.
                view.usage.loading = true;
                view.usage.loaded = false;
                view.refresh_usage(false, cx);
                assert!(view.usage.checking);
                window.remove_window();
            })
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
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
                // A click leaves for Activity, which clears the hover.
                view.usage.loading = true;
                view.show_usage(window, cx);
                assert_eq!(view.workspace_section, WorkspaceSection::Activity);
                assert_eq!(view.usage.hovered, None);
                assert!(view.usage.hover_motion.is_none());
                assert!(view.usage_hover_card().is_none());
                window.remove_window();
            })
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[gpui::test]
    fn usage_hover_card_waits_for_the_pointer_to_rest(cx: &mut gpui::TestAppContext) {
        let (root, _, _, window) =
            super::super::tests::open_recording_workspace(cx, "usage-hover-delay");
        window
            .update(cx, |view, _, cx| {
                view.usage.snapshot = snapshot(
                    r#"{"providers":{"claude":{"displayName":"Claude","resources":{}},
                                     "codex":{"displayName":"Codex","resources":{}}}}"#,
                );
                // Crossing a chip opens nothing yet.
                view.hover_usage_chip("codex", cx);
                assert_eq!(view.usage.hovered, None);
            })
            .unwrap();
        cx.executor().advance_clock(HOVER_CARD_DELAY);
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| {
                assert_eq!(view.usage.hovered.as_deref(), Some("codex"));
                // Once open, the card follows the pointer at once.
                view.hover_usage_chip("claude", cx);
                assert_eq!(view.usage.hovered.as_deref(), Some("claude"));
                // Leaving the group also drops an opening still waiting.
                view.usage.cancel_hover(Instant::now());
                view.hover_usage_chip("codex", cx);
                view.usage.cancel_hover(Instant::now());
            })
            .unwrap();
        cx.executor().advance_clock(HOVER_CARD_DELAY);
        cx.run_until_parked();
        window
            .update(cx, |view, window, _| {
                assert_eq!(view.usage.hovered, None);
                window.remove_window();
            })
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[gpui::test]
    fn usage_section_builds_in_every_state(cx: &mut gpui::TestAppContext) {
        let (root, _, _, window) =
            super::super::tests::open_recording_workspace(cx, "usage-section");
        window
            .update(cx, |view, window, cx| {
                // Nothing read yet, a reading in flight, and one that found nothing.
                for checking in [false, true, false] {
                    view.usage.checking = checking;
                    view.usage_section(cx);
                }
                view.usage.snapshot = snapshot(
                    r#"{"providers":{"codex":{"displayName":"Codex","resources":{}}},
                        "errors":[{"providerId":"grok","message":"Signed out"}]}"#,
                );
                view.usage_section(cx);
                // Quotas with bars and resets, a balance below them, and a monitor failure.
                view.usage.snapshot = snapshot(
                    r#"{"providers":{"claude":{"displayName":"Claude","plan":"max",
                        "resources":{
                            "session":{"kind":"consumption","unit":"percent","used":97,
                                       "limit":100,"resetsAt":"2026-09-26T18:12:00Z"},
                            "credits":{"kind":"balance","unit":"credits","available":40}}}}}"#,
                );
                view.usage.error = Some("Could not refresh the quota monitor.".into());
                view.usage_section(cx);
                // An outdated reading closes with its note; one never read is only the note.
                view.usage.snapshot.errors = snapshot(
                    r#"{"providers":{},"errors":[
                        {"providerId":"claude","message":"Rate limited"}]}"#,
                )
                .errors;
                view.usage_section(cx);
                view.usage
                    .snapshot
                    .providers
                    .get_mut("claude")
                    .unwrap()
                    .resources
                    .clear();
                view.usage_section(cx);
                // Activity carries the section.
                view.usage.loading = true;
                view.select_section(WorkspaceSection::Activity, window, cx);
                view.activity_content(cx);
                window.remove_window();
            })
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[gpui::test]
    fn usage_hover_card_stays_within_the_strip_that_holds_the_chips(cx: &mut gpui::TestAppContext) {
        let (root, _, _, window) =
            super::super::tests::open_recording_workspace(cx, "usage-contained");
        window
            .update(cx, |view, window, _| {
                let chips =
                    Bounds::new(point(px(40.0), px(900.0)), gpui::size(px(150.0), px(22.0)));
                view.usage.tracker_bounds.set(chips);
                // A sidebar 210 wide: the card takes its width, less the inset each side.
                view.usage.strip_bounds.set(Bounds::new(
                    point(px(0.0), px(896.0)),
                    gpui::size(px(210.0), px(30.0)),
                ));
                let (anchor, width) = view.usage_popover_frame(HOVER_CARD_WIDTH);
                assert_eq!(anchor, point(px(POPOVER_INSET), px(900.0 - POPOVER_GAP)));
                assert_eq!(width, 210.0 - 2.0 * POPOVER_INSET);
                assert!(stacks_resets(width));
                // A wide sidebar: full width, centered over the chips, held at the sides.
                view.usage.strip_bounds.set(Bounds::new(
                    point(px(0.0), px(896.0)),
                    gpui::size(px(300.0), px(30.0)),
                ));
                assert_eq!(
                    view.usage_popover_frame(HOVER_CARD_WIDTH),
                    (point(px(POPOVER_INSET), px(892.0)), HOVER_CARD_WIDTH)
                );
                assert!(!stacks_resets(HOVER_CARD_WIDTH));
                view.usage.tracker_bounds.set(Bounds::new(
                    point(px(250.0), px(900.0)),
                    gpui::size(px(40.0), px(22.0)),
                ));
                let (anchor, width) = view.usage_popover_frame(HOVER_CARD_WIDTH);
                assert_eq!(anchor.x + px(width), px(300.0 - POPOVER_INSET));
                // Under the cards there is room to center it.
                view.usage.strip_bounds.set(Bounds::new(
                    point(px(0.0), px(896.0)),
                    gpui::size(px(1200.0), px(30.0)),
                ));
                view.usage.tracker_bounds.set(Bounds::new(
                    point(px(500.0), px(900.0)),
                    gpui::size(px(200.0), px(22.0)),
                ));
                assert_eq!(
                    view.usage_popover_frame(HOVER_CARD_WIDTH),
                    (
                        point(px(600.0 - HOVER_CARD_WIDTH / 2.0), px(892.0)),
                        HOVER_CARD_WIDTH
                    )
                );
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
