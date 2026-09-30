use std::fs::OpenOptions;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context as _, Result, bail};

pub(super) const MAX_CAPTURE_INDEXES: usize = 8;

pub(super) struct CachedCaptureIndex {
    pub(super) temporary: TemporaryIndex,
    pub(super) source_index: Option<IndexFingerprint>,
    pub(super) untracked_paths: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct IndexFingerprint {
    device: u64,
    inode: u64,
    length: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

impl IndexFingerprint {
    pub(super) fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            length: metadata.len(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanoseconds: metadata.ctime_nsec(),
        }
    }
}

pub(super) fn index_fingerprint(path: &Path) -> Result<Option<IndexFingerprint>> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", path.display()));
        }
    };
    Ok(Some(IndexFingerprint::from_metadata(&metadata)))
}

pub(super) fn copy_source_index(
    source: &Path,
    destination: &Path,
    expected: IndexFingerprint,
) -> Result<()> {
    let mut input = std::fs::File::open(source)
        .with_context(|| format!("failed to open {}", source.display()))?;
    if IndexFingerprint::from_metadata(&input.metadata()?) != expected {
        bail!("{} changed while preparing the capture", source.display());
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(destination)
        .with_context(|| format!("failed to create {}", destination.display()))?;
    std::io::copy(&mut input, &mut output)
        .with_context(|| format!("failed to copy {}", source.display()))?;
    if IndexFingerprint::from_metadata(&input.metadata()?) != expected
        || index_fingerprint(source)? != Some(expected)
    {
        bail!(
            "{} changed while copying the capture index",
            source.display()
        );
    }
    output.sync_all()?;
    Ok(())
}

pub(super) struct CachedCaptureSlot {
    pub(super) index: Arc<Mutex<Option<CachedCaptureIndex>>>,
    pub(super) used_at: Instant,
}

pub(super) struct TemporaryIndex(pub(super) PathBuf);

impl Drop for TemporaryIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(self.0.with_extension("lock"));
    }
}
