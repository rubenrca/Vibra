//! Subscription usage: a one-line sidebar footer with a pair of concentric
//! progress rings per provider (session outside, week inside). Hovering one
//! lists its quotas in a card above the footer, which eases in and glides
//! between providers; clicking opens a modal listing every subscription.

use std::cell::Cell;
use std::f32::consts::TAU;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt, AnyElement, Bounds, Context, Corner, Div, Hsla, MouseButton,
    PathBuilder, Pixels, Point, Rgba, SharedString, Stateful, Task, Timer, Window, canvas, div,
    point, prelude::*, px, relative, svg,
};

use crate::domain::usage::{
    ProviderUsage, UsageSnapshot, now_timestamp, reset_label, resource_label, timestamp,
};
use crate::infrastructure::clock::local_utc_offset;
use crate::infrastructure::usage::UsageMonitor;
use crate::ui::menu::{menu_hover, menu_panel};
use crate::ui::theme::{colors, mix};

use super::chrome::{SIDEBAR_ROW_INSET, ease_out_cubic};
use super::context_menu::MENU_EDGE_MARGIN;
use super::{WorkspaceView, sidebar_tooltip};

const POLL_INTERVAL: Duration = Duration::from_secs(30);
const RING_SIZE: f32 = 18.0;
const RING_STROKE: f32 = 1.75;
/// Space between concentric rings.
const RING_GAP: f32 = 1.5;
const CHIP_HEIGHT: f32 = 32.0;
const FOOTER_PAD_Y: f32 = 8.0;
/// Space between the hover card and the chips it describes.
const HOVER_CARD_GAP: f32 = 4.0;
const HOVER_CARD_MAX_WIDTH: f32 = 232.0;
/// The hover card fades in while rising this far into place.
const HOVER_CARD_RISE: f32 = 4.0;
const HOVER_CARD_ENTER: Duration = Duration::from_millis(100);
/// Quotas count up from zero when the card opens…
const HOVER_COUNT_UP: Duration = Duration::from_millis(400);
/// …and glide to the next provider's values when the pointer moves on.
const HOVER_SWITCH: Duration = Duration::from_millis(240);

