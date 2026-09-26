//! Notes and automations: the user's own material, kept apart from the
//! workspace layout. Automations only describe a command; running it is a
//! visible terminal session like any other.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const CURRENT_LIBRARY_SCHEMA_VERSION: u32 = 1;
pub const MAX_NOTE_CHARS: usize = 100_000;
pub const MAX_AUTOMATION_NAME_CHARS: usize = 80;
pub const MAX_AUTOMATION_COMMAND_CHARS: usize = 4_000;
/// A scheduled run still starts this late (sleep, a busy launch); later
/// occurrences are skipped instead of replaying a backlog.
pub const AUTOMATION_GRACE_MINUTES: i64 = 10;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Library {
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<Note>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub automations: Vec<Automation>,
}

impl Default for Library {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_LIBRARY_SCHEMA_VERSION,
            notes: Vec::new(),
            automations: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<Uuid>,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub updated_at: u64,
}

impl Note {
    /// The first non-empty line, without Markdown heading marks.
    pub fn title(&self) -> String {
        self.body
            .lines()
            .map(|line| line.trim().trim_start_matches('#').trim())
            .find(|line| !line.is_empty())
            .map(|line| line.chars().take(80).collect())
            .unwrap_or_else(|| "Nota sin título".to_owned())
    }

    /// The first line after the title, for list previews.
    pub fn preview(&self) -> String {
        self.body
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .nth(1)
            .map(|line| line.chars().take(120).collect())
            .unwrap_or_default()
    }

