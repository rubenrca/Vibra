//! Automations: a command line the user saved, run on demand or on a
//! schedule. Each run opens a new session in the project and types the
//! command into its shell, so any CLI works and its output stays visible.

use std::time::Duration;

use gpui::{AnyElement, Context, KeyDownEvent, SharedString, Timer, Window, div, prelude::*, px};
use uuid::Uuid;

use crate::domain::inbox::{InboxKind, relative_time};
use crate::domain::library::{Automation, AutomationSchedule};
use crate::infrastructure::library::{local_minute_now, unix_now};
use crate::ui::theme::{MONO_FONT, colors, surface_tint};

use super::navigation::{project_color, section_button, section_empty_state, section_frame};
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
    fn fields(&self) -> Vec<AutomationField> {
        let mut fields = vec![AutomationField::Name, AutomationField::Command];
        if self.schedule != AutomationSchedule::Manual {
            fields.push(AutomationField::Time);
        }
        fields
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
        let projects: Vec<Option<Uuid>> = std::iter::once(None)
            .chain(
                self.snapshot
                    .projects
                    .iter()
                    .map(|project| Some(project.id)),
            )
            .collect();
        if let Some(form) = self.automation_form.as_mut() {
            let index = projects
                .iter()
                .position(|project| *project == form.project_id)
                .unwrap_or(0);
            form.project_id = projects[(index + 1) % projects.len()];
            cx.notify();
        }
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

    /// Opens a new session named after the automation and types its command.
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
            .filter(|id| {
                self.snapshot
                    .projects
                    .iter()
                    .any(|project| project.id == *id)
            })
            .or(self.snapshot.selected_project_id);
        let Some(project_id) = project_id else {
            self.report_automation_failure(
                &automation,
                "No hay un proyecto donde ejecutarla.",
                reveal,
            );
            return None;
        };
        match self.run_in_new_session(
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
                    format!("{} se está ejecutando", automation.name),
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
                    format!("{} no pudo iniciar", automation.name),
                    "La terminal no aceptó el comando.".to_owned(),
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

    /// Opens a session named `title` in the project and types `command` into
    /// its shell. Returns the new terminal and whether the shell took the
    /// command. With `reveal` off the user's current session stays selected.
    pub(super) fn run_in_new_session(
        &mut self,
        project_id: Uuid,
        title: &str,
        command: &str,
        reveal: bool,
        cx: &mut Context<Self>,
    ) -> Result<(Uuid, bool), &'static str> {
        let previous = self.snapshot.selected_project_id.zip(
            self.snapshot
                .selected_workspace()
                .map(|workspace| workspace.id),
        );
        let workspace_id = self
            .snapshot
            .create_workspace_in_project(project_id)
            .ok_or("El proyecto no tiene una carpeta asociada.")?;
        self.snapshot
            .rename_workspace(project_id, workspace_id, title);
        let session_id = self
            .snapshot
            .projects
            .iter()
            .find(|project| project.id == project_id)
            .and_then(|project| project.workspaces.as_deref())
            .and_then(|workspaces| {
                workspaces
                    .iter()
                    .find(|workspace| workspace.id == workspace_id)
            })
            .and_then(|workspace| workspace.tabs.first())
            .and_then(|tab| tab.sessions.first())
            .map(|session| session.id)
            .ok_or("No se pudo abrir la sesión.")?;
        if !reveal && let Some((project, workspace)) = previous {
            self.snapshot.select_workspace(project, workspace);
        }
        self.reconcile_terminal_views(cx);
        let started = self
            .terminals
            .get(&session_id)
            .cloned()
            .is_some_and(|terminal| {
                terminal.update(cx, |terminal, cx| terminal.run_command(command, cx))
            });
        if reveal {
            self.leave_library_section(WorkspaceSection::Workspace);
            self.workspace_section = WorkspaceSection::Workspace;
            self.diff_view
                .update(cx, |diff, cx| diff.set_review_expanded(false, cx));
            self.sync_terminal_surface_visibility(cx);
            self.sync_diff_root(cx);
            self.sync_git_panel_visibility(cx);
            self.refresh_project_files(cx);
            self.pending_focus_session = Some(session_id);
        }
        self.persist(cx);
        Ok((session_id, started))
    }

    fn report_automation_failure(&mut self, automation: &Automation, reason: &str, seen: bool) {
        self.inbox.push(
            InboxKind::AutomationFailed,
            None,
            format!("{} no pudo iniciar", automation.name),
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
            section_button("automations-new", "Nueva automatización", true)
                .on_click(
                    cx.listener(|this, _, window, cx| this.open_automation_form(None, window, cx)),
                )
                .into_any_element(),
        ];
        let mut body = div().w_full().max_w(px(760.0)).flex().flex_col().gap_2();
        if let Some(form) = self.automation_form.clone() {
            body = body.child(self.automation_form_card(&form, cx));
        }
        if self.library.automations.is_empty() && self.automation_form.is_none() {
            body = body.child(section_empty_state(
                "chrome-icons/automations.svg",
                "Tus tareas recurrentes",
                "Guarda un comando (por ejemplo claude -p \"resume los cambios de ayer\" o cargo test) y ejecútalo cuando quieras o a una hora fija. Cada ejecución abre una sesión nueva en el proyecto, así ves la salida y puedes seguir trabajando en esa terminal.",
            ));
        }
        for automation in self.library.automations.clone() {
            body = body.child(self.automation_row(&automation, now, cx));
        }
        section_frame("Automations", actions, body.into_any_element())
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
                .projects
                .iter()
                .find(|project| project.id == project_id)
                .map(|project| (project_id, project.name.clone()))
        });
        let schedule = if automation.schedule == AutomationSchedule::Manual {
            "Manual".to_owned()
        } else if automation.enabled {
            automation.schedule.label()
        } else {
            format!("{} · pausada", automation.schedule.label())
        };
        let last_run = automation
            .last_run_at
            .map(|at| format!("Última: {}", relative_time(now, at)))
            .unwrap_or_else(|| "Nunca ejecutada".to_owned());
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
                                    .unwrap_or_else(|| "Proyecto seleccionado".to_owned())
                            )),
                    ),
            )
            .when(automation.schedule != AutomationSchedule::Manual, |row| {
                row.child(
                    section_button(
                        SharedString::from(format!("automation-toggle-{id}")),
                        if automation.enabled {
                            "Pausar"
                        } else {
                            "Reanudar"
                        },
                        false,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_automation(id, cx))),
                )
            })
            .child(
                section_button(
                    SharedString::from(format!("automation-edit-{id}")),
                    "Editar",
                    false,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_automation_form(Some(id), window, cx)
                })),
            )
            .child(
                section_button(
                    SharedString::from(format!("automation-delete-{id}")),
                    "Eliminar",
                    false,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.delete_automation(id, cx))),
            )
            .child(
                section_button(
                    SharedString::from(format!("automation-run-{id}")),
                    "Ejecutar",
                    true,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.run_automation(id, None, true, cx);
                })),
            )
            .into_any_element()
    }

    fn automation_form_card(&self, form: &AutomationForm, cx: &mut Context<Self>) -> AnyElement {
        let project = form.project_id.and_then(|project_id| {
            self.snapshot
                .projects
                .iter()
                .find(|project| project.id == project_id)
                .map(|project| project.name.clone())
        });
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
            AutomationSchedule::Hourly { .. } => "Minuto de cada hora (MM)",
            _ => "Hora (HH:MM)",
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
                        "Editar automatización"
                    } else {
                        "Nueva automatización"
                    }),
            )
            .child(field(
                "automation-field-name",
                "Nombre",
                &form.name,
                "Resumen diario",
                AutomationField::Name,
                false,
                cx,
            ))
            .child(field(
                "automation-field-command",
                "Comando (se escribe en una terminal nueva y se ejecuta)",
                &form.command,
                "claude \"revisa los TODO del proyecto\"",
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
                                    .child("Proyecto"),
                            )
                            .child(
                                section_button(
                                    "automation-field-project",
                                    project.unwrap_or_else(|| "Proyecto seleccionado".to_owned()),
                                    false,
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.cycle_form_project(cx))),
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
                                    .child("Cuándo"),
                            )
                            .child(
                                section_button(
                                    "automation-field-schedule",
                                    match form.schedule {
                                        AutomationSchedule::Manual => "Manual",
                                        AutomationSchedule::Hourly { .. } => "Cada hora",
                                        AutomationSchedule::Daily { .. } => "Todos los días",
                                        AutomationSchedule::Weekdays { .. } => "Lunes a viernes",
                                    },
                                    false,
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.cycle_form_schedule(cx))),
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
                            .child("Tab cambia de campo · ↩ guarda · Esc cancela. Las programadas corren mientras Vibra está abierta."),
                    )
                    .child(
                        section_button("automation-cancel", "Cancelar", false).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.automation_form = None;
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        section_button("automation-save", "Guardar", true)
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
