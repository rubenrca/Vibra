//! Automations: a command line the user saved, run on demand or on a
//! schedule. Each run opens a new tab in the project and types the
//! command into its shell, so any CLI works and its output stays visible.

use std::time::Duration;

use gpui::{AnyElement, Context, KeyDownEvent, SharedString, Timer, Window, div, prelude::*, px};
use uuid::Uuid;

use crate::domain::inbox::{InboxKind, relative_time};
use crate::domain::library::{Automation, AutomationSchedule};
use crate::infrastructure::library::{local_minute_now, unix_now};
use crate::ui::theme::{MONO_FONT, colors, surface_tint};

use super::navigation::{project_color, section_button, section_empty_state, section_frame};
use super::projects::next_library_project;
use super::{WorkspaceSection, WorkspaceView};
use crate::ui::text_edit::{TextKeyOutcome, apply_text_key};

const SCHEDULER_INTERVAL: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AutomationField {
    Name,
    Command,
    Time,
}

#[derive(Debug, Clone)]
pub(super) struct AutomationForm {
    id: Option<Uuid>,
    name: String,
    command: String,
    time: String,
    schedule: AutomationSchedule,
    project_id: Option<Uuid>,
    field: AutomationField,
    error: Option<SharedString>,
}

impl AutomationForm {
    fn fields(&self) -> &[AutomationField] {
        if self.schedule == AutomationSchedule::Manual {
            &[AutomationField::Name, AutomationField::Command]
        } else {
            &[
                AutomationField::Name,
                AutomationField::Command,
                AutomationField::Time,
            ]
        }
    }

    fn move_field(&mut self, offset: isize) {
        let fields = self.fields();
        let index = fields
            .iter()
            .position(|field| *field == self.field)
            .unwrap_or(0) as isize;
        let next = (index + offset).rem_euclid(fields.len() as isize) as usize;
        self.field = fields[next];
    }

    fn buffer(&mut self) -> &mut String {
        match self.field {
            AutomationField::Name => &mut self.name,
            AutomationField::Command => &mut self.command,
            AutomationField::Time => &mut self.time,
        }
    }
}

impl WorkspaceView {
    pub(super) fn open_automation_form(
        &mut self,
        id: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing = id.and_then(|id| self.library.automation(id)).cloned();
        let form = match existing {
            Some(automation) => AutomationForm {
                id: Some(automation.id),
                name: automation.name,
                command: automation.command,
                time: automation.schedule.time_text(),
                schedule: automation.schedule,
                project_id: automation.project_id,
                field: AutomationField::Name,
                error: None,
            },
            None => AutomationForm {
                id: None,
                name: String::new(),
                command: String::new(),
                time: String::new(),
                schedule: AutomationSchedule::Manual,
                project_id: self.snapshot.selected_project_id,
                field: AutomationField::Name,
                error: None,
            },
        };
        if self.workspace_section != WorkspaceSection::Automations {
            self.select_section(WorkspaceSection::Automations, window, cx);
        }
        self.automation_form = Some(form);
        cx.notify();
    }

    fn save_automation_form(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.automation_form.as_mut() else {
            return;
        };
        match self.library.save_automation(
            form.id,
            &form.name,
            &form.command,
            form.project_id,
            form.schedule,
            &form.time,
        ) {
            Ok(_) => {
                self.automation_form = None;
                self.persist_library(cx);
            }
            Err(error) => {
                form.error = Some(error.message().into());
                cx.notify();
            }
        }
    }

    fn cycle_form_schedule(&mut self, cx: &mut Context<Self>) {
        if let Some(form) = self.automation_form.as_mut() {
            let current = form.schedule.with_time(&form.time).unwrap_or(form.schedule);
            form.schedule = current.next_kind();
            form.time = form.schedule.time_text();
            if form.field == AutomationField::Time && form.schedule == AutomationSchedule::Manual {
                form.field = AutomationField::Command;
            }
            form.error = None;
            cx.notify();
        }
    }

    fn cycle_form_project(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.automation_form.as_mut() else {
            return;
        };
        form.project_id = next_library_project(&self.snapshot.projects, form.project_id);
        cx.notify();
    }

