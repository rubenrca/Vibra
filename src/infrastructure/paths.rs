use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result, bail};
use directories::{BaseDirs, ProjectDirs};
use uuid::Uuid;

const APP_SUPPORT_DIRECTORY: &str = "Vibra";

pub fn application_support_directory() -> Option<PathBuf> {
    Some(BaseDirs::new()?.data_dir().join(APP_SUPPORT_DIRECTORY))
}

pub fn gpui_preview_support_directory() -> Option<PathBuf> {
    ProjectDirs::from("dev", "rubenrca", "VibraGPUI")
        .map(|directories| directories.data_dir().to_path_buf())
}

/// Check the opened file and bound the read even if it grows after inspection.
pub fn read_file_limited(path: &Path, limit: u64, limit_label: &str) -> Result<Vec<u8>> {
    let file =
        fs::File::open(path).with_context(|| format!("could not open {}", path.display()))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("could not inspect {}", path.display()))?;
    if metadata.len() > limit {
        bail!("{} exceeds the {limit_label} limit", path.display());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .with_context(|| format!("could not read {}", path.display()))?;
    if bytes.len() as u64 > limit {
        bail!("{} exceeds the {limit_label} limit", path.display());
    }
    Ok(bytes)
}

/// Options for the shared tmp+rename write used by workspace, settings, files, and hooks.
#[derive(Debug, Clone, Default)]
pub struct AtomicWriteOptions {
    pub unix_mode: Option<u32>,
}

/// Atomically replace `path` with `bytes` via a sibling temp file + rename.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_write_with(path, bytes, AtomicWriteOptions::default())
}

/// Import an older file without replacing a snapshot another instance created.
pub fn atomic_write_if_missing(path: &Path, bytes: &[u8]) -> Result<bool> {
    with_exclusive_file_lock(path, || {
        match fs::symlink_metadata(path) {
            Ok(_) => return Ok(false),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("could not inspect {}", path.display()));
            }
        }
        atomic_write(path, bytes)?;
        Ok(true)
    })
}

pub fn atomic_write_with(path: &Path, bytes: &[u8], options: AtomicWriteOptions) -> Result<()> {
    let parent = path
        .parent()
        .context("the output path has no parent directory")?;
    fs::create_dir_all(parent).with_context(|| format!("could not create {}", parent.display()))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("the output path has no filename")?;
    // A process can write the same target from multiple threads. A unique name
    // also prevents an existing temporary symlink from redirecting the write.
    let temporary = parent.join(format!(".{name}.{}.tmp", Uuid::new_v4().simple()));
    write_temp(&temporary, bytes, &options)?;
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error).with_context(|| {
            format!(
                "could not move {} to {}",
                temporary.display(),
                path.display()
            )
        });
    }
    fs::File::open(parent)
        .with_context(|| format!("could not open {} for syncing", parent.display()))?
        .sync_all()
        .with_context(|| format!("could not sync {}", parent.display()))?;
    Ok(())
}

fn write_temp(temporary: &Path, bytes: &[u8], options: &AtomicWriteOptions) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        // Workspace and settings snapshots may contain user data. Keep the
        // temporary private before any bytes are written.
        .mode(options.unix_mode.unwrap_or(0o600))
        .open(temporary)
        .with_context(|| format!("could not create {}", temporary.display()))?;
    let result = (|| {
        if let Some(mode) = options.unix_mode {
            use std::os::unix::fs::PermissionsExt;
            output.set_permissions(fs::Permissions::from_mode(mode))?;
        }
        output.write_all(bytes)?;
        output.sync_all()
    })();
    if result.is_err() {
        // The file was created by this call. An open failure above must not
        // remove an unrelated file that happened to have the same name.
        let _ = fs::remove_file(temporary);
    }
    result.map_err(Into::into)
}

/// Serialize the read/compare/replace sequence across Vibra processes. The lock
/// has its own inode because the data file is replaced by `atomic_write`.
pub fn with_exclusive_file_lock<T>(
    path: &Path,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;

    let parent = path
        .parent()
        .context("the lock path has no parent directory")?;
    fs::create_dir_all(parent)?;
    let mut name = path
        .file_name()
        .context("the lock path has no filename")?
        .to_os_string();
    name.push(".lock");
    let lock_path = parent.join(name);
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&lock_path)
        .with_context(|| format!("could not open {}", lock_path.display()))?;
    loop {
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } == 0 {
            break;
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error).with_context(|| format!("could not lock {}", lock_path.display()));
        }
    }
    operation()
}