#[derive(Default)]
pub(super) struct UsageState {
    monitor: Arc<Mutex<UsageMonitor>>,
    snapshot: UsageSnapshot,
    error: Option<String>,
    loading: bool,
    /// Whether a first reading has finished; until then there is nothing to say.
    loaded: bool,
    pub open: bool,
    /// Bottom-left corner of the modal, level with the footer chip that opened it.
    anchor: Point<Pixels>,
    /// Provider whose chip the pointer last entered, until it leaves the footer.
    hovered: Option<String>,
    /// How the hover card got to what it shows, while it shows anything.
    hover_motion: Option<HoverMotion>,
    /// Gives each hover motion an animation of its own.
    hover_motions: u64,
    /// Where the footer sits in the window, to hang the hover card above it.
    footer_bounds: Rc<Cell<Bounds<Pixels>>>,
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
                    0 => "Updated less than 1 min ago".to_owned(),
                    minutes => format!("Updated {minutes} min ago"),
                })
        };
        UsageCard {
            id: id.to_owned(),
            name: provider.display_name.clone(),
            plan: provider.plan.clone(),
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

    /// Opens the modal beside the sidebar, rising from `bottom`, the bottom
    /// edge of the chip that was clicked.
    fn open_usage(&mut self, bottom: f32, window: &mut Window, cx: &mut Context<Self>) {
        self.usage.anchor = point(px(self.sidebar_popover_left()), px(bottom));
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

    /// Sidebar footer on a single line. Each provider shows its quota windows as
    /// concentric rings; hovering one lists them and clicking any of them opens
    /// every subscription.
    pub(super) fn usage_tracker(&self, cx: &mut Context<Self>) -> AnyElement {
        let now = now_timestamp();
        let snapshot = &self.usage.snapshot;
        div()
            .id("sidebar-usage")
            .relative()
            .flex_none()
            // Keep the footer's height while the first reading is in flight so
            // the providers appear without moving the project list.
            .min_h(px(CHIP_HEIGHT + 2.0 * FOOTER_PAD_Y))
            .px(px(SIDEBAR_ROW_INSET))
            .py(px(FOOTER_PAD_Y))
            .flex()
            .items_center()
            .overflow_hidden()
            // The card stays up between chips and goes once the pointer leaves the footer.
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered && this.usage.hover(None, Instant::now()) {
                    cx.notify();
                }
            }))
            .child(super::measure_bounds(self.usage.footer_bounds.clone()))
            .when(
                snapshot.providers.is_empty() && self.usage.loaded,
                |footer| {
                    let bounds = Rc::new(Cell::new(Bounds::<Pixels>::default()));
                    footer.child(
                        usage_chip("sidebar-usage-empty".into())
                            .child(usage_ring(vec![(None, colors().subtle)]))
                            .child(div().min_w(px(0.0)).truncate().child("No usage data"))
                            .child(super::measure_bounds(bounds.clone()))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_usage(bounds.get().bottom().into(), window, cx)
                            })),
                    )
                },
            )
            .children(snapshot.providers.iter().map(|(id, provider)| {
                let stale = self.usage.provider_stale(id, provider, now);
                let bounds = Rc::new(Cell::new(Bounds::<Pixels>::default()));
                let percents = ring_percents(provider);
                // A quota close to blocking work also sets its provider's name apart.
                let alerting = !stale
                    && percents
                        .iter()
                        .any(|&percent| quota_alert(percent).is_some());
                let rings = percents
                    .into_iter()
                    .map(|percent| {
                        let color = if stale {
                            colors().subtle
                        } else {
                            quota_alert(percent).unwrap_or(colors().muted)
                        };
                        (percent, color)
                    })
                    .collect();
                let hover_id = id.clone();
                usage_chip(SharedString::from(format!("sidebar-usage-{id}")))
                    .when(alerting, |chip| {
                        chip.font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(colors().foreground)
                    })
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered && this.usage.hover(Some(&hover_id), Instant::now()) {
                            cx.notify();
                        }
                    }))
                    .child(usage_ring(rings))
                    .child(
                        div()
                            .min_w(px(0.0))
                            .truncate()
                            .child(provider.display_name.clone()),
                    )
                    .child(super::measure_bounds(bounds.clone()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_usage(bounds.get().bottom().into(), window, cx)
                    }))
            }))
            .into_any_element()
    }

    /// Quotas of the provider under the pointer, in a card hanging above the
    /// footer, level with the sidebar rows.
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
        let footer = self.usage.footer_bounds.get();
        let row_width = f32::from(footer.size.width) - 2.0 * SIDEBAR_ROW_INSET;
        // Not measured yet, or a sidebar too narrow for any card.
        if row_width <= 0.0 {
            return None;
        }
        let now = now_timestamp();
        let card = self.usage.card(id, provider, now, local_utc_offset(now));
        let width = row_width.min(HOVER_CARD_MAX_WIDTH);
        let position = point(
            footer.left() + px(SIDEBAR_ROW_INSET),
            footer.top() + px(FOOTER_PAD_Y - HOVER_CARD_GAP),
        );
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
        let refresh_tooltip = if self.usage.loading {
            "Checking quotas…"
        } else {
            "Refresh usage · May request Keychain access"
        };
        let content = div()
            .id("usage-provider-list")
            .min_h(px(0.0))
            .overflow_y_scroll()
            .px(px(14.0))
            .pb(px(14.0))
            .flex()
            .flex_col()
            .gap(px(14.0))
            .when_some(self.usage.error.clone(), |content, error| {
                content.child(div().text_color(colors().warning).child(error))
            })
            .when(
                snapshot.providers.is_empty() && self.usage.error.is_none(),
                |content| {
                    content.child(
                        div()
                            .text_color(colors().muted)
                            .child(if self.usage.loading {
                                "Checking quotas…"
                            } else {
                                "Sign in with Claude Code, Codex, or Grok Build and click Refresh."
                            }),
                    )
                },
            )
            .children(
                snapshot
                    .providers
                    .iter()
                    .enumerate()
                    .map(|(index, (id, provider))| {
                        usage_section(&self.usage.card(id, provider, now, utc_offset), index)
                    }),
            )
            // Failures for a provider that has nothing to list would otherwise vanish.
            .children(
                snapshot
                    .errors
                    .iter()
                    .filter(|error| !snapshot.providers.contains_key(&error.provider_id))
                    .map(|error| {
                        div()
                            .text_color(colors().warning)
                            .child(format!("{}: {}", error.provider_id, error.message))
                    }),
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
                        .child(
                    menu_panel()
                        .id("usage-modal")
                        .p(px(0.0))
                        .w(px(380.0))
                        .max_h(px((available - MENU_EDGE_MARGIN).max(100.0)))
                        .text_size(px(11.0))
                        .text_color(colors().foreground)
                        .overflow_hidden()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap(px(10.0))
                                .px(px(14.0))
                                .py(px(10.0))
                                .child(
                                    div()
                                        .id("usage-title")
                                        .flex_1()
                                        .text_size(px(12.0))
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .text_color(colors().muted)
                                        .tooltip(|_, cx| {
                                            sidebar_tooltip(
                                                "Subscription usage · Refreshes every 5 min",
                                                cx,
                                            )
                                        })
                                        .child("Usage"),
                                )
                                .child(
                                    div()
                                        .id("usage-refresh")
                                        .group("usage-refresh")
                                        .size(px(24.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded(px(5.0))
                                        .cursor_pointer()
                                        .text_color(colors().muted)
                                        .hover(|button| button.bg(menu_hover()))
                                        .tooltip(move |_, cx| sidebar_tooltip(refresh_tooltip, cx))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.refresh_usage(true, cx)
                                        }))
                                        .when(self.usage.loading, |button| button.child("…"))
                                        .when(!self.usage.loading, |button| {
                                            button.child(
                                                svg()
                                                    .path("chrome-icons/refresh.svg")
                                                    .size(px(13.0))
                                                    .text_color(colors().muted)
                                                    .group_hover("usage-refresh", |icon| {
                                                        icon.text_color(colors().foreground)
                                                    }),
                                            )
                                        }),
                                ),
                        )
                        .child(content),
                        ),
                )
                .into_any_element(),
        )
    }
}

