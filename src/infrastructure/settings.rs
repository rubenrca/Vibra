use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::domain::appearance::AppearanceMode;
use crate::infrastructure::paths::{
    RevisionGuard, application_support_directory, atomic_write_if_missing,
    gpui_preview_support_directory, read_file_limited,
};

const SETTINGS_FILE_NAME: &str = "settings.json";
const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;

pub const CURRENT_SETTINGS_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    #[serde(default)]
    pub inbox: crate::domain::work_items::InboxPreferences,
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default = "default_terminal_font_size")]
    pub terminal_font_size: f32,
    #[serde(default)]
    pub show_hidden_files: bool,
    #[serde(default = "default_true")]
    pub left_sidebar_visible: bool,
    #[serde(default = "default_true", alias = "gitPanelVisible")]
    pub right_sidebar_visible: bool,
    #[serde(default = "default_left_sidebar_width")]
    pub left_sidebar_width: f32,
    #[serde(default = "default_right_sidebar_width")]
    pub right_sidebar_width: f32,
    /// Projects kept in the global sidebar’s Pinned section.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pinned_project_ids: Vec<uuid::Uuid>,
    /// Palette id (`midnight`, `nord`, or `user:stem` from ~/.vibra/themes).
    #[serde(default = "default_theme_id")]
    pub theme_id: String,
    /// `light`, `dark`, or `system`.
    #[serde(default)]
    pub appearance_mode: AppearanceMode,
    /// Notify when an agent finishes or needs attention off-screen.
    #[serde(default = "default_true")]
    pub agent_notifications: bool,
    #[serde(default = "default_window_width")]
    pub window_width: f32,
    #[serde(default = "default_window_height")]
    pub window_height: f32,
    /// Review diffs side by side instead of unified.
    #[serde(default)]
    pub diff_split: bool,
    /// Wrap long diff lines instead of scrolling them horizontally.
    #[serde(default)]
    pub diff_wrap: bool,
    /// Code size in the Git review, independent of the terminal.
    #[serde(default = "default_diff_font_size")]
    pub diff_font_size: f32,
    /// Share of the center given to the terminal when a review sits beside it.
    #[serde(default = "default_review_split")]
    pub review_split: f32,
}

pub const MIN_REVIEW_SPLIT: f32 = 0.2;
pub const MAX_REVIEW_SPLIT: f32 = 0.8;

const fn default_review_split() -> f32 {
    0.5
}

pub const MIN_TERMINAL_FONT_SIZE: f32 = 8.0;
pub const MAX_TERMINAL_FONT_SIZE: f32 = 32.0;

pub const fn default_terminal_font_size() -> f32 {
    12.0
}

pub const DEFAULT_DIFF_FONT_SIZE: f32 = 12.0;
pub const MIN_DIFF_FONT_SIZE: f32 = 9.0;
pub const MAX_DIFF_FONT_SIZE: f32 = 24.0;

const fn default_diff_font_size() -> f32 {
    DEFAULT_DIFF_FONT_SIZE
}

const fn default_true() -> bool {
    true
}

pub const DEFAULT_LEFT_SIDEBAR_WIDTH: f32 = 220.0;
pub const DEFAULT_RIGHT_SIDEBAR_WIDTH: f32 = 320.0;
pub const DEFAULT_WINDOW_WIDTH: f32 = 1240.0;
pub const DEFAULT_WINDOW_HEIGHT: f32 = 780.0;
pub const MIN_LEFT_SIDEBAR_WIDTH: f32 = 188.0;
pub const MAX_LEFT_SIDEBAR_WIDTH: f32 = 420.0;
pub const MIN_RIGHT_SIDEBAR_WIDTH: f32 = 280.0;
pub const MAX_RIGHT_SIDEBAR_WIDTH: f32 = 720.0;
pub const MIN_WINDOW_WIDTH: f32 = 900.0;
pub const MIN_WINDOW_HEIGHT: f32 = 580.0;
const MAX_WINDOW_DIMENSION: f32 = 10_000.0;

