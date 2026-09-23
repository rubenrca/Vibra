use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;

use crate::domain::workspace::{CURRENT_WORKSPACE_SCHEMA_VERSION, WorkspaceSnapshot};
use crate::infrastructure::paths::{
    RevisionGuard, application_support_directory, atomic_write, gpui_preview_support_directory,
};
use anyhow::{Context, Result, bail};

const WORKSPACE_FILE_NAME: &str = "workspace.json";
const SWIFT_BACKUP_FILE_NAME: &str = "workspace.swift-v0.2.7.backup.json";
const PROJECTS_BACKUP_FILE_NAME: &str = "workspace.pre-projects.backup.json";
const MAX_WORKSPACE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug)]
pub struct WorkspaceRepository {
    path: PathBuf,
    preview_path: Option<PathBuf>,
    swift_backup_path: PathBuf,
    revision: Arc<RevisionGuard>,
}

impl Clone for WorkspaceRepository {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            preview_path: self.preview_path.clone(),
            swift_backup_path: self.swift_backup_path.clone(),
            revision: self.revision.clone(),
        }
    }
}

fn encode_workspace(snapshot: &WorkspaceSnapshot) -> Result<Vec<u8>> {
    serde_json::to_vec(snapshot).context("no se pudo serializar el workspace")
}