/// One provider in the footer, shaped like the navigation rows above it.
/// Chips share the line and give up width by truncating their name, so a
/// narrow sidebar never cuts a ring. Hovering one leaves it as it is: the
/// card above is the response.
fn usage_chip(id: SharedString) -> Stateful<Div> {
    div()
        .id(id)
        .relative()
        .h(px(CHIP_HEIGHT))
        .min_w(px(0.0))
        .px(px(4.0))
        .flex()
        .items_center()
        .gap(px(5.0))
        .cursor_pointer()
        .text_size(px(12.0))
        .text_color(colors().muted)
}

/// A provider's session window, then its weekly one, outermost ring first.
/// Providers that report neither get their tightest quota, or an empty ring
/// without data.
fn ring_percents(provider: &ProviderUsage) -> Vec<Option<f64>> {
    let windows: Vec<_> = ["session", "weekly"]
        .into_iter()
        .filter_map(|key| provider.resources.get(key)?.percent_used())
        .map(Some)
        .collect();
    if windows.is_empty() {
        vec![
            provider
                .tightest_quota()
                .and_then(|(_, resource)| resource.percent_used()),
        ]
    } else {
        windows
    }
}

/// Concentric quota rings, the first one outermost. Each is a faint track with
/// the consumed share drawn clockwise from the top; without a percentage only
/// the track shows.
fn usage_ring(rings: Vec<(Option<f64>, Rgba)>) -> impl IntoElement {
    div().size(px(RING_SIZE)).flex_none().relative().child(
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let track = Hsla::from(colors().foreground).opacity(0.18);
                let outer = (RING_SIZE - RING_STROKE) / 2.0;
                for (index, (percent, color)) in rings.into_iter().enumerate() {
                    let radius = outer - index as f32 * (RING_STROKE + RING_GAP);
                    let fraction =
                        percent.map_or(0.0, |percent| (percent / 100.0).clamp(0.0, 1.0) as f32);
                    paint_ring_arc(window, bounds.center(), radius, 1.0, track);
                    paint_ring_arc(window, bounds.center(), radius, fraction, color.into());
                }
            },
        )
        .absolute()
        .inset_0(),
    )
}