const fn default_left_sidebar_width() -> f32 {
    DEFAULT_LEFT_SIDEBAR_WIDTH
}

const fn default_right_sidebar_width() -> f32 {
    DEFAULT_RIGHT_SIDEBAR_WIDTH
}

const fn default_window_width() -> f32 {
    DEFAULT_WINDOW_WIDTH
}

const fn default_window_height() -> f32 {
    DEFAULT_WINDOW_HEIGHT
}

fn default_theme_id() -> String {
    "midnight".to_string()
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            inbox: Default::default(),
            schema_version: CURRENT_SETTINGS_SCHEMA_VERSION,
            terminal_font_size: default_terminal_font_size(),
            show_hidden_files: false,
            left_sidebar_visible: true,
            right_sidebar_visible: true,
            left_sidebar_width: DEFAULT_LEFT_SIDEBAR_WIDTH,
            right_sidebar_width: DEFAULT_RIGHT_SIDEBAR_WIDTH,
            pinned_project_ids: Vec::new(),
            theme_id: default_theme_id(),
            appearance_mode: AppearanceMode::System,
            agent_notifications: true,
            window_width: DEFAULT_WINDOW_WIDTH,
            window_height: DEFAULT_WINDOW_HEIGHT,
            diff_split: false,
            diff_wrap: false,
            diff_font_size: DEFAULT_DIFF_FONT_SIZE,
            review_split: default_review_split(),
        }
    }
}

fn clamp_or(value: f32, default: f32, min: f32, max: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        default
    }
}

impl AppSettings {
    fn normalize(&mut self) {
        self.terminal_font_size = clamp_or(
            self.terminal_font_size,
            default_terminal_font_size(),
            MIN_TERMINAL_FONT_SIZE,
            MAX_TERMINAL_FONT_SIZE,
        );
        self.diff_font_size = clamp_or(
            self.diff_font_size,
            DEFAULT_DIFF_FONT_SIZE,
            MIN_DIFF_FONT_SIZE,
            MAX_DIFF_FONT_SIZE,
        );
        self.left_sidebar_width = clamp_or(
            self.left_sidebar_width,
            DEFAULT_LEFT_SIDEBAR_WIDTH,
            MIN_LEFT_SIDEBAR_WIDTH,
            MAX_LEFT_SIDEBAR_WIDTH,
        );
        self.right_sidebar_width = clamp_or(
            self.right_sidebar_width,
            DEFAULT_RIGHT_SIDEBAR_WIDTH,
            MIN_RIGHT_SIDEBAR_WIDTH,
            MAX_RIGHT_SIDEBAR_WIDTH,
        );
        self.window_width = clamp_or(
            self.window_width,
            DEFAULT_WINDOW_WIDTH,
            MIN_WINDOW_WIDTH,
            MAX_WINDOW_DIMENSION,
        );
        self.window_height = clamp_or(
            self.window_height,
            DEFAULT_WINDOW_HEIGHT,
            MIN_WINDOW_HEIGHT,
            MAX_WINDOW_DIMENSION,
        );
        self.review_split = clamp_or(
            self.review_split,
            default_review_split(),
            MIN_REVIEW_SPLIT,
            MAX_REVIEW_SPLIT,
        );
        if self.theme_id.trim().is_empty() {
            self.theme_id = default_theme_id();
        }
        self.schema_version = CURRENT_SETTINGS_SCHEMA_VERSION;
    }

    pub fn set_window_size(&mut self, width: f32, height: f32) -> bool {
        if !width.is_finite() || !height.is_finite() {
            return false;
        }
        let width = clamp_or(
            width,
            DEFAULT_WINDOW_WIDTH,
            MIN_WINDOW_WIDTH,
            MAX_WINDOW_DIMENSION,
        );
        let height = clamp_or(
            height,
            DEFAULT_WINDOW_HEIGHT,
            MIN_WINDOW_HEIGHT,
            MAX_WINDOW_DIMENSION,
        );
        if (self.window_width - width).abs() < 0.5 && (self.window_height - height).abs() < 0.5 {
            return false;
        }
        self.window_width = width;
        self.window_height = height;
        true
    }
}

