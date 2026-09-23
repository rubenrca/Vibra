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

/// Options for the shared tmp+rename write used by workspace, settings, files, and hooks.
#[derive(Debug, Clone, Default)]
pub struct AtomicWriteOptions {
    pub unix_mode: Option<u32>,
}

/// Atomically replace `path` with `bytes` via a sibling temp file + rename.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_write_with(path, bytes, AtomicWriteOptions::default())
}

pub fn atomic_write_with(path: &Path, bytes: &[u8], options: AtomicWriteOptions) -> Result<()> {
    let parent = path
        .parent()
        .context("la ruta de escritura no tiene directorio padre")?;
    fs::create_dir_all(parent).with_context(|| format!("no se pudo crear {}", parent.display()))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("la ruta de escritura no tiene nombre")?;
    // A process can write the same target from multiple threads. A unique name
    // also prevents an existing temporary symlink from redirecting the write.
    let temporary = parent.join(format!(".{name}.{}.tmp", Uuid::new_v4().simple()));
    write_temp(&temporary, bytes, &options)?;
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error).with_context(|| {
            format!(
                "no se pudo mover {} a {}",
                temporary.display(),
                path.display()
            )
        });
    }
    fs::File::open(parent)
        .with_context(|| format!("no se pudo abrir {} para sincronizar", parent.display()))?
        .sync_all()
        .with_context(|| format!("no se pudo sincronizar {}", parent.display()))?;
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
        .with_context(|| format!("no se pudo crear {}", temporary.display()))?;
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
        .context("la ruta de bloqueo no tiene directorio padre")?;
    fs::create_dir_all(parent)?;
    let mut name = path
        .file_name()
        .context("la ruta de bloqueo no tiene nombre")?
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
        .with_context(|| format!("no se pudo abrir {}", lock_path.display()))?;
    loop {
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } == 0 {
            break;
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error)
                .with_context(|| format!("no se pudo bloquear {}", lock_path.display()));
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
}

impl RevisionGuard {
    pub fn loaded(&self, bytes: Option<Vec<u8>>) {
        *self
            .revision
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = FileRevision::Loaded(bytes);
    }

    pub fn blocked(&self, error: &anyhow::Error) {
        *self
            .revision
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            FileRevision::Blocked(error.to_string());
    }

    pub fn save(&self, path: &Path, bytes: &[u8]) -> Result<bool> {
        let mut revision = self
            .revision
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let FileRevision::Blocked(error) = &*revision {
            bail!("no se puede guardar porque falló la carga original: {error}");
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
                        .with_context(|| format!("no se pudo leer {}", path.display()));
                }
            };
            match &*revision {
                FileRevision::Loaded(expected) if oversized || *expected != current => {
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
                            "no se pudo conservar la copia local en {}",
                            recovery_path.display()
                        )
                    })?;
                    bail!(
                        "{} cambió en otro proceso; se conservó el archivo nuevo y tu copia local quedó en {}",
                        path.display(),
                        recovery_path.display()
                    );
                }
                FileRevision::Unloaded if current.is_some() => {
                    bail!("{} debe cargarse antes de sobrescribirlo", path.display());
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
    fn atomic_write_replaces_the_target_file() {
        let root = std::env::temp_dir().join(format!("vibra-atomic-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("note.txt");
        atomic_write(&path, b"one").unwrap();
        atomic_write(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
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
