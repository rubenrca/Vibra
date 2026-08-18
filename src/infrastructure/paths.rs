use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::{BaseDirs, ProjectDirs};

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
pub struct AtomicWriteOptions<'a> {
    pub unix_mode: Option<u32>,
    pub sync: bool,
    pub preserve_permissions_from: Option<&'a Path>,
}

/// Atomically replace `path` with `bytes` via a sibling temp file + rename.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_write_with(path, bytes, AtomicWriteOptions::default())
}

pub fn atomic_write_with(path: &Path, bytes: &[u8], options: AtomicWriteOptions<'_>) -> Result<()> {
    let parent = path
        .parent()
        .context("la ruta de escritura no tiene directorio padre")?;
    fs::create_dir_all(parent).with_context(|| format!("no se pudo crear {}", parent.display()))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("la ruta de escritura no tiene nombre")?;
    let temporary = parent.join(format!(".{name}.{}.tmp", std::process::id()));
    if let Err(error) = write_temp(&temporary, bytes, &options) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
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
    Ok(())
}

fn write_temp(temporary: &Path, bytes: &[u8], options: &AtomicWriteOptions<'_>) -> Result<()> {
    use std::io::Write;

    let mut output = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(temporary)
        .with_context(|| format!("no se pudo crear {}", temporary.display()))?;
    if let Some(source) = options.preserve_permissions_from {
        fs::set_permissions(temporary, fs::metadata(source)?.permissions())?;
    } else if let Some(mode) = options.unix_mode {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(temporary, fs::Permissions::from_mode(mode))?;
        }
        #[cfg(not(unix))]
        let _ = mode;
    }
    output.write_all(bytes)?;
    if options.sync {
        output.sync_all()?;
    }
    Ok(())
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
}