#[derive(Debug, Clone)]
pub struct SettingsRepository {
    path: PathBuf,
    preview_path: Option<PathBuf>,
    revision: Arc<RevisionGuard>,
}

impl SettingsRepository {
    pub fn for_current_user() -> Option<Self> {
        let support_directory = application_support_directory()?;
        Some(Self {
            path: support_directory.join(SETTINGS_FILE_NAME),
            preview_path: gpui_preview_support_directory()
                .map(|directory| directory.join(SETTINGS_FILE_NAME)),
            revision: Arc::new(RevisionGuard::default()),
        })
    }

    #[cfg(test)]
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            preview_path: None,
            revision: Arc::new(RevisionGuard::default()),
        }
    }

    #[cfg(test)]
    fn with_preview(path: impl Into<PathBuf>, preview_path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            preview_path: Some(preview_path.into()),
            revision: Arc::new(RevisionGuard::default()),
        }
    }

    /// Folder holding `settings.json`, shared with the other app-owned files.
    pub fn directory(&self) -> Option<&std::path::Path> {
        self.path.parent()
    }

    pub fn load(&self) -> Result<AppSettings> {
        let result = self.load_inner();
        if let Err(error) = &result {
            self.revision.blocked(error);
        }
        result
    }

    fn load_inner(&self) -> Result<AppSettings> {
        self.import_preview_settings()?;
        if !self.path.exists() {
            self.revision.loaded(None);
            return Ok(AppSettings::default());
        }
        let bytes = read_settings_file(&self.path)?;
        let settings = self.decode(&bytes)?;
        self.revision.loaded(Some(bytes));
        Ok(settings)
    }

    fn decode(&self, bytes: &[u8]) -> Result<AppSettings> {
        let mut settings: AppSettings = serde_json::from_slice(bytes)
            .with_context(|| format!("Invalid settings in {}", self.path.display()))?;
        if settings.schema_version > CURRENT_SETTINGS_SCHEMA_VERSION {
            bail!(
                "{} uses settings schema {} but this version supports up to {}",
                self.path.display(),
                settings.schema_version,
                CURRENT_SETTINGS_SCHEMA_VERSION
            );
        }
        settings.normalize();
        Ok(settings)
    }

    pub fn save(&self, settings: &AppSettings) -> Result<()> {
        self.import_preview_settings()?;
        let data = serde_json::to_vec(settings)?;
        if data.len() as u64 > MAX_SETTINGS_BYTES {
            bail!("settings exceed the 1 MiB limit and cannot be saved");
        }
        self.revision
            .save_merging(&self.path, &data, MAX_SETTINGS_BYTES, |base, current| {
                let base = base
                    .map(|bytes| self.decode(bytes))
                    .transpose()?
                    .unwrap_or_default();
                let mut current = if let Some(bytes) = current {
                    // Validate before merging, retaining unknown fields in supported schemas.
                    self.decode(bytes)?;
                    serde_json::from_slice(bytes)?
                } else {
                    serde_json::to_value(AppSettings::default())?
                };
                if let Some(object) = current.as_object_mut()
                    && let Some(value) = object.remove("gitPanelVisible")
                {
                    object.insert("rightSidebarVisible".into(), value);
                }
                merge_changed_settings(
                    &serde_json::to_value(base)?,
                    &serde_json::to_value(settings)?,
                    &mut current,
                );
                current["schemaVersion"] = CURRENT_SETTINGS_SCHEMA_VERSION.into();
                Ok(serde_json::to_vec(&current)?)
            })?;
        Ok(())
    }

    fn import_preview_settings(&self) -> Result<()> {
        if self.path.exists() {
            return Ok(());
        }
        let Some(preview_path) = self
            .preview_path
            .as_ref()
            .filter(|preview_path| preview_path.exists())
        else {
            return Ok(());
        };
        let data = read_settings_file(preview_path)?;
        let preview: AppSettings = serde_json::from_slice(&data)
            .with_context(|| format!("Invalid JSON in {}", preview_path.display()))?;
        if preview.schema_version > CURRENT_SETTINGS_SCHEMA_VERSION {
            bail!("{} uses a newer schema", preview_path.display());
        }
        atomic_write_if_missing(&self.path, &data).with_context(|| {
            format!(
                "could not import {} to {}",
                preview_path.display(),
                self.path.display()
            )
        })?;
        Ok(())
    }
}

