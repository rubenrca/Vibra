use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::infrastructure::paths::{application_support_directory, gpui_preview_support_directory};

const SETTINGS_FILE_NAME: &str = "settings.json";
const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;
const KNOWN_THEME_IDS: &[&str] = &["midnight", "moss", "harbor", "cinder", "violet", "bloom"];

pub const CURRENT_SETTINGS_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default = "default_terminal_font_size")]
    pub terminal_font_size: f32,
    #[serde(default)]
    pub show_hidden_files: bool,
    #[serde(default = "default_true")]
    pub left_sidebar_visible: bool,
    #[serde(default)]
    pub git_panel_visible: bool,
    /// Built-in palette id (e.g. `midnight`, `moss`).
    #[serde(default = "default_theme_id")]
    pub theme_id: String,
    /// `light`, `dark`, or `system`.
    #[serde(default = "default_appearance_mode")]
    pub appearance_mode: String,
    /// Notify when an agent finishes or needs attention off-screen.
    #[serde(default = "default_true")]
    pub agent_notifications: bool,
}

const fn default_terminal_font_size() -> f32 {
    12.0
}

const fn default_true() -> bool {
    true
}

fn default_theme_id() -> String {
    "midnight".to_string()
}

fn default_appearance_mode() -> String {
    "system".to_string()
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SETTINGS_SCHEMA_VERSION,
            terminal_font_size: default_terminal_font_size(),
            show_hidden_files: false,
            left_sidebar_visible: true,
            git_panel_visible: false,
            theme_id: default_theme_id(),
            appearance_mode: default_appearance_mode(),
            agent_notifications: true,
        }
    }
}

impl AppSettings {
    fn normalize(&mut self) {
        if !self.terminal_font_size.is_finite() {
            self.terminal_font_size = default_terminal_font_size();
        }
        self.terminal_font_size = self.terminal_font_size.clamp(8.0, 32.0);
        if !KNOWN_THEME_IDS.contains(&self.theme_id.as_str()) {
            self.theme_id = default_theme_id();
        }
        self.appearance_mode = match self.appearance_mode.as_str() {
            "light" => "light".to_string(),
            "dark" => "dark".to_string(),
            _ => "system".to_string(),
        };
        self.schema_version = CURRENT_SETTINGS_SCHEMA_VERSION;
    }
}

#[derive(Debug, Clone)]
pub struct SettingsRepository {
    path: PathBuf,
    preview_path: Option<PathBuf>,
}

impl SettingsRepository {
    pub fn for_current_user() -> Option<Self> {
        let support_directory = application_support_directory()?;
        Some(Self {
            path: support_directory.join(SETTINGS_FILE_NAME),
            preview_path: gpui_preview_support_directory()
                .map(|directory| directory.join(SETTINGS_FILE_NAME)),
        })
    }

    #[cfg(test)]
    fn at(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            preview_path: None,
        }
    }

    #[cfg(test)]
    fn with_preview(path: impl Into<PathBuf>, preview_path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            preview_path: Some(preview_path.into()),
        }
    }

    pub fn load(&self) -> Result<AppSettings> {
        self.import_preview_settings()?;
        if !self.path.exists() {
            return Ok(AppSettings::default());
        }
        let metadata = fs::metadata(&self.path)?;
        if metadata.len() > MAX_SETTINGS_BYTES {
            bail!("{} supera el límite de 1 MiB", self.path.display());
        }
        let bytes = fs::read(&self.path)?;
        let mut settings: AppSettings = serde_json::from_slice(&bytes)
            .with_context(|| format!("JSON inválido en {}", self.path.display()))?;
        if settings.schema_version > CURRENT_SETTINGS_SCHEMA_VERSION {
            bail!(
                "{} usa settings schema {} pero esta versión entiende hasta {}",
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
        let parent = self.path.parent().context("settings.json no tiene padre")?;
        fs::create_dir_all(parent)?;
        let temporary = self.path.with_extension("json.tmp");
        fs::write(&temporary, serde_json::to_vec_pretty(settings)?)?;
        fs::rename(&temporary, &self.path)?;
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
        let parent = self.path.parent().context("settings.json no tiene padre")?;
        fs::create_dir_all(parent)?;
        fs::copy(preview_path, &self.path).with_context(|| {
            format!(
                "no se pudo importar {} a {}",
                preview_path.display(),
                self.path.display()
            )
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let settings = repository.load().unwrap();
        assert_eq!(settings.schema_version, CURRENT_SETTINGS_SCHEMA_VERSION);
        assert_eq!(settings.terminal_font_size, 32.0);
        repository.save(&settings).unwrap();
        assert_eq!(repository.load().unwrap(), settings);
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
            br#"{"schemaVersion":1,"terminalFontSize":14,"showHiddenFiles":true,"leftSidebarVisible":false,"gitPanelVisible":true}"#,
        )
        .unwrap();
        let repository = SettingsRepository::with_preview(&canonical, &preview);

        let settings = repository.load().unwrap();

        assert_eq!(settings.terminal_font_size, 14.0);
        assert!(settings.show_hidden_files);
        assert!(!settings.left_sidebar_visible);
        assert!(settings.git_panel_visible);
        assert!(canonical.exists());
        fs::remove_dir_all(root).unwrap();
    }
}
