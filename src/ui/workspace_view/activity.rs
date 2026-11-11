//! Activity: what runs for the spaces, read-only, like a small activity
//! monitor. Tiles sum up the servers, the agents, the fullest context, the
//! quotas and today's tokens; below them, each space with its servers,
//! memory and agents, then what runs outside Vibra. Sampling runs only while
//! Activity is in front.

use std::collections::HashMap;
use std::f32::consts::PI;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Context, Div, Font, FontFeatures, FontStyle, FontWeight, PathBuilder, Pixels,
    Point, Rgba, SharedString, Stateful, Task, Timer, Window, canvas, div, point, prelude::*, px,
};
use uuid::Uuid;

use crate::domain::activity::{
    ContextFill, OutsideGroup, ProcessSample, Server, TOKEN_DAYS, TokenHistory, memory_label,
    token_label,
};
use crate::domain::agents::AgentKind;
use crate::domain::usage::now_timestamp;
use crate::infrastructure::activity::{
    AgentSession, ContextReader, PaneProbe, TokenCounter, sample_processes,
};
use crate::infrastructure::automation::{AgentAttention, AgentRuntimeState};
use crate::infrastructure::clock::local_utc_offset;
use crate::ui::agent_marks::{agent_status_color, agent_tile, icon_tile};
use crate::ui::space_marks::{SpaceActivity, ripple_tile};
use crate::ui::theme::{colors, mix};

use super::chrome::{card, format_sidebar_path};
use super::spaces::{agent_state_short_label, elapsed_label};
use super::usage::{QuotaGlance, provider_kind};
use super::{WorkspaceSection, WorkspaceView, sidebar_tooltip};

/// Processes and ports change by the second; this keeps up without load.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(3);
/// Transcripts are read incrementally, but a week of them is not free.
const TOKEN_INTERVAL: Duration = Duration::from_secs(20);
const CONTENT_MAX_WIDTH: f32 = 1080.0;
const TILE_HEIGHT: f32 = 116.0;
const TILE_MIN_WIDTH: f32 = 196.0;
const TILE_RADIUS: f32 = 12.0;
const RING_SIZE: f32 = 58.0;
const RING_STROKE: f32 = 6.0;
const ROW_TILE_SIZE: f32 = 28.0;
const AGENT_TILE_SIZE: f32 = 16.0;
const CONTEXT_BAR_WIDTH: f32 = 96.0;
const MEMORY_COLUMN_WIDTH: f32 = 64.0;

#[derive(Default)]
pub(super) struct ActivityState {
    contexts: Arc<Mutex<ContextReader>>,
    counter: Arc<Mutex<TokenCounter>>,
    sample: Option<ProcessSample>,
    fills: HashMap<Uuid, ContextFill>,
    history: Option<TokenHistory>,
    polling: bool,
    sampling: bool,
    counting: bool,
    counted_at: Option<Instant>,
    _poll_task: Option<Task<()>>,
    _sample_task: Option<Task<()>>,
    _count_task: Option<Task<()>>,
}

/// An agent in a space, as its row lists it.
struct SpaceAgent {
    session_id: Uuid,
    kind: Option<String>,
    title: String,
    state: Option<AgentRuntimeState>,
    attention: Option<AgentAttention>,
    since: Option<Instant>,
    fill: Option<ContextFill>,
}

/// A space's row: what its terminals run and use.
struct SpaceSummary {
    project_id: Uuid,
    name: String,
    activity: SpaceActivity,
    /// Unknown until the first sample.
    memory: Option<u64>,
    servers: Vec<Server>,
    agents: Vec<SpaceAgent>,
}

impl WorkspaceView {
    /// Samples while Activity is in front; leaving it stops the clock.
    pub(super) fn start_activity_poll(&mut self, cx: &mut Context<Self>) {
        // Workspace tests must not read the machine's processes.
        if cfg!(test) || self.activity.polling {
            return;
        }
        self.activity.polling = true;
        self.refresh_activity(cx);
        self.activity._poll_task = Some(cx.spawn(async move |this, cx| {
            loop {
                Timer::after(SAMPLE_INTERVAL).await;
                let shown = this.update(cx, |this, cx| {
                    if this.workspace_section != WorkspaceSection::Activity {
                        this.activity.polling = false;
                        return false;
                    }
                    this.refresh_activity(cx);
                    true
                });
                if !matches!(shown, Ok(true)) {
                    break;
                }
            }
        }));
    }

