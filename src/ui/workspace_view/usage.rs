//! Compact subscription usage in the status bar, with a read-only detail popover.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    AnyElement, Context, MouseButton, Rgba, SharedString, Task, Timer, Window, div, prelude::*, px,
    relative, svg,
};

use crate::domain::usage::{
    ProviderUsage, UsageSnapshot, now_timestamp, reset_label, resource_label, timestamp,
};
use crate::infrastructure::usage::UsageMonitor;
use crate::ui::theme::{colors, popover_surface, surface_tint};

use super::{WorkspaceView, sidebar_tooltip};

const POLL_INTERVAL: Duration = Duration::from_secs(30);
const BAR_PROVIDER_LIMIT: usize = 3;

#[derive(Default)]
pub(super) struct UsageState {
    monitor: Arc<Mutex<UsageMonitor>>,
    snapshot: UsageSnapshot,
    error: Option<String>,
    loading: bool,
    pub open: bool,
    _poll_task: Option<Task<()>>,
    _refresh_task: Option<Task<()>>,
}

impl UsageState {
    fn apply(&mut self, result: anyhow::Result<UsageSnapshot>) {
        self.loading = false;
        match result {
            Ok(snapshot) => {
                self.snapshot = snapshot;
                self.error = None;
            }
            Err(error) => self.error = Some(error.to_string()),
        }
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
                        .map_err(|_| anyhow::anyhow!("No se pudo actualizar el monitor de cuotas."))
                        .map(|mut monitor| monitor.refresh(manual))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.usage.apply(result);
                cx.notify();
            });
        }));
    }

    pub(super) fn close_usage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.usage.open = false;
        self.focus_selected_terminal(window, cx);
        cx.notify();
    }

    pub(super) fn usage_status(&self, cx: &mut Context<Self>) -> AnyElement {
        let now = now_timestamp();
        let tooltip = self.usage.error.clone().unwrap_or_else(|| {
            if self.usage.snapshot.providers.is_empty() {
                "Inicia sesión en tus proveedores para ver las cuotas".to_owned()
            } else {
                "Uso de suscripciones IA · porcentaje consumido".to_owned()
            }
        });
        div()
            .id("status-usage")
            .h(px(22.0))
            .min_w(px(0.0))
            .max_w(relative(0.5))
            .px(px(7.0))
            .rounded(px(6.0))
            .flex()
            .items_center()
            .gap(px(14.0))
            .overflow_hidden()
            .cursor_pointer()
            .hover(|item| item.bg(surface_tint(colors().hover, colors().titlebar)))
            .tooltip(move |_, cx| sidebar_tooltip(tooltip.clone(), cx))
            .on_click(cx.listener(|this, _, window, cx| {
                this.usage.open = !this.usage.open;
                if this.usage.open {
                    this.context_menu = None;
                    this.ide_menu_open = false;
                    this.focus_handle.focus(window);
                    this.refresh_usage(false, cx);
                } else {
                    this.focus_selected_terminal(window, cx);
                }
                cx.notify();
            }))
            .when(self.usage.snapshot.providers.is_empty(), |item| {
                item.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_color(colors().subtle)
                        .child(provider_icon("", 16.0, colors().subtle))
                        .child(if self.usage.loading { "…" } else { "—" }),
                )
            })
            .children(
                self.usage
                    .snapshot
                    .providers
                    .iter()
                    .take(BAR_PROVIDER_LIMIT)
                    .map(|(id, provider)| {
                        let stale = self.usage.provider_stale(id, provider, now);
                        let color = if stale {
                            colors().subtle
                        } else {
                            colors().foreground
                        };
                        let quota = provider.tightest_quota();
                        let percentage = quota.and_then(|(_, resource)| resource.percent_used());
                        let tooltip = format!(
                            "{} · {}{}",
                            provider.display_name,
                            quota.map_or_else(
                                || "Sin cuota disponible".to_owned(),
                                |(key, resource)| format!(
                                    "{}: {}",
                                    resource_label(key),
                                    resource.value_label()
                                )
                            ),
                            if stale {
                                " · Datos desactualizados"
                            } else {
                                ""
                            }
                        );
                        div()
                            .id(SharedString::from(format!("status-usage-{id}")))
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .flex_none()
                            .whitespace_nowrap()
                            .text_size(px(13.0))
                            .font_weight(gpui::FontWeight::NORMAL)
                            .text_color(color)
                            .tooltip(move |_, cx| sidebar_tooltip(tooltip.clone(), cx))
                            .child(provider_icon(id, 16.0, color))
                            .child(
                                percentage.map_or_else(
                                    || "—".to_owned(),
                                    |percent| format!("{percent:.0}%"),
                                ),
                            )
                    }),
            )
            .when(
                self.usage.snapshot.providers.len() > BAR_PROVIDER_LIMIT,
                |item| {
                    item.child(format!(
                        "+{}",
                        self.usage.snapshot.providers.len() - BAR_PROVIDER_LIMIT
                    ))
                },
            )
            .when(!self.usage.snapshot.errors.is_empty(), |item| {
                item.child(div().text_color(colors().warning).child("!"))
            })
            .into_any_element()
    }

    pub(super) fn usage_popover(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.usage.open
            || self.settings_open
            || self.palette_mode.is_some()
            || self.rename_prompt.is_some()
        {
            return None;
        }
        let now = now_timestamp();
        let height: f32 = window.bounds().size.height.into();
        let content = div()
            .id("usage-provider-list")
            .min_h(px(0.0))
            .overflow_y_scroll()
            .p(px(14.0))
            .flex()
            .flex_col()
            .gap(px(18.0))
            .when_some(self.usage.error.clone(), |content, error| {
                content.child(div().text_color(colors().warning).child(error))
            })
            .when(
                self.usage.snapshot.providers.is_empty() && self.usage.error.is_none(),
                |content| {
                    content.child(
                        div()
                            .text_color(colors().muted)
                            .child(if self.usage.loading {
                                "Consultando cuotas…"
                            } else {
                                "Inicia sesión con Claude Code, Codex o Grok Build y pulsa Actualizar."
                            }),
                    )
                },
            )
            .children(self.usage.snapshot.providers.iter().map(|(id, provider)| {
                let stale = self.usage.provider_stale(id, provider, now);
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(7.0))
                            .child(provider_icon(id, 13.0, colors().foreground))
                            .child(
                                div()
                                    .min_w(px(0.0))
                                    .flex_1()
                                    .truncate()
                                    .child(provider.display_name.clone()),
                            )
                            .when_some(provider.plan.clone(), |header, plan| {
                                header.child(
                                    div()
                                        .max_w(px(140.0))
                                        .truncate()
                                        .text_color(colors().muted)
                                        .child(plan),
                                )
                            })
                            .when(stale, |header| {
                                header.child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(colors().warning)
                                        .child("Desactualizado"),
                                )
                            }),
                    )
                    .when_some(
                        provider.fetched_at.as_deref().and_then(timestamp),
                        |card, fetched| {
                            let minutes = now.saturating_sub(fetched).max(0) / 60;
                            card.child(div().text_size(px(10.0)).text_color(colors().subtle).child(
                                if minutes == 0 {
                                    "Actualizado hace menos de 1 min".to_owned()
                                } else {
                                    format!("Actualizado hace {minutes} min")
                                },
                            ))
                        },
                    )
                    .when(provider.resources.is_empty(), |card| {
                        card.child(
                            div()
                                .text_color(colors().muted)
                                .child("Sin cuotas disponibles"),
                        )
                    })
                    .children(provider.resources.iter().map(|(key, resource)| {
                        let percent = resource.percent_used();
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(5.0))
                            .child(
                                div()
                                    .flex()
                                    .justify_between()
                                    .gap(px(12.0))
                                    .child(
                                        div()
                                            .min_w(px(0.0))
                                            .truncate()
                                            .text_color(colors().muted)
                                            .child(resource_label(key).to_owned()),
                                    )
                                    .child(resource.value_label()),
                            )
                            .when_some(percent, |row, percent| {
                                row.child(
                                    div()
                                        .h(px(4.0))
                                        .w_full()
                                        .rounded_full()
                                        .overflow_hidden()
                                        .bg(colors().border_subtle)
                                        .child(
                                            div()
                                                .h_full()
                                                .w(relative(
                                                    (percent / 100.0).clamp(0.0, 1.0) as f32
                                                ))
                                                .bg(if stale {
                                                    colors().subtle
                                                } else {
                                                    quota_color(Some(percent))
                                                }),
                                        ),
                                )
                            })
                            .when_some(resource.resets_at.as_deref(), |row, reset| {
                                row.child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(colors().subtle)
                                        .child(reset_label(reset, now)),
                                )
                            })
                    }))
            }))
            .children(self.usage.snapshot.errors.iter().map(|error| {
                div()
                    .text_color(colors().warning)
                    .child(format!("{}: {}", error.provider_id, error.message))
            }));

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
                    div()
                        .id("usage-popover")
                        .absolute()
                        .right(px(8.0))
                        .bottom(px(super::status_bar::STATUS_BAR_HEIGHT + 6.0))
                        .w(px(390.0))
                        .max_w_full()
                        .max_h(px((height - 90.0).max(100.0)))
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .rounded(px(10.0))
                        .border_1()
                        .border_color(colors().border_subtle)
                        .bg(popover_surface())
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap(px(10.0))
                                .p(px(14.0))
                                .border_b_1()
                                .border_color(colors().border_subtle)
                                .child(div().flex_1().child("Uso de suscripciones"))
                                .child(
                                    div()
                                        .id("usage-refresh")
                                        .cursor_pointer()
                                        .text_color(colors().muted)
                                        .hover(|button| button.text_color(colors().foreground))
                                        .tooltip(|_, cx| {
                                            sidebar_tooltip(
                                                "Consultar cuotas · puede solicitar acceso al llavero",
                                                cx,
                                            )
                                        })
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.refresh_usage(true, cx)),
                                        )
                                        .child(if self.usage.loading {
                                            "Consultando…"
                                        } else {
                                            "Actualizar"
                                        }),
                                ),
                        )
                        .child(content)
                        .child(
                            div()
                                .flex_none()
                                .px(px(14.0))
                                .py(px(10.0))
                                .border_t_1()
                                .border_color(colors().border_subtle)
                                .text_size(px(10.0))
                                .text_color(colors().subtle)
                                .child(
                                    "Porcentaje consumido · Consulta directa · Actualización cada 5 min",
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

fn quota_color(percent: Option<f64>) -> Rgba {
    match percent {
        Some(value) if value >= 95.0 => colors().danger,
        Some(value) if value >= 80.0 => colors().warning,
        _ => colors().muted,
    }
}

fn provider_icon(id: &str, size: f32, color: Rgba) -> impl IntoElement {
    let path = if id.starts_with("claude") {
        "agent-marks/claude.svg"
    } else if id.starts_with("codex") {
        "agent-marks/codex.svg"
    } else if id.starts_with("grok") {
        "agent-marks/grok.svg"
    } else if id.starts_with("cursor") {
        "agent-marks/cursor.svg"
    } else {
        "chrome-icons/sparkles.svg"
    };
    // GPUI's SVG painter requires a color on the SVG itself; unlike text,
    // it does not paint with a color inherited only from the parent div.
    svg()
        .path(path)
        .size(px(size))
        .flex_none()
        .text_color(color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_failure_retains_values_and_recovery_replaces_them() {
        let snapshot = serde_json::from_str(
            r#"{"providers":{"codex":{"displayName":"Codex","resources":{}}}}"#,
        )
        .unwrap();
        let mut state = UsageState::default();
        state.apply(Ok(snapshot));
        state.apply(Err(anyhow::anyhow!("Sin conexión")));
        assert!(state.provider_stale("codex", &state.snapshot.providers["codex"], now_timestamp()));
        assert_eq!(state.snapshot.providers.len(), 1);
        state.apply(Ok(UsageSnapshot::default()));
        assert!(state.snapshot.providers.is_empty());
        assert!(state.error.is_none());
        assert!(!state.loading);
    }
}
