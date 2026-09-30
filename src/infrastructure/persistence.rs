mod queue;

pub(crate) use queue::{
    DocumentKind, FinishError, PersistenceQueue, SaveResult, save_final_blocking,
};

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::domain::workspace::{
    CURRENT_WORKSPACE_SCHEMA_VERSION, ProjectSnapshot, SidebarItemSnapshot, WorkspaceSnapshot,
};
use crate::infrastructure::paths::{
    RevisionGuard, application_support_directory, atomic_write, gpui_preview_support_directory,
    read_file_limited, read_optional_file_limited, with_exclusive_file_lock,
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Deserializer};
use uuid::Uuid;

const WORKSPACE_FILE_NAME: &str = "workspace.json";
const SWIFT_BACKUP_FILE_NAME: &str = "workspace.swift-v0.2.7.backup.json";
const PROJECTS_BACKUP_FILE_NAME: &str = "workspace.pre-projects.backup.json";
const MAX_WORKSPACE_BYTES: u64 = 16 * 1024 * 1024;

/// Keep schema presence while decoding the hierarchy once. An explicit zero is
/// a versioned legacy file; an absent version identifies the original Swift format.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredWorkspace {
    #[serde(default, deserialize_with = "present_schema_version")]
    schema_version: Option<u32>,
    #[serde(default)]
    projects: Vec<ProjectSnapshot>,
    selected_project_id: Option<Uuid>,
    #[serde(default)]
    workspace_order: Vec<Uuid>,
    #[serde(default)]
    sidebar_items: Vec<SidebarItemSnapshot>,
}

fn present_schema_version<'de, D: Deserializer<'de>>(decoder: D) -> Result<Option<u32>, D::Error> {
    u32::deserialize(decoder).map(Some)
}

impl StoredWorkspace {
    fn into_snapshot(self) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            schema_version: self.schema_version.unwrap_or_default(),
            projects: self.projects,
            selected_project_id: self.selected_project_id,
            workspace_order: self.workspace_order,
            sidebar_items: self.sidebar_items,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorkspaceRepository {
    path: PathBuf,
    preview_path: Option<PathBuf>,
    swift_backup_path: PathBuf,
    revision: Arc<RevisionGuard>,
}

fn encode_workspace(snapshot: &WorkspaceSnapshot) -> Result<Vec<u8>> {
    serde_json::to_vec(snapshot).context("could not serialize the workspace")
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
        let loaded = with_exclusive_file_lock(&self.path, || self.read_and_back_up())?;
        let Some((data, mut snapshot)) = loaded else {
            self.revision.loaded(None);
            return Ok(None);
        };
        let original = snapshot.clone();
        snapshot.normalize();
        self.revision.loaded(Some(data));
        if snapshot != original {
            let normalized = encode_workspace(&snapshot)?;
            if normalized.len() as u64 > MAX_WORKSPACE_BYTES {
                bail!("the normalized workspace exceeds the 16 MiB limit");
            }
            self.revision.save(&self.path, &normalized)?;
        }
        Ok(Some(snapshot))
    }

    pub fn save(&self, snapshot: &WorkspaceSnapshot) -> Result<bool> {
        let data = encode_workspace(snapshot)?;
        if data.len() as u64 > MAX_WORKSPACE_BYTES {
            bail!("the workspace exceeds the 16 MiB limit and cannot be saved");
        }
        self.revision.save(&self.path, &data)
    }