    pub(super) fn handle_automation_form_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.workspace_section != WorkspaceSection::Automations {
            return false;
        }
        let Some(form) = self.automation_form.as_mut() else {
            return false;
        };
        let outcome = apply_text_key(
            form.buffer(),
            event.keystroke.key.as_str(),
            event.keystroke.key_char.as_deref(),
            &event.keystroke.modifiers,
            false,
            || cx.read_from_clipboard().and_then(|item| item.text()),
        );
        match outcome {
            TextKeyOutcome::Edited => {
                form.error = None;
                cx.notify();
            }
            TextKeyOutcome::NextField => {
                form.move_field(1);
                cx.notify();
            }
            TextKeyOutcome::PreviousField => {
                form.move_field(-1);
                cx.notify();
            }
            TextKeyOutcome::Submit => self.save_automation_form(cx),
            TextKeyOutcome::Cancel => {
                self.automation_form = None;
                cx.notify();
            }
            TextKeyOutcome::Unhandled => return false,
        }
        true
    }

    fn delete_automation(&mut self, id: Uuid, cx: &mut Context<Self>) {
        if self.library.delete_automation(id) {
            if self
                .automation_form
                .as_ref()
                .is_some_and(|form| form.id == Some(id))
            {
                self.automation_form = None;
            }
            self.persist_library(cx);
        }
    }

    fn toggle_automation(&mut self, id: Uuid, cx: &mut Context<Self>) {
        if self.library.toggle_automation(id) {
            self.persist_library(cx);
        }
    }

    /// Opens a new tab named after the automation and types its command.
    /// Scheduled runs keep the user's current selection; manual runs show it.
    pub(super) fn run_automation(
        &mut self,
        id: Uuid,
        slot: Option<i64>,
        reveal: bool,
        cx: &mut Context<Self>,
    ) -> Option<Uuid> {
        let automation = self.library.automation(id).cloned()?;
        let now = unix_now();
        // A scheduled occurrence is attempted once, even when it cannot start.
        self.library.record_automation_run(id, now, slot);
        self.persist_library(cx);
        let project_id = automation
            .project_id
            .or(self.snapshot.selected_project_id)
            .filter(|id| self.snapshot.project(*id).is_some());
        let Some(project_id) = project_id else {
            self.report_automation_failure(
                &automation,
                "No project available to run this automation.",
                reveal,
            );
            return None;
        };
        match self.run_in_new_tab(
            project_id,
            &automation.name,
            &automation.command,
            reveal,
            cx,
        ) {
            Ok((session_id, true)) => {
                let detail = self.session_location_label(session_id);
                self.inbox.push(
                    InboxKind::AutomationStarted,
                    Some(session_id),
                    format!("{} is running", automation.name),
                    detail,
                    now,
                    reveal,
                );
                Some(session_id)
            }
            Ok((session_id, false)) => {
                self.inbox.push(
                    InboxKind::AutomationFailed,
                    Some(session_id),
                    format!("{} could not start", automation.name),
                    "The terminal did not accept the command.".to_owned(),
                    now,
                    reveal,
                );
                Some(session_id)
            }
            Err(reason) => {
                self.report_automation_failure(&automation, reason, reveal);
                None
            }
        }
    }

    /// Opens a tab named `title` in the project and types `command` into its
    /// shell. Returns the new terminal and whether the shell took the
    /// command. With `reveal` off the user's current tab stays selected.
    pub(super) fn run_in_new_tab(
        &mut self,
        project_id: Uuid,
        title: &str,
        command: &str,
        reveal: bool,
        cx: &mut Context<Self>,
    ) -> Result<(Uuid, bool), &'static str> {
        let (_, session_id) = self
            .snapshot
            .open_tab_in_project(project_id, reveal)
            .ok_or("The project has no associated folder.")?;
        // The tab keeps the automation's name while its shell runs.
        self.pane_names.insert(session_id, title.to_owned());
        self.reconcile_terminal_views(cx);
        let started = self
            .terminals
            .get(&session_id)
            .cloned()
            .is_some_and(|terminal| {
                terminal.update(cx, |terminal, cx| terminal.run_command(command, cx))
            });
        if reveal {
            self.prepare_terminal_tab(cx);
            self.pending_focus_session = Some(session_id);
        }
        self.persist(cx);
        Ok((session_id, started))
    }

    fn report_automation_failure(&mut self, automation: &Automation, reason: &str, seen: bool) {
        self.inbox.push(
            InboxKind::AutomationFailed,
            None,
            format!("{} could not start", automation.name),
            reason.to_owned(),
            unix_now(),
            seen,
        );
    }

    /// Checks the schedule while the app is open. Missed occurrences (the app
    /// was closed or the Mac slept) are skipped rather than replayed.
    pub(super) fn start_automation_scheduler(&mut self, cx: &mut Context<Self>) {
        self._automation_scheduler = Some(cx.spawn(async move |this, cx| {
            loop {
                Timer::after(SCHEDULER_INTERVAL).await;
                if this
                    .update(cx, |this, cx| this.run_due_automations(cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    fn run_due_automations(&mut self, cx: &mut Context<Self>) {
        for (id, slot) in self.library.due_automations(local_minute_now()) {
            self.run_automation(id, Some(slot), false, cx);
        }
    }

    pub(super) fn automations_content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let now = unix_now();
        let actions = vec![
            section_button("automations-new", "New automation", true)
                .on_click(
                    cx.listener(|this, _, window, cx| this.open_automation_form(None, window, cx)),
                )
                .into_any_element(),
        ];
        let mut body = div().w_full().max_w(px(760.0)).flex().flex_col().gap_2();
        if let Some(form) = &self.automation_form {
            body = body.child(self.automation_form_card(form, cx));
        }
        if self.library.automations.is_empty() && self.automation_form.is_none() {
            body = body.child(section_empty_state(
                "chrome-icons/automations.svg",
                "Your recurring tasks",
                concat!(
                    "Save a command (such as claude -p \"summarize yesterday's changes\" ",
                    "or cargo test) and run it whenever you want or on a schedule. ",
                    "Each run opens a new tab in the project ",
                    "so you can see the output and keep working in that terminal.",
                ),
            ));
        }
        for automation in &self.library.automations {
            body = body.child(self.automation_row(automation, now, cx));
        }
        section_frame(
            "Automations · In development",
            actions,
            body.into_any_element(),
        )
    }

    fn automation_row(
        &self,
        automation: &Automation,
        now: u64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = automation.id;
        let project = automation.project_id.and_then(|project_id| {
            self.snapshot
                .project(project_id)
                .map(|project| (project_id, project.name.clone()))
        });
        let schedule = if automation.schedule == AutomationSchedule::Manual {
            "Manual".to_owned()
        } else if automation.enabled {
            automation.schedule.label()
        } else {
            format!("{} · paused", automation.schedule.label())
        };
        let last_run = automation
            .last_run_at
            .map(|at| format!("Last run: {}", relative_time(now, at)))
            .unwrap_or_else(|| "Never run".to_owned());
        div()
            .id(SharedString::from(format!("automation-{id}")))
            .w_full()
            .p_3()
            .rounded(px(8.0))
            .border_1()
            .border_color(colors().border_subtle)
            .flex()
            .items_center()
            .gap_3()
            .when(!automation.enabled, |row| row.opacity(0.65))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(
                        div()
                            .text_size(px(13.5))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .truncate()
                            .child(automation.name.clone()),
                    )
                    .child(
                        div()
                            .font_family(MONO_FONT)
                            .text_size(px(12.0))
                            .text_color(colors().muted)
                            .truncate()
                            .child(automation.command.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .text_size(px(11.5))
                            .text_color(colors().subtle)
                            .when_some(project.clone(), |meta, (project_id, _)| {
                                meta.child(
                                    div()
                                        .size(px(7.0))
                                        .rounded_full()
                                        .bg(project_color(project_id)),
                                )
                            })
                            .child(format!(
                                "{} · {schedule} · {last_run}",
                                project
                                    .map(|(_, name)| name)
                                    .unwrap_or_else(|| "Selected project".to_owned())
                            )),
                    ),
            )
            .when(automation.schedule != AutomationSchedule::Manual, |row| {
                row.child(
                    section_button(
                        SharedString::from(format!("automation-toggle-{id}")),
                        if automation.enabled {
                            "Pause"
                        } else {
                            "Resume"
                        },
                        false,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_automation(id, cx))),
                )
            })
            .child(
                section_button(
                    SharedString::from(format!("automation-edit-{id}")),
                    "Edit",
                    false,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_automation_form(Some(id), window, cx)
                })),
            )
            .child(
                section_button(
                    SharedString::from(format!("automation-delete-{id}")),
                    "Delete",
                    false,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.delete_automation(id, cx))),
            )
            .child(
                section_button(
                    SharedString::from(format!("automation-run-{id}")),
                    "Run",
                    true,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.run_automation(id, None, true, cx);
                })),
            )
            .into_any_element()
    }

    fn automation_form_card(&self, form: &AutomationForm, cx: &mut Context<Self>) -> AnyElement {
        let project = form
            .project_id
            .and_then(|project_id| self.snapshot.project(project_id))
            .map(|project| project.name.clone());
        let field = |id: &'static str,
                     label: &'static str,
                     value: &str,
                     placeholder: &'static str,
                     which: AutomationField,
                     mono: bool,
                     cx: &mut Context<Self>| {
            let active = form.field == which;
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(colors().subtle)
                        .child(label),
                )
                .child(
                    div()
                        .id(id)
                        .h(px(32.0))
                        .px_2()
                        .rounded(px(6.0))
                        .flex()
                        .items_center()
                        .border_1()
                        .border_color(if active {
                            colors().accent
                        } else {
                            colors().border_subtle
                        })
                        .bg(surface_tint(colors().elevated, colors().background))
                        .cursor_text()
                        .text_size(px(13.0))
                        .when(mono, |input| input.font_family(MONO_FONT))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(form) = this.automation_form.as_mut() {
                                form.field = which;
                                cx.notify();
                            }
                        }))
                        .child(if value.is_empty() && !active {
                            div()
                                .text_color(colors().subtle)
                                .child(placeholder)
                                .into_any_element()
                        } else {
                            div()
                                .min_w(px(0.0))
                                .truncate()
                                .child(if active {
                                    format!("{value}▏")
                                } else {
                                    value.to_owned()
                                })
                                .into_any_element()
                        }),
                )
        };
        let time_label = match form.schedule {
            AutomationSchedule::Hourly { .. } => "Minute of each hour (MM)",
            _ => "Time (HH:MM)",
        };
        div()
            .w_full()
            .p_4()
            .mb_2()
            .rounded(px(10.0))
            .border_1()
            .border_color(colors().border_subtle)
            .bg(surface_tint(colors().panel, colors().background))
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_size(px(13.5))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(if form.id.is_some() {
                        "Edit automation"
                    } else {
                        "New automation"
                    }),
            )
            .child(field(
                "automation-field-name",
                "Name",
                &form.name,
                "Daily summary",
                AutomationField::Name,
                false,
                cx,
            ))
            .child(field(
                "automation-field-command",
                "Command (runs in a new terminal)",
                &form.command,
                "claude \"review the project's TODOs\"",
                AutomationField::Command,
                true,
                cx,
            ))
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(colors().subtle)
                                    .child("Project"),
                            )
                            .child(
                                section_button(
                                    "automation-field-project",
                                    project.unwrap_or_else(|| "Selected project".to_owned()),
                                    false,
                                )
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.cycle_form_project(cx)),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(colors().subtle)
                                    .child("When"),
                            )
                            .child(
                                section_button(
                                    "automation-field-schedule",
                                    match form.schedule {
                                        AutomationSchedule::Manual => "Manual",
                                        AutomationSchedule::Hourly { .. } => "Hourly",
                                        AutomationSchedule::Daily { .. } => "Daily",
                                        AutomationSchedule::Weekdays { .. } => "Weekdays",
                                    },
                                    false,
                                )
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.cycle_form_schedule(cx)),
                                ),
                            ),
                    )
                    .when(form.schedule != AutomationSchedule::Manual, |row| {
                        row.child(div().w(px(180.0)).child(field(
                            "automation-field-time",
                            time_label,
                            &form.time,
                            "09:00",
                            AutomationField::Time,
                            true,
                            cx,
                        )))
                    }),
            )
            .when_some(form.error.clone(), |card, error| {
                card.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(colors().danger)
                        .child(error),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.5))
                            .text_color(colors().subtle)
                            .child(concat!(
                                "Tab switches fields · ↩ saves · Esc cancels. ",
                                "Scheduled tasks run while Vibra is open.",
                            )),
                    )
                    .child(
                        section_button("automation-cancel", "Cancel", false).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.automation_form = None;
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        section_button("automation-save", "Save", true)
                            .on_click(cx.listener(|this, _, _, cx| this.save_automation_form(cx))),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_fields_skip_the_time_for_manual_runs() {
        let mut form = AutomationForm {
            id: None,
            name: String::new(),
            command: String::new(),
            time: String::new(),
            schedule: AutomationSchedule::Manual,
            project_id: None,
            field: AutomationField::Command,
            error: None,
        };
        form.move_field(1);
        assert_eq!(form.field, AutomationField::Name);
        form.schedule = AutomationSchedule::Daily { hour: 9, minute: 0 };
        form.field = AutomationField::Command;
        form.move_field(1);
        assert_eq!(form.field, AutomationField::Time);
        form.move_field(-2);
        assert_eq!(form.field, AutomationField::Name);
    }
}