    fn refresh_activity(&mut self, cx: &mut Context<Self>) {
        let home = self.home_folder();
        if !self.activity.sampling {
            self.activity.sampling = true;
            let (panes, sessions) = self.activity_probes(cx);
            let contexts = self.activity.contexts.clone();
            let home = home.clone();
            self.activity._sample_task = Some(cx.spawn(async move |this, cx| {
                let (sample, fills) = cx
                    .background_spawn(async move {
                        let sample = sample_processes(&panes, Some(&home));
                        let fills = contexts
                            .lock()
                            .map(|mut reader| reader.read(&sessions, &home))
                            .unwrap_or_default();
                        (sample, fills)
                    })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.activity.sampling = false;
                    this.activity.sample = Some(sample);
                    this.activity.fills = fills;
                    cx.notify();
                });
            }));
        }
        let due = self
            .activity
            .counted_at
            .is_none_or(|counted| counted.elapsed() >= TOKEN_INTERVAL);
        if !self.activity.counting && due {
            self.activity.counting = true;
            self.activity.counted_at = Some(Instant::now());
            let counter = self.activity.counter.clone();
            self.activity._count_task = Some(cx.spawn(async move |this, cx| {
                let history = cx
                    .background_spawn(async move {
                        let now = now_timestamp();
                        let offset = local_utc_offset(now);
                        counter
                            .lock()
                            .ok()
                            .map(|mut counter| counter.refresh(&home, now, offset))
                    })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.activity.counting = false;
                    if history.is_some() {
                        this.activity.history = history;
                    }
                    cx.notify();
                });
            }));
        }
    }

    /// Each pane's shell, and the agent sessions their hooks named.
    fn activity_probes(&self, cx: &Context<Self>) -> (Vec<PaneProbe>, Vec<AgentSession>) {
        let mut panes = Vec::new();
        let mut sessions = Vec::new();
        for session in self.snapshot.terminal_sessions() {
            let shell = self
                .terminals
                .get(&session.id)
                .and_then(|terminal| terminal.read(cx).shell_process_id());
            if let Some(shell_pid) = shell {
                panes.push(PaneProbe {
                    pane_id: session.id,
                    shell_pid,
                });
            }
            let Some(presence) = self.resolved_agent_presence(session.id) else {
                continue;
            };
            let kind = AgentKind::ALL
                .into_iter()
                .find(|kind| kind.display_name().eq_ignore_ascii_case(&presence.kind));
            if let (Some(kind), Some(session_id)) = (kind, presence.session_id) {
                sessions.push(AgentSession {
                    pane_id: session.id,
                    kind,
                    session_id,
                });
            }
        }
        (panes, sessions)
    }

    fn activity_spaces(&self, cx: &Context<Self>) -> Vec<SpaceSummary> {
        let sample = self.activity.sample.as_ref();
        // In the sidebar's order: pinned spaces first.
        self.visible_project_order()
            .into_iter()
            .filter_map(|id| self.snapshot.project(id))
            .map(|project| {
                let mut memory = sample.map(|_| 0);
                let mut servers = Vec::new();
                let mut agents = Vec::new();
                for (index, session) in project.terminal_sessions().enumerate() {
                    if let Some(usage) = sample.and_then(|sample| sample.panes.get(&session.id)) {
                        memory = memory.map(|memory| memory + usage.memory);
                        servers.extend(usage.servers.iter().cloned());
                    }
                    let identity = self.pane_identity(session, index, cx);
                    if identity.agent_kind.is_none() || identity.agent_state.is_none() {
                        continue;
                    }
                    agents.push(SpaceAgent {
                        session_id: session.id,
                        kind: identity.agent_kind,
                        title: identity.title,
                        state: identity.agent_state,
                        attention: identity.agent_attention,
                        since: self.agent_state_since.get(&session.id).copied(),
                        fill: self.activity.fills.get(&session.id).copied(),
                    });
                }
                SpaceSummary {
                    project_id: project.id,
                    name: project.name.clone(),
                    activity: self.space_activity(project.id),
                    memory,
                    servers,
                    agents,
                }
            })
            .collect()
    }

    pub(super) fn activity_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let spaces = self.activity_spaces(cx);
        let outside = self
            .activity
            .sample
            .as_ref()
            .map(|sample| sample.outside.clone())
            .unwrap_or_default();
        let body = div()
            .w_full()
            .max_w(px(CONTENT_MAX_WIDTH))
            .px(px(20.0))
            .pt(px(18.0))
            .pb(px(24.0))
            .flex()
            .flex_col()
            .gap(px(22.0))
            .child(self.activity_tiles(&spaces, &outside))
            .child(self.spaces_section(spaces, cx))
            .when(!outside.is_empty(), |body| {
                body.child(self.outside_section(outside))
            })
            .child(
                div()
                    .text_size(px(11.5))
                    .line_height(px(16.0))
                    .text_color(colors().subtle)
                    .child(
                        "Read-only. A space's memory is what its terminals run. Tokens are \
                         input and output, without cache reads.",
                    ),
            );
        card(colors().background)
            .id("activity")
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("activity-scroll")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .flex()
                    .justify_center()
                    .child(body),
            )
            .into_any_element()
    }

    /// Servers, agents, the fullest context, each quota, and today's tokens.
    fn activity_tiles(&self, spaces: &[SpaceSummary], outside: &[OutsideGroup]) -> Div {
        let sampled = self.activity.sample.is_some();
        let servers = spaces
            .iter()
            .map(|space| space.servers.len())
            .sum::<usize>()
            + outside
                .iter()
                .map(|group| group.servers.len())
                .sum::<usize>();
        let memory = spaces.iter().filter_map(|space| space.memory).sum::<u64>()
            + outside.iter().map(|group| group.memory).sum::<u64>();
        let running = tile(
            label_row("Running", None),
            sampled.then(|| {
                div()
                    .text_size(px(11.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors().success)
                    .child("Live")
                    .into_any_element()
            }),
            value_row(
                if sampled {
                    servers.to_string()
                } else {
                    "–".into()
                },
                sampled.then(|| memory_label(memory)),
            ),
            match servers {
                _ if !sampled => "Reading processes…".to_owned(),
                1 => "server listening".to_owned(),
                _ => "servers listening".to_owned(),
            },
        );

        let agents: Vec<&SpaceAgent> = spaces.iter().flat_map(|space| &space.agents).collect();
        let elsewhere: usize = outside.iter().map(|group| group.agents.len()).sum();
        let count = |state| {
            agents
                .iter()
                .filter(|agent| agent.state == Some(state))
                .count()
        };
        let (working, waiting) = (
            count(AgentRuntimeState::Working),
            count(AgentRuntimeState::Waiting),
        );
        let mut parts = Vec::new();
        if working > 0 {
            parts.push(format!("{working} working"));
        }
        if waiting > 0 {
            parts.push(format!(
                "{waiting} need{} you",
                if waiting == 1 { "s" } else { "" }
            ));
        }
        if elsewhere > 0 {
            parts.push(format!("{elsewhere} elsewhere"));
        }
        let agents_caption = match (agents.len() + elsewhere, parts.is_empty()) {
            (0, _) => "No agents running".to_owned(),
            (_, true) => "All idle".to_owned(),
            _ => parts.join(" · "),
        };
        let agents_tile = tile(
            label_row("Agents", None),
            None,
            value_row((agents.len() + elsewhere).to_string(), None),
            agents_caption,
        );

        // The fullest context first; the next one beside it.
        let mut filled: Vec<&SpaceAgent> = agents
            .iter()
            .copied()
            .filter(|agent| agent.fill.is_some())
            .collect();
        filled.sort_by(|left, right| {
            let percent = |agent: &SpaceAgent| agent.fill.map_or(0.0, ContextFill::percent);
            percent(right).total_cmp(&percent(left))
        });
        let context_tile = match filled.first() {
            Some(fullest) => {
                let percent = fullest.fill.map_or(0.0, ContextFill::percent);
                let next = filled
                    .get(1)
                    .and_then(|agent| agent.fill)
                    .map(|fill| format!("{:.0}%", fill.percent()));
                let used = fullest.fill.map(|fill| {
                    format!("{} of {}", token_label(fill.used), token_label(fill.window))
                });
                tile(
                    label_row("Context", fullest.kind.as_deref()),
                    None,
                    ring_row(
                        percent,
                        context_color(percent),
                        next.map(|next| ("Next", next)),
                        used,
                    ),
                    fullest.title.clone(),
                )
            }
            None => tile(
                label_row("Context", None),
                None,
                value_row("–".into(), None),
                if agents.is_empty() {
                    "No agents running".to_owned()
                } else {
                    "Waiting for a Claude or Codex turn".to_owned()
                },
            ),
        };

        let tokens_tile = match self.activity.history {
            Some(history) => tile(
                label_row("Today", None),
                None,
                div()
                    .flex()
                    .items_end()
                    .justify_between()
                    .gap(px(10.0))
                    .child(big_value(token_label(history.today())))
                    .child(day_bars(&history)),
                format!("tokens · {} in 7 days", token_label(history.total())),
            ),
            None => tile(
                label_row("Today", None),
                None,
                value_row("–".into(), None),
                "Counting tokens…",
            ),
        };

        div()
            .flex()
            .flex_wrap()
            .gap(px(10.0))
            .child(running)
            .child(agents_tile)
            .child(context_tile)
            .children(self.usage.glances().into_iter().map(quota_tile))
            .child(tokens_tile)
    }

    fn spaces_section(&self, spaces: Vec<SpaceSummary>, cx: &mut Context<Self>) -> Div {
        let count = spaces.len();
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(section_heading("Spaces", count))
            .when(spaces.is_empty(), |section| {
                section.child(empty_note("No spaces open"))
            })
            .children(spaces.into_iter().map(|space| self.space_row(space, cx)))
    }

    fn space_row(&self, space: SpaceSummary, cx: &mut Context<Self>) -> AnyElement {
        let SpaceSummary {
            project_id,
            name,
            activity,
            memory,
            servers,
            agents,
        } = space;
        let detail = if !agents.is_empty() {
            None
        } else if servers.is_empty() {
            Some("Nothing running".to_owned())
        } else {
            let mut names: Vec<&str> = servers.iter().map(|server| server.name.as_str()).collect();
            names.dedup();
            Some(names.join(" · "))
        };
        let id = format!("activity-space-{project_id}");
        row_frame(id.clone(), true)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.select_project(project_id, window, cx);
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(ripple_tile(&name, activity, ROW_TILE_SIZE))
                    .child(row_titles(name, detail))
                    .child(port_chips(&id, &servers))
                    .child(memory_column(memory)),
            )
            .children(agents.into_iter().map(|agent| self.agent_line(agent, cx)))
            .into_any_element()
    }

    /// An agent under its space: its task, state, and how full its context is.
    fn agent_line(&self, agent: SpaceAgent, cx: &mut Context<Self>) -> AnyElement {
        let SpaceAgent {
            session_id,
            kind,
            title,
            state,
            attention,
            since,
            fill,
        } = agent;
        let status = state.map(|state| {
            let label = agent_state_short_label(state, attention);
            match since {
                Some(since) => format!("{label} · {}", elapsed_label(since.elapsed())),
                None => label.to_owned(),
            }
        });
        let status_color = match state {
            Some(AgentRuntimeState::Waiting) => {
                agent_status_color(state, attention).unwrap_or_else(|| colors().subtle)
            }
            _ => colors().subtle,
        };
        div()
            .id(SharedString::from(format!("activity-agent-{session_id}")))
            // Under the space's name, past its tile.
            .ml(px(ROW_TILE_SIZE + 10.0))
            .h(px(24.0))
            .px(px(6.0))
            .rounded(px(6.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .cursor_pointer()
            .hover(|line| line.bg(hover_fill()))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_pane(session_id, window, cx);
                cx.stop_propagation();
            }))
            .child(agent_tile(kind.as_deref(), AGENT_TILE_SIZE))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_size(px(12.0))
                    .text_color(colors().foreground)
                    .child(title),
            )
            .when_some(status, |line, status| {
                line.child(
                    div()
                        .flex_none()
                        .text_size(px(11.5))
                        .text_color(status_color)
                        .child(status),
                )
            })
            .child(context_meter(fill))
            .into_any_element()
    }

    fn outside_section(&self, groups: Vec<OutsideGroup>) -> Div {
        let home = self.home_folder();
        let count = groups.len();
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(section_heading("Outside Vibra", count))
            .children(groups.into_iter().map(|group| {
                let name = group
                    .root
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "~".to_owned());
                let path = format_sidebar_path(&group.root.to_string_lossy(), Some(&home));
                let agents: Vec<&str> = group
                    .agents
                    .iter()
                    .map(|kind| kind.display_name())
                    .collect();
                let detail = match agents.as_slice() {
                    [] => format!("{path} · started in another app"),
                    names => format!("{path} · {} in another app", names.join(", ")),
                };
                let id = format!("activity-outside-{}", group.root.to_string_lossy());
                row_frame(id.clone(), false).child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .child(match group.agents.first() {
                            Some(kind) => agent_tile(Some(kind.display_name()), ROW_TILE_SIZE),
                            None => icon_tile("chrome-icons/folder.svg", ROW_TILE_SIZE),
                        })
                        .child(row_titles(name, Some(detail)))
                        .child(port_chips(&id, &group.servers))
                        .child(memory_column(Some(group.memory))),
                )
            }))
    }
}