    /// Called under the workspace lock so import, inspection, and backups refer
    /// to one revision. Normalization saves later with the same bytes as its guard.
    fn read_and_back_up(&self) -> Result<Option<(Vec<u8>, WorkspaceSnapshot)>> {
        let current = read_optional_file_limited(&self.path, MAX_WORKSPACE_BYTES, "16 MiB")?;
        let (source, data) = if let Some(data) = current {
            (&self.path, data)
        } else {
            let Some(preview) = &self.preview_path else {
                return Ok(None);
            };
            let Some(data) = read_optional_file_limited(preview, MAX_WORKSPACE_BYTES, "16 MiB")?
            else {
                return Ok(None);
            };
            (preview, data)
        };
        let parsed = serde_json::from_slice::<StoredWorkspace>(&data);
        // Preserve the legacy recovery behavior even for a structurally invalid
        // Swift document. Valid snapshots only need the typed decode above.
        if source == &self.path
            && parsed.is_err()
            && serde_json::from_slice::<serde_json::Value>(&data)
                .ok()
                .is_some_and(|value| {
                    value
                        .as_object()
                        .is_some_and(|object| !object.contains_key("schemaVersion"))
                })
        {
            self.back_up(&self.swift_backup_path, &data)?;
        }
        let stored = parsed.with_context(|| format!("Invalid JSON in {}", source.display()))?;
        let unversioned = stored.schema_version.is_none();
        let snapshot = stored.into_snapshot();
        if snapshot.schema_version > CURRENT_WORKSPACE_SCHEMA_VERSION {
            bail!(
                "{} uses schema {} but this version of Vibra only supports up to {}",
                source.display(),
                snapshot.schema_version,
                CURRENT_WORKSPACE_SCHEMA_VERSION
            );
        }
        if source != &self.path {
            atomic_write(&self.path, &data).with_context(|| {
                format!(
                    "could not import {} to {}",
                    source.display(),
                    self.path.display()
                )
            })?;
        }
        if unversioned {
            self.back_up(&self.swift_backup_path, &data)?;
        }
        if snapshot.schema_version < 7 {
            self.back_up(&self.path.with_file_name(PROJECTS_BACKUP_FILE_NAME), &data)?;
        }
        Ok(Some((data, snapshot)))
    }

    fn back_up(&self, path: &Path, data: &[u8]) -> Result<()> {
        if !valid_json_backup(path) {
            atomic_write(path, data).with_context(|| {
                format!(
                    "could not back up {} to {}",
                    self.path.display(),
                    path.display()
                )
            })?;
        }
        Ok(())
    }
}

fn read_workspace_file(path: &std::path::Path) -> Result<Vec<u8>> {
    read_file_limited(path, MAX_WORKSPACE_BYTES, "16 MiB")
}

fn valid_json_backup(path: &std::path::Path) -> bool {
    read_workspace_file(path)
        .ok()
        .is_some_and(|bytes| serde_json::from_slice::<serde::de::IgnoredAny>(&bytes).is_ok())
}

#[cfg(test)]
mod tests {
    use std::fs;
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
        assert!(migrated.close_terminal(migrated.selected_session().unwrap().id));
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
    fn explicit_zero_schema_uses_only_the_pre_projects_backup() {
        let root = std::env::temp_dir().join(format!("vibra-version-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("workspace.json");
        let original = br#"{"schemaVersion":0,"projects":[]}"#;
        fs::write(&path, original).unwrap();
        WorkspaceRepository::at(&path).load().unwrap();
        assert!(!root.join(SWIFT_BACKUP_FILE_NAME).exists());
        assert_eq!(
            fs::read(root.join(PROJECTS_BACKUP_FILE_NAME)).unwrap(),
            original
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_schemas_and_invalid_swift_documents_remain_recoverable() {
        for original in [
            br#"{"schemaVersion":null,"projects":[]}"#.as_slice(),
            br#"{"schemaVersion":999,"schemaVersion":7,"projects":[]}"#,
            br#"{"projects":"invalid"}"#,
        ] {
            let root = std::env::temp_dir().join(format!("vibra-invalid-{}", Uuid::new_v4()));
            fs::create_dir_all(&root).unwrap();
            let path = root.join("workspace.json");
            fs::write(&path, original).unwrap();
            let repository = WorkspaceRepository::at(&path);
            assert!(repository.load().is_err());
            assert!(repository.save(&WorkspaceSnapshot::default()).is_err());
            assert_eq!(fs::read(&path).unwrap(), original);
            if !original
                .windows(b"schemaVersion".len())
                .any(|part| part == b"schemaVersion")
            {
                assert_eq!(
                    fs::read(root.join(SWIFT_BACKUP_FILE_NAME)).unwrap(),
                    original
                );
            }
            fs::remove_dir_all(root).unwrap();
        }
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

        assert!(error.to_string().contains("schema 999"));
        assert!(repository.save(&WorkspaceSnapshot::default()).is_err());
        assert_eq!(
            fs::read(root.join("workspace.json")).unwrap(),
            br#"{"schemaVersion":999,"projects":[]}"#
        );
        fs::remove_dir_all(root).unwrap();
    }
}