#[derive(Debug, Default)]
enum FileRevision {
    #[default]
    Unloaded,
    Loaded(Option<Vec<u8>>),
    Blocked(String),
}

/// Remembers exactly which bytes the app loaded. A second process cannot
/// silently replace a user's newer workspace or settings snapshot.
#[derive(Debug, Default)]
pub struct RevisionGuard {
    revision: Mutex<FileRevision>,
    recovery_path: Mutex<Option<PathBuf>>,
    /// Last submitted settings, which may differ from the merged file on disk.
    merge_input: Mutex<Option<Vec<u8>>>,
}

impl RevisionGuard {
    pub fn loaded(&self, bytes: Option<Vec<u8>>) {
        let mut revision = self
            .revision
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *self
            .merge_input
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *revision = FileRevision::Loaded(bytes);
    }

    pub fn blocked(&self, error: &anyhow::Error) {
        *self
            .revision
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            FileRevision::Blocked(error.to_string());
    }

    /// Merge only locally changed preferences into the latest file while holding
    /// the process lock. Workspaces and libraries continue to use strict `save`.
    pub fn save_merging(
        &self,
        path: &Path,
        bytes: &[u8],
        limit: u64,
        merge: impl FnOnce(Option<&[u8]>, Option<&[u8]>) -> Result<Vec<u8>>,
    ) -> Result<bool> {
        let mut revision = self
            .revision
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let FileRevision::Blocked(error) = &*revision {
            bail!("cannot save because the initial load failed: {error}");
        }
        let mut merge_input = self
            .merge_input
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        with_exclusive_file_lock(path, || {
            let current = match fs::File::open(path) {
                Ok(file) => {
                    let mut contents = Vec::new();
                    file.take(limit + 1).read_to_end(&mut contents)?;
                    if contents.len() as u64 > limit {
                        let recovery = self.preserve_local_copy(path, bytes)?;
                        bail!(
                            "{} exceeds the settings size limit; your local copy was saved at {}",
                            path.display(),
                            recovery.display()
                        );
                    }
                    Some(contents)
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error).context("could not read the latest settings"),
            };
            let loaded = match &*revision {
                FileRevision::Unloaded if current.is_some() => {
                    bail!(
                        "{} must be loaded before it can be overwritten",
                        path.display()
                    );
                }
                FileRevision::Loaded(loaded) => loaded.as_deref(),
                _ => None,
            };
            let merged = match merge(merge_input.as_deref().or(loaded), current.as_deref()) {
                Ok(merged) => merged,
                Err(error) => {
                    let recovery = self.preserve_local_copy(path, bytes)?;
                    bail!(
                        concat!(
                            "{error}; the settings file was preserved ",
                            "and your local copy was saved at {}"
                        ),
                        recovery.display(),
                        error = error
                    );
                }
            };
            if merged.len() as u64 > limit {
                bail!("merged settings exceed the size limit and cannot be saved");
            }
            let changed = current.as_deref() != Some(merged.as_slice());
            if changed {
                atomic_write(path, &merged)?;
            }
            *revision = FileRevision::Loaded(Some(merged));
            // Remember what the caller submitted, not the merged values it has
            // not seen. A later resize must not undo another instance's theme.
            *merge_input = Some(bytes.to_vec());
            Ok(changed)
        })
    }

    fn preserve_local_copy(&self, path: &Path, bytes: &[u8]) -> Result<PathBuf> {
        let recovery_path = {
            let mut slot = self
                .recovery_path
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            slot.get_or_insert_with(|| {
                let name = path
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .unwrap_or("snapshot");
                path.with_file_name(format!(
                    "{name}-recovery-{}-{}.json",
                    std::process::id(),
                    Uuid::new_v4().simple()
                ))
            })
            .clone()
        };
        atomic_write(&recovery_path, bytes).with_context(|| {
            format!(
                "could not preserve the local copy at {}",
                recovery_path.display()
            )
        })?;
        Ok(recovery_path)
    }

