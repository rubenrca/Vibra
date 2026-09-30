//! Git process helpers: discovery, bounded reads, and writes.

use std::collections::HashSet;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};

pub(super) const MAX_DIFF_BYTES: usize = 4 * 1024 * 1024;
pub(super) const MAX_GIT_OUTPUT_BYTES: usize = 32 * 1024 * 1024;
pub(super) const MAX_GIT_ERROR_BYTES: usize = 64 * 1024;
pub(super) const DIFF_PLAIN: [&str; 2] = ["--no-ext-diff", "--no-textconv"];
const GIT_WRITE_TIMEOUT: Duration = Duration::from_secs(120);
const GIT_READ_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) fn git_stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

pub(super) fn git_path(root: &Path, name: &str) -> Result<PathBuf> {
    let output = run_git(root, ["rev-parse", "--git-path", name])?;
    ensure_success(&output, "git rev-parse --git-path")?;
    let path = PathBuf::from(OsString::from_vec(
        trim_git_newline(&output.stdout).to_vec(),
    ));
    Ok(if path.is_absolute() {
        path
    } else {
        root.join(path)
    })
}

pub(super) fn run_git_with_index<I, S>(root: &Path, index: &Path, arguments: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut command = git_command();
    command
        // The private index is reused between polls. Always consider ctime,
        // even when a repository has disabled it for its real index, so a
        // same-size edit with restored mtime cannot reuse a stale blob.
        .arg("-c")
        .arg("core.trustctime=true")
        .arg("-c")
        .arg("core.checkStat=default")
        .env("GIT_INDEX_FILE", index)
        .arg("-C")
        .arg(root)
        .args(arguments)
        .stdin(Stdio::null());
    crate::infrastructure::process::command_output(
        &mut command,
        None,
        crate::infrastructure::process::CommandLimits {
            timeout: GIT_WRITE_TIMEOUT,
            stdout: MAX_GIT_OUTPUT_BYTES,
            stderr: MAX_GIT_ERROR_BYTES,
        },
    )
    .with_context(|| format!("failed to run Git in {}", root.display()))
}

fn trim_git_newline(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\n").unwrap_or(bytes)
}

/// Dock-launched macOS apps only get `/usr/bin:/bin:/usr/sbin:/sbin`. Apple's
/// `/usr/bin/git` is an Xcode stub that fails (license, missing CLT) even when
/// Homebrew Git is installed. Prefer a Git that can actually run.
pub(crate) fn git_program() -> &'static Path {
    static GIT: OnceLock<PathBuf> = OnceLock::new();
    GIT.get_or_init(discover_git)
}

fn discover_git() -> PathBuf {
    discover_git_with(
        std::env::var_os("VIBRA_GIT").map(PathBuf::from),
        std::env::var_os("PATH"),
        extra_git_candidates(),
    )
}