impl WorkspaceRepository {
    pub fn for_current_user() -> Option<Self> {
        let support_directory = application_support_directory()?;
        Some(Self {
            path: support_directory.join(WORKSPACE_FILE_NAME),
            preview_path: gpui_preview_support_directory()
                .map(|directory| directory.join(WORKSPACE_FILE_NAME)),
            swift_backup_path: support_directory.join(SWIFT_BACKUP_FILE_NAME),
            revision: Arc::new(RevisionGuard::default()),
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
            revision: Arc::new(RevisionGuard::default()),
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
            revision: Arc::new(RevisionGuard::default()),
        }
    }

    pub fn load(&self) -> Result<Option<WorkspaceSnapshot>> {
        let result = self.load_inner();
        if let Err(error) = &result {
            self.revision.blocked(error);
        }
        result
    }

    fn load_inner(&self) -> Result<Option<WorkspaceSnapshot>> {
        self.prepare_migration()?;
        if !self.path.exists() {
            self.revision.loaded(None);
            return Ok(None);
        }
        let data = read_workspace_file(&self.path)?;
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
        if snapshot.schema_version < 7 {
            let backup = self.path.with_file_name(PROJECTS_BACKUP_FILE_NAME);
            if !valid_json_backup(&backup) {
                atomic_write(&backup, &data)
                    .with_context(|| format!("no se pudo respaldar {}", self.path.display()))?;
            }
        }
        let original = snapshot.clone();
        snapshot.normalize();
        self.revision.loaded(Some(data));
        if snapshot != original {
            let normalized = encode_workspace(&snapshot)?;
            if normalized.len() as u64 > MAX_WORKSPACE_BYTES {
                bail!("el workspace normalizado supera el límite de 16 MiB");
            }
            self.revision.save(&self.path, &normalized)?;
        }
        Ok(Some(snapshot))
    }

    pub fn save(&self, snapshot: &WorkspaceSnapshot) -> Result<bool> {
        let data = encode_workspace(snapshot)?;
        if data.len() as u64 > MAX_WORKSPACE_BYTES {
            bail!("el workspace supera el límite de 16 MiB y no se puede guardar");
        }
        self.revision.save(&self.path, &data)
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
            let data = read_workspace_file(preview_path)?;
            let preview: WorkspaceSnapshot = serde_json::from_slice(&data)
                .with_context(|| format!("JSON inválido en {}", preview_path.display()))?;
            if preview.schema_version > CURRENT_WORKSPACE_SCHEMA_VERSION {
                bail!("{} usa un esquema futuro", preview_path.display());
            }
            atomic_write(&self.path, &data).with_context(|| {
                format!(
                    "no se pudo importar {} a {}",
                    preview_path.display(),
                    self.path.display()
                )
            })?;
        }

        if self.path.exists() && !valid_json_backup(&self.swift_backup_path) {
            let data = read_workspace_file(&self.path)?;
            let is_swift_snapshot = serde_json::from_slice::<serde_json::Value>(&data)
                .ok()
                .and_then(|value| {
                    value
                        .as_object()
                        .map(|object| !object.contains_key("schemaVersion"))
                })
                .unwrap_or(false);
            if is_swift_snapshot {
                atomic_write(&self.swift_backup_path, &data).with_context(|| {
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

fn read_workspace_file(path: &std::path::Path) -> Result<Vec<u8>> {
    let file =
        fs::File::open(path).with_context(|| format!("no se pudo abrir {}", path.display()))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("no se pudo inspeccionar {}", path.display()))?;
    if metadata.len() > MAX_WORKSPACE_BYTES {
        bail!("{} supera el límite de 16 MiB", path.display());
    }
    let mut data = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_WORKSPACE_BYTES + 1)
        .read_to_end(&mut data)
        .with_context(|| format!("no se pudo leer {}", path.display()))?;
    if data.len() as u64 > MAX_WORKSPACE_BYTES {
        bail!("{} supera el límite de 16 MiB", path.display());
    }
    Ok(data)
}

fn valid_json_backup(path: &std::path::Path) -> bool {
    read_workspace_file(path)
        .ok()
        .is_some_and(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).is_ok())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::domain::workspace::{ProjectSnapshot, SessionSnapshot};
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
        let encoded = fs::read(root.join("workspace.json")).unwrap();
        assert!(
            !encoded.contains(&b' ') || !std::str::from_utf8(&encoded).unwrap().contains("\n  "),
            "workspace.json should be compact, not pretty-printed"
        );
        assert!(!repository.save(&expected).unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn repository_preserves_external_changes_instead_of_overwriting_them() {
        let root = std::env::temp_dir().join(format!("vibra-gpui-{}", Uuid::new_v4()));
        let path = root.join("workspace.json");
        let repository = WorkspaceRepository::at(&path);
        let snapshot = WorkspaceSnapshot::default();
        repository.save(&snapshot).unwrap();
        let mut changed = snapshot.clone();
        changed.create_workspace(Path::new("/tmp/local-change"));
        fs::write(&path, b"external edit").unwrap();
        let error = repository.save(&changed).unwrap_err();
        assert_eq!(fs::read(&path).unwrap(), b"external edit");
        let recovery = fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("workspace-recovery-"))
            })
            .unwrap();
        assert!(error.to_string().contains(&recovery.display().to_string()));
        assert_eq!(
            fs::read(&recovery).unwrap(),
            encode_workspace(&changed).unwrap()
        );

        fs::remove_file(&path).unwrap();
        changed.create_workspace(Path::new("/tmp/newer-local-change"));
        assert!(repository.save(&changed).is_err());
        assert_eq!(
            fs::read(&recovery).unwrap(),
            encode_workspace(&changed).unwrap()
        );
        assert!(!path.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn two_loaded_repositories_cannot_overwrite_each_other() {
        let root = std::env::temp_dir().join(format!("vibra-gpui-{}", Uuid::new_v4()));
        let path = root.join("workspace.json");
        let first = WorkspaceRepository::at(&path);
        let second = WorkspaceRepository::at(&path);
        assert!(first.load().unwrap().is_none());
        assert!(second.load().unwrap().is_none());
        let mut first_snapshot = WorkspaceSnapshot::default();
        first_snapshot.create_workspace(Path::new("/tmp/first"));
        let mut second_snapshot = WorkspaceSnapshot::default();
        second_snapshot.create_workspace(Path::new("/tmp/second"));
        first.save(&first_snapshot).unwrap();
        assert!(second.save(&second_snapshot).is_err());
        assert_eq!(first.load().unwrap().unwrap(), first_snapshot);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn external_oversized_file_is_not_read_or_replaced_during_save() {
        let root = std::env::temp_dir().join(format!("vibra-gpui-{}", Uuid::new_v4()));
        let path = root.join("workspace.json");
        let repository = WorkspaceRepository::at(&path);
        let original = WorkspaceSnapshot::default();
        repository.save(&original).unwrap();
        fs::File::create(&path)
            .unwrap()
            .set_len(32 * 1024 * 1024)
            .unwrap();
        let mut changed = original;
        changed.create_workspace(Path::new("/tmp/local"));

        assert!(repository.save(&changed).is_err());
        assert_eq!(fs::metadata(&path).unwrap().len(), 32 * 1024 * 1024);
        assert!(fs::read_dir(&root).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("workspace-recovery-")
        }));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oversized_workspace_is_rejected_before_replacing_the_last_good_file() {
        let root = std::env::temp_dir().join(format!("vibra-gpui-{}", Uuid::new_v4()));
        let path = root.join("workspace.json");
        let repository = WorkspaceRepository::at(&path);
        let snapshot = WorkspaceSnapshot::default();
        repository.save(&snapshot).unwrap();
        let original = fs::read(&path).unwrap();
        let mut oversized = snapshot;
        oversized.create_workspace(Path::new("/tmp/project"));
        oversized.projects[0].name = "x".repeat(MAX_WORKSPACE_BYTES as usize);
        assert!(repository.save(&oversized).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn project_migration_backs_up_original_json_once_and_persists_empty_projects() {
        let root = std::env::temp_dir().join(format!("vibra-project-migration-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("workspace.json");
        let repository = WorkspaceRepository::at(&path);
        let mut legacy = WorkspaceSnapshot::default();
        legacy.create_workspace(Path::new("/projects/demo"));
        legacy.schema_version = 6;
        let original = serde_json::to_vec(&legacy).unwrap();
        fs::write(&path, &original).unwrap();

        let mut migrated = repository.load().unwrap().unwrap();
        let backup = root.join(PROJECTS_BACKUP_FILE_NAME);
        assert_eq!(fs::read(&backup).unwrap(), original);
        assert_eq!(migrated.schema_version, CURRENT_WORKSPACE_SCHEMA_VERSION);
        assert!(migrated.close_selected_terminal());
        repository.save(&migrated).unwrap();
        assert_eq!(repository.load().unwrap().unwrap(), migrated);
        assert_eq!(migrated.projects.len(), 1);
        assert_eq!(fs::read(&backup).unwrap(), original);
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
    fn legacy_generated_ids_are_persisted_during_load() {
        let root = std::env::temp_dir().join(format!("vibra-gpui-{}", Uuid::new_v4()));
        let path = root.join("workspace.json");
        let repository = WorkspaceRepository::at(&path);
        let session = SessionSnapshot::new("/tmp/legacy".into());
        let legacy = WorkspaceSnapshot {
            schema_version: 0,
            projects: vec![ProjectSnapshot {
                id: Uuid::new_v4(),
                name: "Legacy".into(),
                root_path: "/tmp/legacy".into(),
                collapsed: false,
                selected_session_id: Some(session.id),
                visible_session_ids: Some(vec![session.id]),
                sessions: vec![session],
                split_axis: None,
                tabs: None,
                selected_tab_id: None,
                workspaces: None,
                selected_workspace_id: None,
            }],
            selected_project_id: None,
            workspace_order: Vec::new(),
            sidebar_items: Vec::new(),
        };
        fs::create_dir_all(&root).unwrap();
        fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        let first = repository.load().unwrap().unwrap();
        let second = WorkspaceRepository::at(&path).load().unwrap().unwrap();
        assert_eq!(first, second);
        assert_eq!(second.schema_version, CURRENT_WORKSPACE_SCHEMA_VERSION);
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
        assert!(repository.save(&WorkspaceSnapshot::default()).is_err());
        assert_eq!(
            fs::read(root.join("workspace.json")).unwrap(),
            br#"{"schemaVersion":999,"projects":[]}"#
        );
        fs::remove_dir_all(root).unwrap();
    }
}
