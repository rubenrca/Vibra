//! Last successful list, shown while the provider is refreshed in the background.
//! This is disposable data, separate from Inbox preferences and comment drafts.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{WorkItemsPage, WorkQuery, github, linear};
use crate::domain::work_items::WorkSource;
use crate::infrastructure::clock::unix_now;
use crate::infrastructure::paths::{atomic_write, read_file_limited};

const VERSION: u32 = 1;
const MAX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_AGE: u64 = 7 * 86_400;

#[derive(Serialize, Deserialize)]
struct Snapshot<T> {
    version: u32,
    key: String,
    saved_at: u64,
    page: T,
}

pub struct ListCache {
    path: PathBuf,
    key: String,
}

impl ListCache {
    /// Run off the UI thread. `gh auth token` only reads local credentials;
    /// no network request is needed to identify the cache's account. Only its
    /// digest is persisted, never the token itself.
    pub fn new(source: WorkSource, query: &WorkQuery) -> Option<Self> {
        let credential = match source {
            WorkSource::GitHub => {
                github::gh_output(&["auth", "token", "--hostname", "github.com"], None).ok()?
            }
            WorkSource::Linear => {
                read_file_limited(&linear::token_path().ok()?, 4096, "4 KiB").ok()?
            }
        };
        let credential = credential.trim_ascii();
        if credential.is_empty() {
            return None;
        }
        let root = directories::BaseDirs::new()?
            .cache_dir()
            .join("Vibra/inbox");
        Self::at(root, source, query, credential)
    }

    fn at(root: PathBuf, source: WorkSource, query: &WorkQuery, credential: &[u8]) -> Option<Self> {
        let mut digest = Sha256::new();
        digest.update(Sha256::digest(credential));
        digest.update(serde_json::to_vec(&(source, query)).ok()?);
        Some(Self {
            // One bounded snapshot per provider; changing filters/accounts
            // replaces it instead of accumulating private task history.
            path: root.join(format!("{}.json", source.label().to_ascii_lowercase())),
            key: format!("{:x}", digest.finalize()),
        })
    }

    pub fn load(&self) -> Option<WorkItemsPage> {
        let bytes = read_file_limited(&self.path, MAX_BYTES, "16 MiB").ok()?;
        let snapshot: Snapshot<WorkItemsPage> = serde_json::from_slice(&bytes).ok()?;
        let now = unix_now();
        (snapshot.version == VERSION
            && snapshot.key == self.key
            && snapshot.saved_at <= now
            && now - snapshot.saved_at <= MAX_AGE)
            .then_some(snapshot.page)
    }

    /// Cache failures must never prevent a live refresh. Preserve the last
    /// complete response when some repositories could not be fetched.
    pub fn save(&self, page: &WorkItemsPage) {
        if !page.connected || !page.failed_scopes.is_empty() {
            return;
        }
        let Ok(bytes) = serde_json::to_vec(&Snapshot {
            version: VERSION,
            key: self.key.clone(),
            saved_at: unix_now(),
            page,
        }) else {
            return;
        };
        if bytes.len() as u64 <= MAX_BYTES {
            let _ = atomic_write(&self.path, &bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::work_items::{WorkKind, WorkStatus, fixture};
    use crate::infrastructure::work_items::InboxProject;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn inbox_cache_restores_only_the_same_account_projects_and_filters() {
        let root = std::env::temp_dir().join(format!("vibra-inbox-cache-{}", uuid::Uuid::new_v4()));
        let query = WorkQuery {
            projects: vec![InboxProject {
                id: uuid::Uuid::new_v4(),
                root: root.clone(),
            }],
            assigned_to_me: false,
            status: None,
        };
        let cache = ListCache::at(root.clone(), WorkSource::GitHub, &query, b"account-a").unwrap();
        assert!(cache.load().is_none());
        cache.save(&WorkItemsPage {
            items: vec![fixture()],
            connected: true,
            ..Default::default()
        });
        assert_eq!(cache.load().unwrap().items[0].url, fixture().url);
        assert_eq!(
            fs::metadata(&cache.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(
            !fs::read_to_string(&cache.path)
                .unwrap()
                .contains("account-a")
        );
        let other = ListCache::at(root.clone(), WorkSource::GitHub, &query, b"account-b").unwrap();
        assert!(other.load().is_none());
        for changed in [
            WorkQuery {
                assigned_to_me: true,
                ..query.clone()
            },
            WorkQuery {
                status: Some(WorkStatus::Open),
                ..query.clone()
            },
            WorkQuery {
                projects: vec![],
                ..query.clone()
            },
        ] {
            assert!(
                ListCache::at(root.clone(), WorkSource::GitHub, &changed, b"account-a")
                    .unwrap()
                    .load()
                    .is_none()
            );
        }
        assert!(
            ListCache::at(root.clone(), WorkSource::Linear, &query, b"account-a")
                .unwrap()
                .load()
                .is_none()
        );
        // A partial error or disconnected provider cannot erase useful data.
        cache.save(&WorkItemsPage {
            connected: true,
            failed_scopes: vec![("demo/app".into(), WorkKind::Issue)],
            ..Default::default()
        });
        cache.save(&WorkItemsPage::default());
        assert_eq!(cache.load().unwrap().items.len(), 1);
        // A successful empty list is authoritative.
        cache.save(&WorkItemsPage {
            connected: true,
            ..Default::default()
        });
        assert!(cache.load().unwrap().items.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn inbox_cache_ignores_expired_incompatible_and_corrupt_snapshots() {
        let root = std::env::temp_dir().join(format!("vibra-inbox-cache-{}", uuid::Uuid::new_v4()));
        let query = WorkQuery {
            projects: vec![],
            assigned_to_me: false,
            status: None,
        };
        let cache = ListCache::at(root.clone(), WorkSource::GitHub, &query, b"account").unwrap();
        cache.save(&WorkItemsPage {
            connected: true,
            ..Default::default()
        });
        let mut snapshot: Snapshot<WorkItemsPage> =
            serde_json::from_slice(&fs::read(&cache.path).unwrap()).unwrap();
        snapshot.saved_at = unix_now() - MAX_AGE - 1;
        fs::write(&cache.path, serde_json::to_vec(&snapshot).unwrap()).unwrap();
        assert!(cache.load().is_none());
        snapshot.saved_at = unix_now();
        snapshot.version += 1;
        fs::write(&cache.path, serde_json::to_vec(&snapshot).unwrap()).unwrap();
        assert!(cache.load().is_none());
        fs::write(&cache.path, b"interrupted write").unwrap();
        assert!(cache.load().is_none());
        fs::OpenOptions::new()
            .write(true)
            .open(&cache.path)
            .unwrap()
            .set_len(MAX_BYTES + 1)
            .unwrap();
        assert!(cache.load().is_none());
        fs::remove_dir_all(root).unwrap();
    }
}