/// Strokes `fraction` of a circle clockwise from 12 o'clock.
fn paint_ring_arc(
    window: &mut Window,
    center: Point<Pixels>,
    radius: f32,
    fraction: f32,
    color: Hsla,
) {
    if fraction <= 0.0 {
        return;
    }
    let top = point(center.x, center.y - px(radius));
    let radii = point(px(radius), px(radius));
    let mut path = PathBuilder::stroke(px(RING_STROKE));
    path.move_to(top);
    if fraction >= 1.0 {
        // An arc cannot start and end on the same point, so close the circle in halves.
        path.arc_to(
            radii,
            px(0.0),
            false,
            true,
            point(center.x, center.y + px(radius)),
        );
        path.arc_to(radii, px(0.0), false, true, top);
    } else {
        let angle = fraction * TAU;
        let end = point(
            center.x + px(radius * angle.sin()),
            center.y - px(radius * angle.cos()),
        );
        path.arc_to(radii, px(0.0), fraction > 0.5, true, end);
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
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

fn provider_mark(id: &str) -> &'static str {
    if id.starts_with("claude") {
        "agent-marks/claude.svg"
    } else if id.starts_with("codex") {
        "agent-marks/codex.svg"
    } else if id.starts_with("grok") {
        "agent-marks/grok.svg"
    } else {
        "chrome-icons/sparkles.svg"
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
            name: card.name.clone(),
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
    name: String,
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

/// The hover card at `now`: provider name, then each quota as label and value
/// over its bar and reset time. It fades and rises in as it opens, counts its
/// quotas up, and glides between providers.
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
        .text_size(px(13.0))
        .text_color(colors().foreground)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .min_w(px(0.0))
                        .flex_1()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(crossfade(
                            card.name.clone(),
                            from.map(|from| from.name.clone()),
                            progress,
                        )),
                )
                .when(card.stale, |title| {
                    title.child(
                        div()
                            .flex_none()
                            .text_size(px(11.0))
                            .text_color(colors().warning)
                            .child("Outdated"),
                    )
                }),
        )
        .when(card.rows.is_empty(), |panel| {
            panel.child(
                div()
                    .text_color(colors().muted)
                    .child("No quotas available"),
            )
        })
        .children(frame.rows.into_iter().map(|row| {
            let previous_reset = from
                .and_then(|from| from.row(&row.label))
                .and_then(|previous| previous.reset.clone());
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
                                .flex_1()
                                .truncate()
                                .text_color(colors().muted)
                                .child(row.label),
                        )
                        .child(
                            div()
                                .flex_none()
                                .when(row.emphasized, |value| {
                                    value.font_weight(gpui::FontWeight::SEMIBOLD)
                                })
                                .child(row.value),
                        ),
                )
                .when_some(row.percent, |quota, percent| {
                    quota.child(quota_bar(percent, row.color, 4.0))
                })
                .when_some(row.reset, |quota, reset| {
                    quota.child(
                        div()
                            .text_size(px(12.0))
                            .text_color(colors().subtle)
                            .child(crossfade(reset, previous_reset, progress)),
                    )
                })
        }))
        .children(card.warnings.iter().map(|warning| {
            div()
                .text_size(px(12.0))
                .text_color(colors().warning)
                .child(warning.clone())
        }))
}

