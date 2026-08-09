use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

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
}

const fn default_terminal_font_size() -> f32 {
    12.0
}

const fn default_true() -> bool {
    true
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SETTINGS_SCHEMA_VERSION,
            terminal_font_size: default_terminal_font_size(),
            show_hidden_files: false,
            left_sidebar_visible: true,
            git_panel_visible: false,
        }
    }
}

impl AppSettings {
    fn normalize(&mut self) {
        if !self.terminal_font_size.is_finite() {
            self.terminal_font_size = default_terminal_font_size();
        }
        self.terminal_font_size = self.terminal_font_size.clamp(8.0, 32.0);
        self.schema_version = CURRENT_SETTINGS_SCHEMA_VERSION;
    }
}

#[derive(Debug, Clone)]
pub struct SettingsRepository {
    path: PathBuf,
}

impl SettingsRepository {
    pub fn for_current_user() -> Option<Self> {
        let project_dirs = ProjectDirs::from("dev", "rubenrca", "VibraGPUI")?;
        Some(Self {
            path: project_dirs.data_dir().join("settings.json"),
        })
    }

    #[cfg(test)]
    fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn load(&self) -> Result<AppSettings> {
        if !self.path.exists() {
            return Ok(AppSettings::default());
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
        let parent = self.path.parent().context("settings.json no tiene padre")?;
        fs::create_dir_all(parent)?;
        let temporary = self.path.with_extension("json.tmp");
        fs::write(&temporary, serde_json::to_vec_pretty(settings)?)?;
        fs::rename(&temporary, &self.path)?;
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
}