    pub fn save(&self, path: &Path, bytes: &[u8]) -> Result<bool> {
        let mut revision = self
            .revision
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let FileRevision::Blocked(error) = &*revision {
            bail!("cannot save because the initial load failed: {error}");
        }
        with_exclusive_file_lock(path, || {
            let comparison_limit = match &*revision {
                FileRevision::Loaded(Some(expected)) => expected.len().max(bytes.len()),
                _ => bytes.len(),
            } as u64;
            let (current, oversized) = match fs::File::open(path) {
                Ok(file) => {
                    let size = file.metadata()?.len();
                    if size > comparison_limit {
                        (Some(Vec::new()), true)
                    } else {
                        let mut contents = Vec::with_capacity(size as usize);
                        file.take(comparison_limit + 1).read_to_end(&mut contents)?;
                        if contents.len() as u64 > comparison_limit {
                            (Some(Vec::new()), true)
                        } else {
                            (Some(contents), false)
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (None, false),
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("could not read {}", path.display()));
                }
            };
            match &*revision {
                FileRevision::Loaded(expected) if oversized || *expected != current => {
                    let recovery_path = self.preserve_local_copy(path, bytes)?;
                    bail!(
                        concat!(
                            "{} changed in another process; the newer file was preserved ",
                            "and your local copy was saved at {}"
                        ),
                        path.display(),
                        recovery_path.display()
                    );
                }
                FileRevision::Unloaded if current.is_some() => {
                    bail!(
                        "{} must be loaded before it can be overwritten",
                        path.display()
                    );
                }
                FileRevision::Blocked(_) => unreachable!("checked above"),
                _ => {}
            }
            if !oversized && current.as_deref() == Some(bytes) {
                *revision = FileRevision::Loaded(current);
                return Ok(false);
            }
            atomic_write(path, bytes)?;
            *revision = FileRevision::Loaded(Some(bytes.to_vec()));
            Ok(true)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn limited_reads_accept_the_boundary_and_reject_oversized_files() {
        let root = std::env::temp_dir().join(format!("vibra-read-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("data.json");
        fs::write(&path, b"1234").unwrap();
        assert_eq!(read_file_limited(&path, 4, "4 bytes").unwrap(), b"1234");
        assert!(
            read_file_limited(&path, 3, "3 bytes")
                .unwrap_err()
                .to_string()
                .contains("3 bytes limit")
        );
        fs::write(&path, b"").unwrap();
        assert!(read_file_limited(&path, 0, "0 bytes").unwrap().is_empty());
        fs::remove_file(&path).unwrap();
        let error = read_file_limited(&path, 4, "4 bytes").unwrap_err();
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::NotFound
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn atomic_write_replaces_the_target_file() {
        let root = std::env::temp_dir().join(format!("vibra-atomic-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("note.txt");
        atomic_write(&path, b"one").unwrap();
        atomic_write(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn imports_wait_for_a_writer_and_preserve_its_new_snapshot() {
        use std::sync::mpsc;
        use std::time::Duration;

        let root = std::env::temp_dir().join(format!("vibra-import-{}", Uuid::new_v4()));
        let path = root.join("workspace.json");
        let (started, start) = mpsc::channel();
        let (completed, completion) = mpsc::channel();
        let importer = with_exclusive_file_lock(&path, || {
            let import_path = path.clone();
            let importer = std::thread::spawn(move || {
                started.send(()).unwrap();
                completed
                    .send(atomic_write_if_missing(&import_path, b"old preview"))
                    .unwrap();
            });
            start.recv().unwrap();
            assert!(matches!(
                completion.recv_timeout(Duration::from_millis(100)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ));
            atomic_write(&path, b"new snapshot")?;
            Ok(importer)
        })
        .unwrap();
        importer.join().unwrap();
        assert!(!completion.recv().unwrap().unwrap());
        assert_eq!(fs::read(&path).unwrap(), b"new snapshot");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_does_not_follow_an_existing_temporary_symlink() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!("vibra-atomic-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("note.txt");
        let sentinel = root.join("sentinel.txt");
        fs::write(&sentinel, b"safe").unwrap();
        symlink(
            &sentinel,
            root.join(format!(".note.txt.{}.tmp", std::process::id())),
        )
        .unwrap();

        atomic_write(&path, b"new content").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"new content");
        assert_eq!(fs::read(&sentinel).unwrap(), b"safe");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_atomic_writes_do_not_mix_contents() {
        let root = std::env::temp_dir().join(format!("vibra-atomic-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("note.txt");
        let writers: Vec<_> = (0..8)
            .map(|index| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let bytes = vec![b'a' + index; 16 * 1024];
                    atomic_write(&path, &bytes).unwrap();
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        let bytes = fs::read(&path).unwrap();
        assert_eq!(bytes.len(), 16 * 1024);
        assert!(bytes.iter().all(|byte| *byte == bytes[0]));
        fs::remove_dir_all(root).unwrap();
    }
}
