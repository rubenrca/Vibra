use gpui::{AnyElement, Context, div, prelude::*, px};

use crate::domain::work_items::WorkSource;
use crate::infrastructure::work_items;
use crate::ui::theme::colors;

use super::super::navigation::section_button;
use super::{SourceFeed, WorkspaceView};

impl WorkspaceView {
    pub(crate) fn inbox_connection_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let connected = self
            .work_inbox
            .linear
            .connected
            .unwrap_or_else(|| !cfg!(test) && work_items::linear_connected());
        div()
            .px_4()
            .py_2()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(div().text_size(px(12.0)).text_color(colors().muted).child(
                        if connected {
                            "Linear connected"
                        } else {
                            "Linear · Personal API key"
                        },
                    ))
                    .child(
                        section_button(
                            "linear-connect",
                            if self.work_inbox.connecting {
                                "Verifying…"
                            } else if connected {
                                "Change key from clipboard"
                            } else {
                                "Connect Linear from clipboard"
                            },
                            false,
                        )
                        .on_click(
                            cx.listener(|this, _, _, cx| this.connect_inbox_linear(false, cx)),
                        ),
                    )
                    .when(connected, |row| {
                        row.child(
                            section_button("linear-disconnect", "Disconnect", false).on_click(
                                cx.listener(|this, _, _, cx| this.connect_inbox_linear(true, cx)),
                            ),
                        )
                    })
                    .child(
                        section_button("linear-key-help", "Get API key ↗", false)
                            .on_click(|_, _, cx| cx.open_url("https://linear.app/settings/api")),
                    ),
            )
            .when_some(self.work_inbox.connection_error.as_ref(), |row, error| {
                row.child(message(error, true))
            })
            .into_any_element()
    }

    pub(crate) fn connect_inbox_linear(&mut self, disconnect: bool, cx: &mut Context<Self>) {
        if self.work_inbox.connecting {
            return;
        }
        if self
            .work_inbox
            .details
            .values()
            .any(|state| state.posting || state.mutation_busy)
        {
            self.work_inbox.connection_error =
                Some("Wait for the Inbox action to finish before changing the connection.".into());
            cx.notify();
            return;
        }
        let token = if disconnect {
            String::new()
        } else {
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .unwrap_or_default()
        };
        if !disconnect && token.trim().is_empty() {
            self.work_inbox.connection_error =
                Some("Copy your Linear personal API key and reconnect.".into());
            cx.notify();
            return;
        }
        self.work_inbox.connecting = true;
        self.work_inbox.connection_error = None;
        self.work_inbox._connection_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    if disconnect {
                        work_items::disconnect_linear()
                    } else {
                        work_items::connect_linear(&token)
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.work_inbox.connecting = false;
                match result {
                    Ok(()) => {
                        this.work_inbox.epoch += 1;
                        this.work_inbox.details.clear();
                        this.work_inbox.confirmation = None;
                        this.work_inbox.comment_editing = false;
                        let generation = this.work_inbox.linear.generation + 1;
                        this.work_inbox.linear = SourceFeed {
                            connected: Some(!disconnect),
                            generation,
                            ..Default::default()
                        };
                        if this.settings.inbox.source == WorkSource::Linear {
                            this.work_inbox.selected = None;
                        }
                        if !disconnect {
                            this.refresh_work_source(WorkSource::Linear, true, cx);
                        }
                    }
                    Err(error) => this.work_inbox.connection_error = Some(format!("{error:#}")),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
}

pub(crate) fn message(text: &str, error: bool) -> gpui::Div {
    div()
        .px_2()
        .py_1()
        .text_size(px(12.0))
        .text_color(if error {
            colors().warning
        } else {
            colors().muted
        })
        .child(text.to_owned())
}