/// A tile: a label with an optional accessory, the reading, and a caption.
fn tile(
    label: impl IntoElement,
    accessory: Option<AnyElement>,
    body: impl IntoElement,
    caption: impl Into<SharedString>,
) -> Div {
    div()
        .flex_1()
        .min_w(px(TILE_MIN_WIDTH))
        .h(px(TILE_HEIGHT))
        .px(px(14.0))
        .py(px(12.0))
        .rounded(px(TILE_RADIUS))
        .border_1()
        .border_color(colors().border_subtle)
        .bg(tile_fill())
        .flex()
        .flex_col()
        .justify_between()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(label)
                .child(div().flex_1())
                .children(accessory),
        )
        .child(body)
        .child(
            div()
                .truncate()
                .text_size(px(11.5))
                .text_color(colors().subtle)
                .child(caption.into()),
        )
}

/// A tile's label, after the mark of the agent it is about.
fn label_row(label: &'static str, kind: Option<&str>) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(colors().muted)
        .when_some(kind, |row, kind| {
            row.child(agent_tile(Some(kind), AGENT_TILE_SIZE))
        })
        .child(label)
}

fn big_value(value: impl Into<SharedString>) -> Div {
    tabular(div())
        .text_size(px(28.0))
        .line_height(px(32.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(colors().foreground)
        .child(value.into())
}

/// A reading with, optionally, a smaller one at the far end.
fn value_row(value: String, aside: Option<String>) -> Div {
    div()
        .flex()
        .items_end()
        .justify_between()
        .gap(px(8.0))
        .child(big_value(value))
        .when_some(aside, |row, aside| {
            row.child(
                tabular(div())
                    .pb(px(4.0))
                    .text_size(px(13.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors().muted)
                    .child(aside),
            )
        })
}

/// A ring for `percent` beside a second reading and a note.
fn ring_row(
    percent: f64,
    color: Rgba,
    second: Option<(&'static str, String)>,
    note: Option<String>,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.0))
        .child(ring(percent, color))
        .child(
            div()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(1.0))
                .when_some(second, |column, (label, value)| {
                    column
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(colors().subtle)
                                .child(label),
                        )
                        .child(
                            tabular(div())
                                .text_size(px(17.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors().foreground)
                                .child(value),
                        )
                })
                .when_some(note, |column, note| {
                    column.child(
                        tabular(div())
                            .truncate()
                            .text_size(px(11.5))
                            .text_color(colors().subtle)
                            .child(note),
                    )
                }),
        )
}

/// A quota: the session window on the ring, the week beside it.
fn quota_tile(glance: QuotaGlance) -> Div {
    let QuotaGlance {
        id,
        name,
        session,
        weekly,
        reset,
        stale,
    } = glance;
    let (window, percent, second) = match (session, weekly) {
        (Some(session), weekly) => (
            "5-hour",
            session,
            weekly.map(|weekly| ("Week", format!("{weekly:.0}%"))),
        ),
        (None, Some(weekly)) => ("Week", weekly, None),
        (None, None) => ("Usage", 0.0, None),
    };
    let caption = match (reset, stale) {
        (_, true) => "Outdated, retrying".to_owned(),
        (Some(reset), false) => reset,
        (None, false) => String::new(),
    };
    tile(
        div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .text_size(px(12.0))
            .font_weight(FontWeight::MEDIUM)
            .text_color(colors().muted)
            .child(agent_tile(provider_kind(&id), AGENT_TILE_SIZE))
            .child(format!("{name} · {window}")),
        None,
        ring_row(percent, quota_color(percent), second, None),
        caption,
    )
}

/// Tokens per day over the week, today in the accent.
fn day_bars(history: &TokenHistory) -> Div {
    const BAR_HEIGHT: f32 = 34.0;
    let most = history.days.iter().copied().max().unwrap_or(0).max(1) as f32;
    div()
        .h(px(BAR_HEIGHT))
        .flex()
        .items_end()
        .gap(px(3.0))
        .children(history.days.iter().enumerate().map(|(index, tokens)| {
            let share = *tokens as f32 / most;
            div()
                .w(px(8.0))
                .h(px((BAR_HEIGHT * share).max(3.0)))
                .rounded(px(2.0))
                .bg(if index == TOKEN_DAYS - 1 {
                    colors().accent
                } else {
                    mix(colors().background, colors().foreground, 0.16)
                })
        }))
}

/// A ring filled clockwise from the top, with its percent inside.
fn ring(percent: f64, color: Rgba) -> Div {
    let fraction = (percent / 100.0).clamp(0.0, 1.0) as f32;
    let track = mix(colors().background, colors().foreground, 0.1);
    div()
        .relative()
        .size(px(RING_SIZE))
        .flex_none()
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let center = bounds.center();
                    let radius = (f32::from(bounds.size.width) - RING_STROKE) / 2.0;
                    paint_arc(window, center, radius, 1.0, track);
                    if fraction > 0.0 {
                        paint_arc(window, center, radius, fraction, color);
                    }
                },
            )
            .absolute()
            .size_full(),
        )
        .child(
            tabular(div())
                .absolute()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors().foreground)
                .child(format!("{percent:.0}%")),
        )
}