/// Apply this instance's edits; unchanged values and unknown fields stay on disk.
/// A preference explicitly changed in both instances uses the latest save.
fn merge_changed_settings(
    base: &serde_json::Value,
    local: &serde_json::Value,
    current: &mut serde_json::Value,
) {
    if base == local {
        return;
    }
    if let (Some(base), Some(local), Some(current)) =
        (base.as_object(), local.as_object(), current.as_object_mut())
    {
        for (key, value) in local {
            if base.get(key) == Some(value) {
                continue;
            }
            if let (Some(base), Some(current)) = (base.get(key), current.get_mut(key)) {
                merge_changed_settings(base, value, current);
            } else {
                current.insert(key.clone(), value.clone());
            }
        }
        for key in base.keys().filter(|key| !local.contains_key(*key)) {
            current.remove(key);
        }
    } else {
        *current = local.clone();
    }
}

fn read_settings_file(path: &std::path::Path) -> Result<Vec<u8>> {
    read_file_limited(path, MAX_SETTINGS_BYTES, "1 MiB")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use uuid::Uuid;

    #[test]
    fn settings_round_trip_and_normalize_legacy_values() {
        let root = std::env::temp_dir().join(format!("vibra-settings-{}", Uuid::new_v4()));
        let repository = SettingsRepository::at(root.join("settings.json"));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("settings.json"),
            br#"{"terminalFontSize":100,"showHiddenFiles":true}"#,
        )
        .unwrap();
        let mut settings = repository.load().unwrap();
        assert_eq!(settings.schema_version, CURRENT_SETTINGS_SCHEMA_VERSION);
        assert_eq!(settings.terminal_font_size, 32.0);
        assert_eq!(settings.window_width, DEFAULT_WINDOW_WIDTH);
        assert_eq!(settings.window_height, DEFAULT_WINDOW_HEIGHT);
        assert!(settings.pinned_project_ids.is_empty());
        settings.pinned_project_ids.push(Uuid::new_v4());
        assert!(settings.set_window_size(1512.0, 864.0));
        repository.save(&settings).unwrap();
        assert_eq!(repository.load().unwrap(), settings);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn settings_load_preserves_theme_id_without_ui_theme_registry() {
        let root = std::env::temp_dir().join(format!("vibra-settings-{}", Uuid::new_v4()));
        let repository = SettingsRepository::at(root.join("settings.json"));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("settings.json"),
            br#"{"themeId":"user:temporarily-missing","appearanceMode":"dark"}"#,
        )
        .unwrap();

        let settings = repository.load().unwrap();
        assert_eq!(settings.theme_id, "user:temporarily-missing");
        assert_eq!(settings.appearance_mode, AppearanceMode::Dark);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn settings_import_from_the_gpui_preview_once() {
        let root = std::env::temp_dir().join(format!("vibra-settings-import-{}", Uuid::new_v4()));
        let canonical = root.join("Vibra/settings.json");
        let preview = root.join("VibraGPUI/settings.json");
        fs::create_dir_all(preview.parent().unwrap()).unwrap();
        fs::write(
            &preview,
            concat!(
                r#"{"schemaVersion":1,"terminalFontSize":14,"showHiddenFiles":true,"#,
                r#""leftSidebarVisible":false,"gitPanelVisible":true}"#
            )
            .as_bytes(),
        )
        .unwrap();
        let repository = SettingsRepository::with_preview(&canonical, &preview);

        let settings = repository.load().unwrap();

        assert_eq!(settings.terminal_font_size, 14.0);
        assert!(settings.show_hidden_files);
        assert!(!settings.left_sidebar_visible);
        assert!(settings.right_sidebar_visible);
        assert!((settings.left_sidebar_width - DEFAULT_LEFT_SIDEBAR_WIDTH).abs() < f32::EPSILON);
        assert!((settings.right_sidebar_width - DEFAULT_RIGHT_SIDEBAR_WIDTH).abs() < f32::EPSILON);
        assert_eq!(settings.window_width, DEFAULT_WINDOW_WIDTH);
        assert_eq!(settings.window_height, DEFAULT_WINDOW_HEIGHT);
        assert!(canonical.exists());
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&canonical).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_load_never_overwrites_future_settings() {
        let root = std::env::temp_dir().join(format!("vibra-settings-{}", Uuid::new_v4()));
        let path = root.join("settings.json");
        fs::create_dir_all(&root).unwrap();
        let original = br#"{"schemaVersion":999,"agentNotifications":false}"#;
        fs::write(&path, original).unwrap();
        let repository = SettingsRepository::at(&path);
        assert!(repository.load().is_err());
        assert!(repository.save(&AppSettings::default()).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_settings_instances_merge_only_their_own_changes() {
        let root = std::env::temp_dir().join(format!("vibra-settings-{}", Uuid::new_v4()));
        let path = root.join("settings.json");
        let first = SettingsRepository::at(&path);
        let second = SettingsRepository::at(&path);
        let mut first_settings = first.load().unwrap();
        let mut second_settings = second.load().unwrap();
        first_settings.show_hidden_files = true;
        second_settings.agent_notifications = false;
        first.save(&first_settings).unwrap();
        second.save(&second_settings).unwrap();
        let reader = SettingsRepository::at(&path);
        let mut expected = first_settings.clone();
        expected.agent_notifications = false;
        assert_eq!(reader.load().unwrap(), expected);

        // Both views still hold their own settings. Subsequent geometry saves
        // must not restore stale values for unrelated preferences.
        second_settings.set_window_size(1500.0, 900.0);
        second.save(&second_settings).unwrap();
        first_settings.diff_wrap = true;
        first.save(&first_settings).unwrap();
        expected.set_window_size(1500.0, 900.0);
        expected.diff_wrap = true;
        assert_eq!(reader.load().unwrap(), expected);
        // A close-time save without new edits leaves the merged state intact.
        second.save(&second_settings).unwrap();
        assert_eq!(reader.load().unwrap(), expected);
        assert!(!fs::read_dir(&root).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("recovery")
        }));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn nested_preferences_and_cleared_lists_survive_other_instances_saves() {
        let root = std::env::temp_dir().join(format!("vibra-settings-{}", Uuid::new_v4()));
        let path = root.join("settings.json");
        SettingsRepository::at(&path)
            .save(&AppSettings {
                pinned_project_ids: vec![Uuid::new_v4()],
                ..AppSettings::default()
            })
            .unwrap();
        let first = SettingsRepository::at(&path);
        let second = SettingsRepository::at(&path);
        let mut first_settings = first.load().unwrap();
        let mut second_settings = second.load().unwrap();
        first_settings.pinned_project_ids.clear();
        first_settings.inbox.assigned_to_me = true;
        first_settings.inbox.seen.insert("first-item".into(), 10);
        second_settings.inbox.list_width = 350.0;
        second_settings.inbox.seen.insert("second-item".into(), 20);
        first.save(&first_settings).unwrap();
        second.save(&second_settings).unwrap();
        second_settings.diff_wrap = true;
        second.save(&second_settings).unwrap();
        let saved = SettingsRepository::at(&path).load().unwrap();
        assert!(saved.pinned_project_ids.is_empty());
        assert!(saved.inbox.assigned_to_me);
        assert_eq!(saved.inbox.list_width, 350.0);
        assert_eq!(saved.inbox.seen.get("first-item"), Some(&10));
        assert_eq!(saved.inbox.seen.get("second-item"), Some(&20));
        assert!(saved.diff_wrap);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn simultaneous_settings_writes_preserve_both_changes() {
        let root = std::env::temp_dir().join(format!("vibra-settings-{}", Uuid::new_v4()));
        let path = root.join("settings.json");
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let writers: Vec<_> = (0..2)
            .map(|index| {
                let repository = SettingsRepository::at(&path);
                let mut settings = repository.load().unwrap();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    if index == 0 {
                        settings.show_hidden_files = true;
                    } else {
                        settings.agent_notifications = false;
                    }
                    barrier.wait();
                    repository.save(&settings).unwrap();
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        let saved = SettingsRepository::at(&path).load().unwrap();
        assert!(saved.show_hidden_files);
        assert!(!saved.agent_notifications);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_window_resizes_use_the_latest_changed_dimensions() {
        let root = std::env::temp_dir().join(format!("vibra-settings-{}", Uuid::new_v4()));
        let path = root.join("settings.json");
        let first = SettingsRepository::at(&path);
        let second = SettingsRepository::at(&path);
        let mut first_settings = first.load().unwrap();
        let mut second_settings = second.load().unwrap();
        first_settings.set_window_size(1500.0, 900.0);
        second_settings.set_window_size(1600.0, 950.0);
        first.save(&first_settings).unwrap();
        second.save(&second_settings).unwrap();
        // Saving unchanged local geometry must not win over a newer resize.
        first.save(&first_settings).unwrap();
        assert_eq!(
            SettingsRepository::at(&path).load().unwrap(),
            second_settings
        );
        first_settings.set_window_size(1700.0, 1000.0);
        first.save(&first_settings).unwrap();
        assert_eq!(
            SettingsRepository::at(&path).load().unwrap(),
            first_settings
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn settings_merge_preserves_unknown_fields_and_migrates_aliases() {
        let root = std::env::temp_dir().join(format!("vibra-settings-{}", Uuid::new_v4()));
        let path = root.join("settings.json");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            &path,
            br#"{"gitPanelVisible":true,"extraPreference":"keep"}"#,
        )
        .unwrap();
        let repository = SettingsRepository::at(&path);
        let mut settings = repository.load().unwrap();
        settings.right_sidebar_visible = false;
        repository.save(&settings).unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["extraPreference"], "keep");
        assert!(saved.get("gitPanelVisible").is_none());
        assert!(!repository.load().unwrap().right_sidebar_visible);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn external_invalid_or_future_settings_keep_the_file_and_a_local_recovery() {
        for external in [
            b"invalid json".as_slice(),
            br#"{"schemaVersion":999,"agentNotifications":false}"#,
            br#"{"windowWidth":"invalid"}"#,
        ] {
            let root = std::env::temp_dir().join(format!("vibra-settings-{}", Uuid::new_v4()));
            let path = root.join("settings.json");
            let repository = SettingsRepository::at(&path);
            let mut settings = repository.load().unwrap();
            repository.save(&settings).unwrap();
            fs::write(&path, external).unwrap();
            settings.show_hidden_files = true;
            assert!(repository.save(&settings).is_err());
            assert_eq!(fs::read(&path).unwrap(), external);
            let recovery = fs::read_dir(&root)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| {
                    path.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .contains("recovery")
                })
                .unwrap();
            assert_eq!(
                serde_json::from_slice::<AppSettings>(&fs::read(recovery).unwrap()).unwrap(),
                settings
            );
            // Repairing the file lets the next save succeed without a restart.
            fs::write(&path, b"{}").unwrap();
            repository.save(&settings).unwrap();
            assert!(repository.load().unwrap().show_hidden_files);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn window_size_is_clamped_and_rejects_invalid_values() {
        let mut settings = AppSettings::default();
        assert!(settings.set_window_size(1600.0, 920.0));
        assert_eq!(settings.window_width, 1600.0);
        assert_eq!(settings.window_height, 920.0);
        assert!(!settings.set_window_size(f32::NAN, 700.0));
        assert!(settings.set_window_size(100.0, 100.0));
        assert_eq!(settings.window_width, MIN_WINDOW_WIDTH);
        assert_eq!(settings.window_height, MIN_WINDOW_HEIGHT);
    }
}