pub(super) fn extra_git_candidates() -> Vec<PathBuf> {
    let mut candidates = vec![
        PathBuf::from("/opt/homebrew/bin/git"),
        PathBuf::from("/usr/local/bin/git"),
        PathBuf::from("/usr/local/git/bin/git"),
        PathBuf::from("/opt/local/bin/git"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        candidates.push(home.join(".local/bin/git"));
        candidates.push(home.join("bin/git"));
    }
    candidates
}

pub(super) fn discover_git_with(
    override_path: Option<PathBuf>,
    path: Option<OsString>,
    extras: Vec<PathBuf>,
) -> PathBuf {
    if let Some(path) = override_path
        && git_is_usable(&path)
    {
        return path;
    }

    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    if let Some(path) = path {
        for directory in std::env::split_paths(&path) {
            candidates.push(directory.join("git"));
        }
    }
    candidates.extend(extras);

    for candidate in &candidates {
        if !seen.insert(candidate.clone()) || is_deferred_system_git(candidate) {
            continue;
        }
        if git_is_usable(candidate) {
            return candidate.clone();
        }
    }
    seen.clear();
    for candidate in &candidates {
        if !seen.insert(candidate.clone()) {
            continue;
        }
        if git_is_usable(candidate) {
            return candidate.clone();
        }
    }
    PathBuf::from("git")
}

/// Apple's `/usr/bin/git` is often a license/CLT shim. Try it last so a
/// Homebrew or `/usr/local` Git is used when the GUI PATH hides it.
#[cfg(target_os = "macos")]
fn is_deferred_system_git(path: &Path) -> bool {
    path == Path::new("/usr/bin/git")
}

#[cfg(not(target_os = "macos"))]
fn is_deferred_system_git(_: &Path) -> bool {
    false
}

pub(super) fn git_is_usable(path: &Path) -> bool {
    if path.components().count() > 1 && !path.is_file() {
        return false;
    }
    crate::infrastructure::process::command_output_until_limit(
        Command::new(path).arg("--version"),
        crate::infrastructure::process::CommandLimits {
            timeout: Duration::from_secs(3),
            stdout: 4096,
            stderr: 0,
        },
    )
    .is_ok_and(|output| output.status.success())
}

fn git_command() -> Command {
    let mut command = Command::new(git_program());
    command
        .arg("-c")
        .arg("core.quotepath=false")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C");
    command
}

pub(super) fn is_not_a_repository(stderr: &[u8]) -> bool {
    String::from_utf8_lossy(stderr)
        .to_ascii_lowercase()
        .contains("not a git repository")
}

pub(super) fn repository_root(root: &Path) -> Result<Option<PathBuf>> {
    if !root.is_dir() {
        return Ok(None);
    }
    let output = run_git(root, ["rev-parse", "--show-toplevel"])?;
    if output.status.success() {
        let path = trim_git_newline(&output.stdout);
        return Ok((!path.is_empty()).then(|| PathBuf::from(OsString::from_vec(path.to_vec()))));
    }
    if is_not_a_repository(&output.stderr) {
        return Ok(None);
    }
    let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    bail!("git rev-parse failed: {message}")
}

/// Raw NUL-delimited paths for Quick Open; the filesystem adapter validates entries.
pub(crate) fn search_paths(root: &Path) -> Result<Vec<u8>> {
    let output = run_git_bounded(
        root,
        [
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
        16 * 1024 * 1024,
    )?;
    ensure_success(&output, "git ls-files")?;
    Ok(output.stdout)
}

pub(super) fn run_git<I, S>(root: &Path, arguments: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let output = run_git_bounded(root, arguments, MAX_GIT_OUTPUT_BYTES)?;
    if output.stdout.len() > MAX_GIT_OUTPUT_BYTES {
        bail!("Git output exceeded 32 MiB in {}", root.display());
    }
    Ok(output)
}

pub(super) fn run_git_diff<I, S>(
    root: &Path,
    arguments: I,
    operation: &str,
    allow_difference: bool,
) -> Result<Vec<u8>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let output = run_git_bounded(root, arguments, MAX_DIFF_BYTES)?;
    let reached_limit = output.stdout.len() > MAX_DIFF_BYTES;
    if !(reached_limit
        || output.status.success()
        || allow_difference && output.status.code() == Some(1))
    {
        ensure_success(&output, operation)?;
    }
    Ok(output.stdout)
}

pub(super) fn patch_from_empty(root: &Path, path: &str, operation: &str) -> Result<Vec<u8>> {
    run_git_diff(
        root,
        [
            "--literal-pathspecs",
            "diff",
            "--no-index",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--unified=3",
            "--",
            "/dev/null",
            path,
        ],
        operation,
        true,
    )
}

/// Git diffs stop immediately at their byte budget and retain the extra byte
/// for the parser's truncation notice. Ordinary reads reject that same sentinel.
fn run_git_bounded<I, S>(root: &Path, arguments: I, limit: usize) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut command = git_command();
    command.arg("-C").arg(root).args(arguments);
    crate::infrastructure::process::command_output_until_limit(
        &mut command,
        crate::infrastructure::process::CommandLimits {
            timeout: GIT_READ_TIMEOUT,
            stdout: limit,
            stderr: MAX_GIT_ERROR_BYTES,
        },
    )
    .with_context(|| format!("failed to run Git in {}", root.display()))
}

pub(super) fn ensure_success(output: &Output, operation: &str) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }
    let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    bail!("{operation} failed: {message}")
}

/// Git environment for writes started from the UI: never wait on a terminal.
fn git_write_command(root: &Path) -> Command {
    let mut command = git_command();
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .env("GIT_SEQUENCE_EDITOR", "true")
        .env("GCM_INTERACTIVE", "never")
        .arg("-C")
        .arg(root)
        .stdin(Stdio::null());
    command
}

pub(super) fn index_is_clean(root: &Path) -> bool {
    run_git(
        root,
        [
            "diff",
            "--cached",
            "--quiet",
            "--no-ext-diff",
            "--no-textconv",
        ],
    )
    .is_ok_and(|output| output.status.success())
}

pub(super) fn run_git_write(root: &Path, arguments: &[&str], operation: &str) -> Result<Output> {
    let mut command = git_write_command(root);
    command.args(arguments);
    let output = crate::infrastructure::process::command_output(
        &mut command,
        None,
        crate::infrastructure::process::CommandLimits {
            timeout: GIT_WRITE_TIMEOUT,
            stdout: MAX_GIT_OUTPUT_BYTES,
            stderr: MAX_GIT_ERROR_BYTES,
        },
    )
    .with_context(|| format!("failed to run Git in {}", root.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let message = [stderr.trim(), stdout.trim()]
            .into_iter()
            .find(|text| !text.is_empty())
            .unwrap_or("no details")
            .lines()
            .take(6)
            .collect::<Vec<_>>()
            .join(" ");
        bail!("{operation}: {message}");
    }
    Ok(output)
}
