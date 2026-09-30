//! `library.json`: notes and automations, next to `settings.json`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

use crate::domain::library::{CURRENT_LIBRARY_SCHEMA_VERSION, Library, LocalMinute};
use crate::infrastructure::paths::{RevisionGuard, read_optional_file_limited};

const LIBRARY_FILE_NAME: &str = "library.json";
const MAX_LIBRARY_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct LibraryRepository {
    path: PathBuf,
    revision: Arc<RevisionGuard>,
}

impl LibraryRepository {
    pub fn in_directory(directory: &Path) -> Self {
        Self::at(directory.join(LIBRARY_FILE_NAME))
    }

    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            revision: Arc::new(RevisionGuard::default()),
        }
    }

    pub fn load(&self) -> Result<Library> {
        let result = self.load_inner();
        if let Err(error) = &result {
            self.revision.blocked(error);
        }
        result
    }

    fn load_inner(&self) -> Result<Library> {
        let Some(bytes) = read_optional_file_limited(&self.path, MAX_LIBRARY_BYTES, "8 MiB")?
        else {
            self.revision.loaded(None);
            return Ok(Library::default());
        };
        let mut library: Library = serde_json::from_slice(&bytes)
            .with_context(|| format!("Invalid JSON in {}", self.path.display()))?;
        if library.schema_version > CURRENT_LIBRARY_SCHEMA_VERSION {
            bail!(
                "{} uses library schema {} but this version supports up to {}",
                self.path.display(),
                library.schema_version,
                CURRENT_LIBRARY_SCHEMA_VERSION
            );
        }
        library.normalize();
        self.revision.loaded(Some(bytes));
        Ok(library)
    }

    pub fn save(&self, library: &Library) -> Result<()> {
        let data = serde_json::to_vec_pretty(&library.normalized_for_storage())?;
        if data.len() as u64 > MAX_LIBRARY_BYTES {
            bail!("notes and automations exceed the 8 MiB limit");
        }
        self.revision.save(&self.path, &data)?;
        Ok(())
    }
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

/// The current wall-clock minute in the user's time zone.
pub fn local_minute_now() -> LocalMinute {
    local_minute_at(unix_now())
}

fn local_minute_at(seconds: u64) -> LocalMinute {
    let time = seconds as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let converted = unsafe { !libc::localtime_r(&time, &mut tm).is_null() };
    let (offset, weekday) = if converted {
        (tm.tm_gmtoff as i64, tm.tm_wday as u8)
    } else {
        (0, ((seconds / 86_400 + 4) % 7) as u8)
    };
    LocalMinute {
        index: (seconds as i64 + offset).div_euclid(60),
        weekday,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::library::AutomationSchedule;
    use std::fs;

    #[test]
    fn library_round_trips_and_drops_blank_notes() {
        let root = std::env::temp_dir().join(format!("vibra-library-{}", uuid::Uuid::new_v4()));
        let repository = LibraryRepository::in_directory(&root);
        assert_eq!(repository.load().unwrap(), Library::default());
        let mut library = Library::default();
        let kept = library.create_note(None, 1);
        library.set_note_body(kept, "Pendiente".into(), 2);
        library.create_note(None, 3);
        library
            .save_automation(
                None,
                "Tests",
                "cargo test",
                None,
                AutomationSchedule::Weekdays { hour: 0, minute: 0 },
                "09:15",
            )
            .unwrap();
        repository.save(&library).unwrap();
        let loaded = LibraryRepository::in_directory(&root).load().unwrap();
        assert_eq!(loaded.notes.len(), 1);
        assert_eq!(loaded.notes[0].id, kept);
        assert_eq!(
            loaded.automations[0].schedule,
            AutomationSchedule::Weekdays {
                hour: 9,
                minute: 15
            }
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn saving_repairs_duplicate_ids_without_changing_the_live_library() {
        let root = std::env::temp_dir().join(format!("vibra-library-{}", uuid::Uuid::new_v4()));
        let repository = LibraryRepository::in_directory(&root);
        let mut library = Library::default();
        let first = library.create_note(Some(uuid::Uuid::new_v4()), 12);
        library.set_note_body(first, "First note\nwith more text".into(), 34);
        library.notes.push(crate::domain::library::Note {
            body: "Second note".into(),
            ..library.notes[0].clone()
        });
        library.create_note(None, 56);
        library
            .save_automation(
                None,
                "Check",
                "cargo check",
                None,
                AutomationSchedule::Manual,
                "",
            )
            .unwrap();
        library.automations.push(library.automations[0].clone());
        let original = library.clone();
        repository.save(&library).unwrap();
        assert_eq!(library, original);
        let loaded = LibraryRepository::in_directory(&root).load().unwrap();
        assert_eq!(loaded.notes.len(), 2);
        assert_eq!(loaded.notes[0], original.notes[0]);
        assert_eq!(loaded.notes[1].body, original.notes[1].body);
        assert_ne!(loaded.notes[1].id, first);
        assert_eq!(loaded.automations[0], original.automations[0]);
        assert_ne!(loaded.automations[1].id, loaded.automations[0].id);
        assert_eq!(
            loaded.automations[1].command,
            original.automations[1].command
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn future_schemas_are_not_overwritten() {
        let root = std::env::temp_dir().join(format!("vibra-library-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(LIBRARY_FILE_NAME), br#"{"schemaVersion": 99}"#).unwrap();
        let repository = LibraryRepository::in_directory(&root);
        assert!(repository.load().is_err());
        assert!(repository.save(&Library::default()).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_minutes_follow_the_epoch() {
        let minute = local_minute_at(0);
        assert!(minute.weekday <= 6);
        assert_eq!(local_minute_at(3_600).index - minute.index, 60);
    }
}
