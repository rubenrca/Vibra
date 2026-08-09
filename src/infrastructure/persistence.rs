use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use directories::ProjectDirs;

use crate::domain::workspace::{CURRENT_WORKSPACE_SCHEMA_VERSION, WorkspaceSnapshot};

#[derive(Debug, Clone)]
pub struct WorkspaceRepository {
    path: PathBuf,
}

impl WorkspaceRepository {
    pub fn for_current_user() -> Option<Self> {
        let project_dirs = ProjectDirs::from("dev", "rubenrca", "VibraGPUI")?;
        Some(Self {
            path: project_dirs.data_dir().join("workspace.json"),
        })
    }

    #[cfg(test)]
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn load(&self) -> Result<Option<WorkspaceSnapshot>> {
        if !self.path.exists() {
            return Ok(None);
        }
        let data = fs::read(&self.path)
            .with_context(|| format!("no se pudo leer {}", self.path.display()))?;
        let mut snapshot: WorkspaceSnapshot = serde_json::from_slice(&data)
            .with_context(|| format!("JSON inválido en {}", self.path.display()))?;
        if snapshot.schema_version > CURRENT_WORKSPACE_SCHEMA_VERSION {
            bail!(
                "{} usa el esquema {} pero esta versión de VibraGPUI solo entiende hasta el {}",
                self.path.display(),
                snapshot.schema_version,
                CURRENT_WORKSPACE_SCHEMA_VERSION
            );
        }
        snapshot.normalize();
        Ok(Some(snapshot))
    }

    pub fn save(&self, snapshot: &WorkspaceSnapshot) -> Result<()> {
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