    pub fn is_blank(&self) -> bool {
        self.body.trim().is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AutomationSchedule {
    Manual,
    Hourly { minute: u8 },
    Daily { hour: u8, minute: u8 },
    Weekdays { hour: u8, minute: u8 },
}

impl AutomationSchedule {
    pub fn label(self) -> String {
        match self {
            Self::Manual => "Manual".to_owned(),
            Self::Hourly { minute } => format!("Cada hora, al minuto {minute:02}"),
            Self::Daily { hour, minute } => format!("Todos los días, {hour:02}:{minute:02}"),
            Self::Weekdays { hour, minute } => {
                format!("Lunes a viernes, {hour:02}:{minute:02}")
            }
        }
    }

    /// The latest occurrence at or before `now`, if it falls today (or this
    /// hour for hourly schedules). Returned as a local minute index.
    pub fn latest_occurrence(self, now: LocalMinute) -> Option<i64> {
        let slot = match self {
            Self::Manual => return None,
            Self::Hourly { minute } => now.index - now.index.rem_euclid(60) + i64::from(minute),
            Self::Daily { hour, minute } => {
                now.day_start() + i64::from(hour) * 60 + i64::from(minute)
            }
            Self::Weekdays { hour, minute } => {
                if !(1..=5).contains(&now.weekday) {
                    return None;
                }
                now.day_start() + i64::from(hour) * 60 + i64::from(minute)
            }
        };
        (slot <= now.index).then_some(slot)
    }

    /// Parses `HH:MM` (or `MM` for hourly schedules) into this schedule's kind.
    pub fn with_time(self, text: &str) -> Option<Self> {
        let text = text.trim();
        match self {
            Self::Manual => Some(Self::Manual),
            Self::Hourly { .. } => {
                let minute = text.trim_start_matches(':').parse::<u8>().ok()?;
                (minute < 60).then_some(Self::Hourly { minute })
            }
            Self::Daily { .. } | Self::Weekdays { .. } => {
                let (hour, minute) = text.split_once(':')?;
                let hour = hour.trim().parse::<u8>().ok()?;
                let minute = minute.trim().parse::<u8>().ok()?;
                if hour >= 24 || minute >= 60 {
                    return None;
                }
                Some(match self {
                    Self::Daily { .. } => Self::Daily { hour, minute },
                    _ => Self::Weekdays { hour, minute },
                })
            }
        }
    }

    pub fn time_text(self) -> String {
        match self {
            Self::Manual => String::new(),
            Self::Hourly { minute } => format!("{minute:02}"),
            Self::Daily { hour, minute } | Self::Weekdays { hour, minute } => {
                format!("{hour:02}:{minute:02}")
            }
        }
    }

    /// Cycles through the kinds, keeping the chosen time where it applies.
    pub fn next_kind(self) -> Self {
        let (hour, minute) = match self {
            Self::Manual => (9, 0),
            Self::Hourly { minute } => (9, minute),
            Self::Daily { hour, minute } | Self::Weekdays { hour, minute } => (hour, minute),
        };
        match self {
            Self::Manual => Self::Daily { hour, minute },
            Self::Daily { .. } => Self::Weekdays { hour, minute },
            Self::Weekdays { .. } => Self::Hourly { minute },
            Self::Hourly { .. } => Self::Manual,
        }
    }
}

/// Wall-clock minute in the user's time zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalMinute {
    /// Minutes since the Unix epoch, shifted by the local UTC offset.
    pub index: i64,
    /// 0 = Sunday … 6 = Saturday.
    pub weekday: u8,
}

impl LocalMinute {
    fn day_start(self) -> i64 {
        self.index - self.index.rem_euclid(24 * 60)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Automation {
    pub id: Uuid,
    pub name: String,
    pub command: String,
    /// Runs in this project's folder; `None` follows the selected project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<Uuid>,
    pub schedule: AutomationSchedule,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<u64>,
    /// Local minute index of the last scheduled occurrence that ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_scheduled_slot: Option<i64>,
}

const fn default_true() -> bool {
    true
}

impl Automation {
    /// The occurrence to run now, if one is due and has not run yet.
    pub fn due_slot(&self, now: LocalMinute) -> Option<i64> {
        if !self.enabled {
            return None;
        }
        let slot = self.schedule.latest_occurrence(now)?;
        let fresh = now.index - slot <= AUTOMATION_GRACE_MINUTES;
        let pending = self.last_scheduled_slot.is_none_or(|last| last < slot);
        (fresh && pending).then_some(slot)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutomationValidationError {
    MissingName,
    MissingCommand,
    InvalidTime,
    TooLong,
}

impl AutomationValidationError {
    pub fn message(&self) -> &'static str {
        match self {
            Self::MissingName => "Ponle un nombre a la automatización.",
            Self::MissingCommand => "Escribe el comando que se ejecutará en la terminal.",
            Self::InvalidTime => "La hora no es válida. Usa HH:MM (o MM si es cada hora).",
            Self::TooLong => "El nombre o el comando son demasiado largos.",
        }
    }
}

impl Library {
    pub fn normalize(&mut self) {
        self.schema_version = CURRENT_LIBRARY_SCHEMA_VERSION;
        self.notes.retain(|note| !note.is_blank());
    }

    pub fn note(&self, id: Uuid) -> Option<&Note> {
        self.notes.iter().find(|note| note.id == id)
    }

    pub fn note_mut(&mut self, id: Uuid) -> Option<&mut Note> {
        self.notes.iter_mut().find(|note| note.id == id)
    }

    pub fn create_note(&mut self, project_id: Option<Uuid>, now: u64) -> Uuid {
        let id = Uuid::new_v4();
        self.notes.push(Note {
            id,
            project_id,
            body: String::new(),
            created_at: now,
            updated_at: now,
        });
        id
    }

    /// Stores the edited body; `false` when nothing changed or it is too long.
    pub fn set_note_body(&mut self, id: Uuid, body: String, now: u64) -> bool {
        if body.chars().count() > MAX_NOTE_CHARS {
            return false;
        }
        let Some(note) = self.note_mut(id) else {
            return false;
        };
        if note.body == body {
            return false;
        }
        note.body = body;
        note.updated_at = now;
        true
    }

    pub fn delete_note(&mut self, id: Uuid) -> bool {
        let before = self.notes.len();
        self.notes.retain(|note| note.id != id);
        self.notes.len() != before
    }

    /// Drops a note the user left empty; returns whether it existed.
    pub fn discard_blank_note(&mut self, id: Uuid) -> bool {
        if self.note(id).is_some_and(Note::is_blank) {
            return self.delete_note(id);
        }
        false
    }

    /// Most recently edited first.
    pub fn notes_by_recency(&self) -> Vec<&Note> {
        let mut notes: Vec<_> = self.notes.iter().collect();
        notes.sort_by_key(|note| std::cmp::Reverse(note.updated_at));
        notes
    }

    pub fn automation(&self, id: Uuid) -> Option<&Automation> {
        self.automations
            .iter()
            .find(|automation| automation.id == id)
    }

    /// Adds or replaces an automation after validating the user's input.
    pub fn save_automation(
        &mut self,
        id: Option<Uuid>,
        name: &str,
        command: &str,
        project_id: Option<Uuid>,
        schedule: AutomationSchedule,
        time: &str,
    ) -> Result<Uuid, AutomationValidationError> {
        let name = name.trim();
        let command = command.trim();
        if name.is_empty() {
            return Err(AutomationValidationError::MissingName);
        }
        if command.is_empty() {
            return Err(AutomationValidationError::MissingCommand);
        }
        if name.chars().count() > MAX_AUTOMATION_NAME_CHARS
            || command.chars().count() > MAX_AUTOMATION_COMMAND_CHARS
        {
            return Err(AutomationValidationError::TooLong);
        }
        let schedule = schedule
            .with_time(time)
            .ok_or(AutomationValidationError::InvalidTime)?;
        if let Some(existing) = id.and_then(|id| {
            self.automations
                .iter_mut()
                .find(|automation| automation.id == id)
        }) {
            let schedule_changed = existing.schedule != schedule;
            existing.name = name.to_owned();
            existing.command = command.to_owned();
            existing.project_id = project_id;
            existing.schedule = schedule;
            if schedule_changed {
                existing.last_scheduled_slot = None;
            }
            return Ok(existing.id);
        }
        let id = Uuid::new_v4();
        self.automations.push(Automation {
            id,
            name: name.to_owned(),
            command: command.to_owned(),
            project_id,
            schedule,
            enabled: true,
            last_run_at: None,
            last_scheduled_slot: None,
        });
        Ok(id)
    }

    pub fn delete_automation(&mut self, id: Uuid) -> bool {
        let before = self.automations.len();
        self.automations.retain(|automation| automation.id != id);
        self.automations.len() != before
    }

    pub fn toggle_automation(&mut self, id: Uuid) -> bool {
        let Some(automation) = self.automations.iter_mut().find(|item| item.id == id) else {
            return false;
        };
        automation.enabled = !automation.enabled;
        true
    }

    pub fn record_automation_run(&mut self, id: Uuid, now: u64, slot: Option<i64>) -> bool {
        let Some(automation) = self.automations.iter_mut().find(|item| item.id == id) else {
            return false;
        };
        automation.last_run_at = Some(now);
        if slot.is_some() {
            automation.last_scheduled_slot = slot;
        }
        true
    }

    /// Automations due now, paired with the occurrence they cover.
    pub fn due_automations(&self, now: LocalMinute) -> Vec<(Uuid, i64)> {
        self.automations
            .iter()
            .filter_map(|automation| automation.due_slot(now).map(|slot| (automation.id, slot)))
            .collect()
    }

    /// Forgets a removed project without losing the user's material.
    pub fn detach_project(&mut self, project_id: Uuid) -> bool {
        let mut changed = false;
        for note in &mut self.notes {
            if note.project_id == Some(project_id) {
                note.project_id = None;
                changed = true;
            }
        }
        for automation in &mut self.automations {
            if automation.project_id == Some(project_id) {
                automation.project_id = None;
                // Its command was configured for the removed project. Keep it
                // available, but don't schedule it in whichever project is next.
                automation.enabled = false;
                changed = true;
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONDAY_0930: LocalMinute = LocalMinute {
        // 1970-01-05 was a Monday.
        index: 4 * 1440 + 9 * 60 + 30,
        weekday: 1,
    };

    fn minute(offset: i64, weekday: u8) -> LocalMinute {
        LocalMinute {
            index: MONDAY_0930.index + offset,
            weekday,
        }
    }

    #[test]
    fn note_titles_skip_blank_lines_and_heading_marks() {
        let note = Note {
            id: Uuid::new_v4(),
            project_id: None,
            body: "\n  ## Plan de release \nRevisar changelog\n".into(),
            created_at: 0,
            updated_at: 0,
        };
        assert_eq!(note.title(), "Plan de release");
        assert_eq!(note.preview(), "Revisar changelog");
        let empty = Note {
            body: "   \n".into(),
            ..note
        };
        assert_eq!(empty.title(), "Nota sin título");
        assert!(empty.is_blank());
    }

    #[test]
    fn blank_notes_are_discarded_and_edits_bump_recency() {
        let mut library = Library::default();
        let first = library.create_note(None, 10);
        let second = library.create_note(None, 20);
        assert!(library.set_note_body(first, "Primera".into(), 30));
        assert!(!library.set_note_body(first, "Primera".into(), 40));
        assert_eq!(library.notes_by_recency()[0].id, first);
        assert!(library.discard_blank_note(second));
        assert!(!library.discard_blank_note(first));
        assert_eq!(library.notes.len(), 1);
    }

    #[test]
    fn schedules_parse_their_time_and_reject_out_of_range_values() {
        let daily = AutomationSchedule::Daily { hour: 0, minute: 0 };
        assert_eq!(
            daily.with_time(" 7:05 "),
            Some(AutomationSchedule::Daily { hour: 7, minute: 5 })
        );
        assert_eq!(daily.with_time("24:00"), None);
        assert_eq!(daily.with_time("9"), None);
        let hourly = AutomationSchedule::Hourly { minute: 0 };
        assert_eq!(
            hourly.with_time("45"),
            Some(AutomationSchedule::Hourly { minute: 45 })
        );
        assert_eq!(hourly.with_time("60"), None);
        assert_eq!(
            AutomationSchedule::Manual.with_time("garbage"),
            Some(AutomationSchedule::Manual)
        );
        assert_eq!(
            AutomationSchedule::Hourly { minute: 15 }.next_kind(),
            AutomationSchedule::Manual
        );
        assert_eq!(
            AutomationSchedule::Manual.next_kind(),
            AutomationSchedule::Daily { hour: 9, minute: 0 }
        );
    }

    #[test]
    fn daily_runs_once_inside_the_grace_window() {
        let mut automation = Automation {
            id: Uuid::new_v4(),
            name: "Resumen".into(),
            command: "claude -p 'resume'".into(),
            project_id: None,
            schedule: AutomationSchedule::Daily {
                hour: 9,
                minute: 30,
            },
            enabled: true,
            last_run_at: None,
            last_scheduled_slot: None,
        };
        assert_eq!(automation.due_slot(minute(-1, 1)), None);
        let slot = automation.due_slot(MONDAY_0930).unwrap();
        assert_eq!(slot, MONDAY_0930.index);
        assert_eq!(
            automation.due_slot(minute(AUTOMATION_GRACE_MINUTES, 1)),
            Some(slot)
        );
        assert_eq!(
            automation.due_slot(minute(AUTOMATION_GRACE_MINUTES + 1, 1)),
            None
        );
        automation.last_scheduled_slot = Some(slot);
        assert_eq!(automation.due_slot(minute(2, 1)), None);
        // The next day is a new occurrence.
        assert_eq!(automation.due_slot(minute(1440, 2)), Some(slot + 1440));
        automation.enabled = false;
        assert_eq!(automation.due_slot(minute(1440, 2)), None);
    }

    #[test]
    fn weekday_and_hourly_schedules_pick_their_own_occurrences() {
        let weekdays = AutomationSchedule::Weekdays {
            hour: 9,
            minute: 30,
        };
        assert!(weekdays.latest_occurrence(MONDAY_0930).is_some());
        assert_eq!(weekdays.latest_occurrence(minute(5 * 1440, 6)), None);
        let hourly = AutomationSchedule::Hourly { minute: 15 };
        assert_eq!(
            hourly.latest_occurrence(MONDAY_0930),
            Some(MONDAY_0930.index - 15)
        );
        assert_eq!(
            AutomationSchedule::Hourly { minute: 45 }.latest_occurrence(MONDAY_0930),
            None
        );
    }

    #[test]
    fn saving_validates_and_rescheduling_resets_the_last_slot() {
        let mut library = Library::default();
        assert_eq!(
            library.save_automation(None, " ", "ls", None, AutomationSchedule::Manual, ""),
            Err(AutomationValidationError::MissingName)
        );
        assert_eq!(
            library.save_automation(
                None,
                "Tests",
                "cargo test",
                None,
                AutomationSchedule::Daily { hour: 0, minute: 0 },
                "25:00"
            ),
            Err(AutomationValidationError::InvalidTime)
        );
        let id = library
            .save_automation(
                None,
                " Tests ",
                " cargo test ",
                None,
                AutomationSchedule::Daily { hour: 0, minute: 0 },
                "08:00",
            )
            .unwrap();
        let automation = library.automation(id).unwrap();
        assert_eq!(automation.name, "Tests");
        assert_eq!(automation.command, "cargo test");
        assert!(library.record_automation_run(id, 100, Some(42)));
        library
            .save_automation(
                Some(id),
                "Tests",
                "cargo test",
                None,
                AutomationSchedule::Daily { hour: 0, minute: 0 },
                "08:00",
            )
            .unwrap();
        assert_eq!(
            library.automation(id).unwrap().last_scheduled_slot,
            Some(42)
        );
        library
            .save_automation(
                Some(id),
                "Tests",
                "cargo test",
                None,
                AutomationSchedule::Daily { hour: 0, minute: 0 },
                "08:30",
            )
            .unwrap();
        assert_eq!(library.automation(id).unwrap().last_scheduled_slot, None);
        assert_eq!(library.automation(id).unwrap().last_run_at, Some(100));
    }

    #[test]
    fn removing_a_project_keeps_its_notes_and_automations() {
        let mut library = Library::default();
        let project = Uuid::new_v4();
        let note = library.create_note(Some(project), 1);
        library.set_note_body(note, "Idea".into(), 2);
        let automation = library
            .save_automation(
                None,
                "Build",
                "cargo build",
                Some(project),
                AutomationSchedule::Hourly { minute: 30 },
                "30",
            )
            .unwrap();
        assert!(!library.due_automations(MONDAY_0930).is_empty());
        assert!(library.detach_project(project));
        assert_eq!(library.note(note).unwrap().project_id, None);
        assert_eq!(library.automation(automation).unwrap().project_id, None);
        assert!(!library.automation(automation).unwrap().enabled);
        assert!(library.due_automations(MONDAY_0930).is_empty());
        assert!(!library.detach_project(project));
    }
}
