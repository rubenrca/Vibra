use std::fs;
use std::path::PathBuf;

use crate::domain::workspace::{CURRENT_WORKSPACE_SCHEMA_VERSION, WorkspaceSnapshot};
use crate::infrastructure::paths::{application_support_directory, gpui_preview_support_directory};
use anyhow::{Context, Result, bail};

const WORKSPACE_FILE_NAME: &str = "workspace.json";
const SWIFT_BACKUP_FILE_NAME: &str = "workspace.swift-v0.2.7.backup.json";

#[derive(Debug, Clone)]
pub struct WorkspaceRepository {
    path: PathBuf,
    preview_path: Option<PathBuf>,
    swift_backup_path: PathBuf,
}

impl WorkspaceRepository {
    pub fn for_current_user() -> Option<Self> {
        let support_directory = application_support_directory()?;
        Some(Self {
            path: support_directory.join(WORKSPACE_FILE_NAME),
            preview_path: gpui_preview_support_directory()
                .map(|directory| directory.join(WORKSPACE_FILE_NAME)),
            swift_backup_path: support_directory.join(SWIFT_BACKUP_FILE_NAME),
        })
    }

    #[cfg(test)]
    pub fn at(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let swift_backup_path = path.with_file_name(SWIFT_BACKUP_FILE_NAME);
        Self {
            path,
            preview_path: None,
            swift_backup_path,
        }
    }

    #[cfg(test)]
    fn with_preview(path: impl Into<PathBuf>, preview_path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let swift_backup_path = path.with_file_name(SWIFT_BACKUP_FILE_NAME);
        Self {
            path,
            preview_path: Some(preview_path.into()),
            swift_backup_path,
        }
    }

    pub fn load(&self) -> Result<Option<WorkspaceSnapshot>> {
        self.prepare_migration()?;
        if !self.path.exists() {
            return Ok(None);
        }
        let data = fs::read(&self.path)
            .with_context(|| format!("no se pudo leer {}", self.path.display()))?;
        let mut snapshot: WorkspaceSnapshot = serde_json::from_slice(&data)
            .with_context(|| format!("JSON inválido en {}", self.path.display()))?;
        if snapshot.schema_version > CURRENT_WORKSPACE_SCHEMA_VERSION {
            bail!(
                "{} usa el esquema {} pero esta versión de Vibra solo entiende hasta el {}",
                self.path.display(),
                snapshot.schema_version,
                CURRENT_WORKSPACE_SCHEMA_VERSION
            );
        }
        snapshot.normalize();
        Ok(Some(snapshot))
    }

    pub fn save(&self, snapshot: &WorkspaceSnapshot) -> Result<()> {
        self.prepare_migration()?;
        let parent = self
            .path
            .parent()
            .context("workspace.json no tiene directorio padre")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("no se pudo crear {}", parent.display()))?;

        let temporary = self.path.with_extension("json.tmp");
        let data = serde_json::to_vec_pretty(snapshot)?;
        fs::write(&temporary, data)
            .with_context(|| format!("no se pudo escribir {}", temporary.display()))?;
        fs::rename(&temporary, &self.path).with_context(|| {
            format!(
                "no se pudo mover {} a {}",
                temporary.display(),
                self.path.display()
            )
        })?;
        Ok(())
    }

    fn prepare_migration(&self) -> Result<()> {
        if !self.path.exists()
            && let Some(preview_path) = self
                .preview_path
                .as_ref()
                .filter(|preview_path| preview_path.exists())
        {
            let parent = self
                .path
                .parent()
                .context("workspace.json no tiene directorio padre")?;
            fs::create_dir_all(parent)
                .with_context(|| format!("no se pudo crear {}", parent.display()))?;
            fs::copy(preview_path, &self.path).with_context(|| {
                format!(
                    "no se pudo importar {} a {}",
                    preview_path.display(),
                    self.path.display()
                )
            })?;
        }

        if self.path.exists() && !self.swift_backup_path.exists() {
            let data = fs::read(&self.path)
                .with_context(|| format!("no se pudo leer {}", self.path.display()))?;
            let is_swift_snapshot = serde_json::from_slice::<serde_json::Value>(&data)
                .ok()
                .and_then(|value| value.as_object().cloned())
                .is_some_and(|object| !object.contains_key("schemaVersion"));
            if is_swift_snapshot {
                fs::copy(&self.path, &self.swift_backup_path).with_context(|| {
                    format!(
                        "no se pudo respaldar {} en {}",
                        self.path.display(),
                        self.swift_backup_path.display()
                    )
                })?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use uuid::Uuid;

    #[test]
    fn repository_round_trips_a_snapshot() {
        let root = std::env::temp_dir().join(format!("vibra-gpui-{}", Uuid::new_v4()));
        let repository = WorkspaceRepository::at(root.join("workspace.json"));
        let mut expected = WorkspaceSnapshot::default();
        expected.create_workspace(Path::new("/tmp/vibra-gpui-round-trip"));

        repository.save(&expected).unwrap();
        let actual = repository.load().unwrap().unwrap();

        assert_eq!(actual, expected);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn repository_migrates_an_unversioned_snapshot() {
        let root = std::env::temp_dir().join(format!("vibra-gpui-{}", Uuid::new_v4()));
        let repository = WorkspaceRepository::at(root.join("workspace.json"));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("workspace.json"),
            br#"{"projects":[],"selectedProjectId":null}"#,
        )
        .unwrap();

        let snapshot = repository.load().unwrap().unwrap();

        assert_eq!(snapshot.schema_version, CURRENT_WORKSPACE_SCHEMA_VERSION);
        assert_eq!(
            fs::read(root.join(SWIFT_BACKUP_FILE_NAME)).unwrap(),
            br#"{"projects":[],"selectedProjectId":null}"#
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn repository_imports_the_gpui_preview_when_vibra_has_no_workspace() {
        let root = std::env::temp_dir().join(format!("vibra-import-{}", Uuid::new_v4()));
        let canonical = root.join("Vibra/workspace.json");
        let preview = root.join("VibraGPUI/workspace.json");
        fs::create_dir_all(preview.parent().unwrap()).unwrap();
        fs::write(
            &preview,
            br#"{"schemaVersion":3,"projects":[],"selectedProjectId":null}"#,
        )
        .unwrap();
        let repository = WorkspaceRepository::with_preview(&canonical, &preview);

        let snapshot = repository.load().unwrap().unwrap();

        assert_eq!(snapshot.schema_version, CURRENT_WORKSPACE_SCHEMA_VERSION);
        assert!(canonical.exists());
        assert!(!root.join("Vibra").join(SWIFT_BACKUP_FILE_NAME).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn repository_rejects_a_snapshot_from_a_newer_schema() {
        let root = std::env::temp_dir().join(format!("vibra-gpui-{}", Uuid::new_v4()));
        let repository = WorkspaceRepository::at(root.join("workspace.json"));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("workspace.json"),
            br#"{"schemaVersion":999,"projects":[]}"#,
        )
        .unwrap();

        let error = repository.load().unwrap_err();

        assert!(error.to_string().contains("esquema 999"));
        fs::remove_dir_all(root).unwrap();
    }
}