/// One provider's block in the modal: header, then each quota with its bar.
fn usage_section(card: &UsageCard, index: usize) -> impl IntoElement + use<> {
    let header_tooltip = [card.plan.clone(), card.status.clone()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
    div()
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .when(index > 0, |section| {
            section
                .pt(px(14.0))
                .border_t_1()
                .border_color(colors().border_subtle)
        })
        .child(
            div()
                .id(SharedString::from(format!("usage-provider-{}", card.id)))
                .flex()
                .items_center()
                .gap(px(7.0))
                .text_size(px(12.0))
                .when(!header_tooltip.is_empty(), |header| {
                    header.tooltip(move |_, cx| sidebar_tooltip(header_tooltip.clone(), cx))
                })
                .child(
                    svg()
                        .path(provider_mark(&card.id))
                        .size(px(13.0))
                        .flex_none()
                        .text_color(colors().foreground),
                )
                .child(
                    div()
                        .min_w(px(0.0))
                        .flex_1()
                        .truncate()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(card.name.clone()),
                )
                .when_some(card.plan.clone(), |header, plan| {
                    header.child(
                        div()
                            .max_w(px(100.0))
                            .truncate()
                            .text_size(px(10.0))
                            .text_color(colors().subtle)
                            .child(plan),
                    )
                })
                .when(card.stale, |header| {
                    header.child(
                        div()
                            .text_size(px(10.0))
                            .text_color(colors().warning)
                            .child("Outdated"),
                    )
                }),
        )
        .when(card.rows.is_empty(), |section| {
            section.child(
                div()
                    .text_color(colors().muted)
                    .child("No quotas available"),
            )
        })
        .children(card.rows.iter().map(|row| {
            div()
                .flex()
                .flex_col()
                .gap(px(5.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .min_w(px(0.0))
                                .truncate()
                                .text_color(colors().muted)
                                .child(row.label.clone()),
                        )
                        .child(
                            div()
                                .min_w(px(0.0))
                                .flex_1()
                                .truncate()
                                .text_size(px(10.0))
                                .text_color(colors().subtle)
                                .when_some(row.reset.clone(), |label, reset| label.child(reset)),
                        )
                        .child(div().flex_none().child(row.value.clone())),
                )
                .when_some(row.percent, |row, percent| {
                    row.child(quota_bar(percent, card.bar_color(percent), 3.0))
                })
        }))
        .children(
            card.warnings
                .iter()
                .map(|warning| div().text_color(colors().warning).child(warning.clone())),
        )
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
        assert_eq!(card.plan.as_deref(), Some("plus"));
        assert!(!card.stale);
        assert!(card.warnings.is_empty());
        assert_eq!(card.status.as_deref(), Some("Updated 3 min ago"));
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
    fn rings_follow_session_and_weekly_windows_with_a_single_fallback() {
        let provider = |resources: &str| -> ProviderUsage {
            serde_json::from_str(&format!(r#"{{"displayName":"P","resources":{resources}}}"#))
                .unwrap()
        };
        let quota = |used: u32| {
            format!(r#"{{"kind":"consumption","unit":"percent","used":{used},"limit":100}}"#)
        };
        // Both limits: one ring each, in session-then-week order, ignoring model scopes.
        assert_eq!(
            ring_percents(&provider(&format!(
                r#"{{"opus":{},"weekly":{},"session":{}}}"#,
                quota(90),
                quota(40),
                quota(20)
            ))),
            [Some(20.0), Some(40.0)]
        );
        // A single window gets a single ring.
        assert_eq!(
            ring_percents(&provider(&format!(r#"{{"weekly":{}}}"#, quota(40)))),
            [Some(40.0)]
        );
        // Without either, the tightest quota stands in for them.
        assert_eq!(
            ring_percents(&provider(&format!(r#"{{"monthly":{}}}"#, quota(65)))),
            [Some(65.0)]
        );
        // No data still leaves the provider visible as an empty ring.
        assert_eq!(ring_percents(&provider("{}")), [None]);
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

        // Leaving the footer closes the card at once.
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
    fn usage_hover_card_hangs_above_the_footer_for_the_hovered_provider(
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
                view.usage.footer_bounds.set(Bounds::new(
                    point(px(0.0), px(900.0)),
                    gpui::size(px(250.0), px(48.0)),
                ));
                // Nothing hovered, a provider the snapshot lacks, then the real one.
                assert!(view.usage_hover_card().is_none());
                view.usage.hover(Some("grok"), Instant::now());
                assert!(view.usage_hover_card().is_none());
                view.usage.hover(Some("codex"), Instant::now());
                assert!(view.usage_hover_card().is_some());
                // The footer is not measured until it has been laid out.
                view.usage.footer_bounds.set(Bounds::default());
                assert!(view.usage_hover_card().is_none());
                view.usage.footer_bounds.set(Bounds::new(
                    point(px(0.0), px(900.0)),
                    gpui::size(px(250.0), px(48.0)),
                ));
                // The modal takes over from the card, and opening it clears the hover.
                view.usage.open = true;
                assert!(view.usage_hover_card().is_none());
                view.usage.loading = true;
                view.open_usage(940.0, window, cx);
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
                view.close_usage(window, cx);
                assert!(view.usage_modal(cx).is_none());
                window.remove_window();
            })
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[gpui::test]
    fn usage_modal_rises_from_the_chip_beside_the_sidebar(cx: &mut gpui::TestAppContext) {
        let (root, _, _, window) =
            super::super::tests::open_recording_workspace(cx, "usage-anchor");
        window
            .update(cx, |view, window, cx| {
                // A refresh in flight keeps the test away from the user's real logins.
                view.usage.loading = true;
                view.open_usage(1480.0, window, cx);
                assert!(view.usage.open);
                assert_eq!(
                    view.usage.anchor,
                    point(px(view.sidebar_popover_left()), px(1480.0))
                );
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