fn paint_arc(window: &mut Window, center: Point<Pixels>, radius: f32, fraction: f32, color: Rgba) {
    let steps = ((fraction * 120.0).ceil() as usize).max(2);
    let mut path = PathBuilder::stroke(px(RING_STROKE));
    for step in 0..=steps {
        let angle = -PI / 2.0 + 2.0 * PI * fraction * step as f32 / steps as f32;
        let at = point(
            center.x + px(radius * angle.cos()),
            center.y + px(radius * angle.sin()),
        );
        if step == 0 {
            path.move_to(at);
        } else {
            path.line_to(at);
        }
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

/// How full a context is, as a short bar and its percent.
fn context_meter(fill: Option<ContextFill>) -> Div {
    let percent = fill.map(ContextFill::percent);
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(
            div()
                .w(px(CONTEXT_BAR_WIDTH))
                .h(px(4.0))
                .rounded(px(2.0))
                .bg(mix(colors().background, colors().foreground, 0.1))
                .when_some(percent, |track, percent| {
                    track.child(
                        div()
                            .h_full()
                            .w(px(CONTEXT_BAR_WIDTH * (percent / 100.0) as f32))
                            .rounded(px(2.0))
                            .bg(context_color(percent)),
                    )
                }),
        )
        .child(
            tabular(div())
                .w(px(34.0))
                .text_right()
                .text_size(px(11.5))
                .text_color(colors().muted)
                .child(percent.map_or_else(|| "–".to_owned(), |percent| format!("{percent:.0}%"))),
        )
}

/// A list row; one that does something lightens under the pointer.
fn row_frame(id: String, interactive: bool) -> Stateful<Div> {
    div()
        .id(SharedString::from(id))
        .px(px(10.0))
        .py(px(9.0))
        .rounded(px(10.0))
        .flex()
        .flex_col()
        .gap(px(6.0))
        .when(interactive, |row| {
            row.cursor_pointer().hover(|row| row.bg(hover_fill()))
        })
}

fn row_titles(name: String, detail: Option<String>) -> Div {
    div()
        .flex_1()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .child(
            div()
                .truncate()
                .text_size(px(13.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors().foreground)
                .child(name),
        )
        .when_some(detail, |titles, detail| {
            titles.child(
                div()
                    .truncate()
                    .text_size(px(11.5))
                    .text_color(colors().subtle)
                    .child(detail),
            )
        })
}

/// A chip per port; each opens the server in the browser.
fn port_chips(row: &str, servers: &[Server]) -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(4.0))
        .children(servers.iter().flat_map(|server| {
            server.ports.iter().map(move |port| {
                let port = *port;
                let tooltip = format!("Open localhost:{port} · {}", server.name);
                tabular(div())
                    .id(SharedString::from(format!("{row}-port-{port}")))
                    .h(px(22.0))
                    .px(px(7.0))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(colors().border_subtle)
                    .flex()
                    .items_center()
                    .text_size(px(11.5))
                    .text_color(colors().muted)
                    .cursor_pointer()
                    .hover(|chip| chip.bg(hover_fill()).text_color(colors().foreground))
                    .tooltip(move |_, cx| sidebar_tooltip(tooltip.clone(), cx))
                    .on_click(move |_, _, cx| {
                        cx.open_url(&format!("http://localhost:{port}"));
                        cx.stop_propagation();
                    })
                    .child(format!(":{port}"))
            })
        }))
}

fn memory_column(memory: Option<u64>) -> Div {
    tabular(div())
        .flex_none()
        .w(px(MEMORY_COLUMN_WIDTH))
        .text_right()
        .text_size(px(12.0))
        .text_color(colors().muted)
        .child(memory.map_or_else(|| "–".to_owned(), memory_label))
}

fn section_heading(title: &'static str, count: usize) -> Div {
    div()
        .px(px(10.0))
        .flex()
        .items_center()
        .gap(px(6.0))
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(colors().muted)
        .child(title)
        .child(
            tabular(div())
                .text_color(colors().subtle)
                .child(count.to_string()),
        )
}

fn empty_note(text: &'static str) -> Div {
    div()
        .px(px(10.0))
        .text_size(px(12.0))
        .text_color(colors().subtle)
        .child(text)
}

/// Green while a context has room, warmer as it fills toward compaction.
fn context_color(percent: f64) -> Rgba {
    match percent {
        value if value >= 90.0 => colors().danger,
        value if value >= 70.0 => colors().warning,
        _ => colors().accent,
    }
}

fn quota_color(percent: f64) -> Rgba {
    match percent {
        value if value >= 95.0 => colors().danger,
        value if value >= 80.0 => colors().warning,
        _ => colors().accent,
    }
}

fn tile_fill() -> Rgba {
    mix(colors().background, colors().foreground, 0.03)
}

fn hover_fill() -> Rgba {
    mix(colors().background, colors().foreground, 0.05)
}

/// The interface font with tabular figures, so readings hold their width.
fn tabular(element: Div) -> Div {
    element.font(Font {
        family: ".SystemUIFont".into(),
        features: FontFeatures(Arc::new(vec![("tnum".into(), 1)])),
        fallbacks: None,
        weight: FontWeight::NORMAL,
        style: FontStyle::Normal,
    })
}
