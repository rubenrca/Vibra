//! Inbox connectors and explicit remote actions. Credentials never enter commands or prompts.

mod cache;
mod detail;
mod github;
mod linear;
mod review;
pub use cache::ListCache;
pub use detail::{
    github_check_job, load_check_log, load_checks, load_detail, load_diff, load_full_file,
    post_comment, run_pr_action,
};
pub use review::load_review_file;

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::domain::work_items::{WorkItem, WorkKind, WorkSource, WorkStatus};
use crate::infrastructure::process::{CommandLimits, command_output};

pub use linear::{connect_linear, disconnect_linear, linear_connected};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxProject {
    pub id: Uuid,
    pub root: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkQuery {
    pub projects: Vec<InboxProject>,
    pub assigned_to_me: bool,
    pub status: Option<WorkStatus>,
}

#[derive(Default, Serialize, Deserialize)]
pub struct WorkItemsPage {
    pub items: Vec<WorkItem>,
    pub truncated: bool,
    pub connected: bool,
    pub warning: Option<String>,
    pub failed_scopes: Vec<(String, WorkKind)>,
}

pub fn list(source: WorkSource, query: &WorkQuery) -> Result<WorkItemsPage> {
    match source {
        WorkSource::GitHub => github::list(query),
        WorkSource::Linear => linear::list(query),
    }
}

/// Keep long, multiline task descriptions out of the terminal's command line.
/// The shell reads and removes this private file before launching the agent.
pub fn prepare_launch(agent: &str, item: &WorkItem) -> Result<(String, PathBuf)> {
    prepare_prompt_launch(agent, &item.prompt())
}

pub fn prepare_prompt_launch(agent: &str, prompt: &str) -> Result<(String, PathBuf)> {
    use crate::domain::work_items::shell_argument;
    use crate::infrastructure::paths::atomic_write;
    if !matches!(agent, "claude" | "codex" | "gemini") {
        bail!("Invalid agent.");
    }
    let path = std::env::temp_dir().join(format!("vibra-inbox-{}.txt", Uuid::new_v4()));
    atomic_write(&path, prompt.as_bytes()).context("Could not prepare the task context.")?;
    let script = launch_script(agent, &path);
    // zsh guarantees the same expansion even when the project's shell is fish.
    Ok((format!("/bin/zsh -lc {}", shell_argument(&script)), path))
}

fn launch_script(agent: &str, path: &std::path::Path) -> String {
    let path = crate::domain::work_items::shell_argument(&path.to_string_lossy());
    format!("{agent} \"$(/bin/cat {path}; /bin/rm -f {path})\"")
}

const MAX_OUTPUT: u64 = 4 * 1024 * 1024;

/// Drain pipes concurrently and kill the whole process group on timeout.
fn bounded_output(command: &mut Command, input: Option<Vec<u8>>) -> Result<Vec<u8>> {
    let output = command_output(
        command,
        input,
        CommandLimits {
            timeout: Duration::from_secs(25),
            stdout: MAX_OUTPUT as usize,
            stderr: 0,
        },
    )
    .context("Could not complete the Inbox query.")?;
    if !output.status.success() {
        bail!("Could not query the service. Check your connection and provider session.");
    }
    if output.stdout.len() as u64 > MAX_OUTPUT {
        bail!("The response exceeds the size limit.");
    }
    Ok(output.stdout)
}

fn string(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_owned()
}

fn array(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or_default()
}

fn names(value: &Value, key: &str) -> Vec<String> {
    array(value)
        .iter()
        .filter_map(|value| value[key].as_str().map(str::to_owned))
        .collect()
}

fn timestamp(value: &Value) -> u64 {
    value
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|date| date.timestamp().max(0) as u64)
        .unwrap_or_default()
}

fn graphql_data(value: Value) -> Result<Value> {
    if value
        .get("errors")
        .is_some_and(|errors| !errors.as_array().is_some_and(Vec::is_empty))
    {
        bail!("The service rejected the query. Check your account permissions.");
    }
    value
        .get("data")
        .filter(|data| data.is_object())
        .cloned()
        .context("The service returned an invalid response.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn work_inbox_launch_passes_multiline_context_as_one_argument_and_removes_private_file() {
        let mut item = crate::domain::work_items::fixture();
        item.body
            .push_str(&"\nLong description with 'quotes' $(printf injected) `whoami`".repeat(500));
        let (command, path) = prepare_launch("claude", &item).unwrap();
        assert!(!command.contains(['\n', '\r']));
        assert!(command.len() < 1024);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), item.prompt());
        // A function replaces the CLI so this test cannot start a real agent.
        let script = format!(
            "claude() {{ test \"$#\" -eq 1 || return 12; printf '%s' \"$1\"; }}; {}",
            launch_script("claude", &path)
        );
        let output = Command::new("/bin/zsh")
            .args(["-f", "-c", &script])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap(), item.prompt());
        assert!(!path.exists());
        assert!(prepare_launch("not-an-agent", &item).is_err());
    }

    #[test]
    fn work_inbox_subprocess_drains_input_and_rejects_failed_commands() {
        let input = vec![b'x'; 128 * 1024];
        let output = bounded_output(&mut Command::new("/bin/cat"), Some(input.clone())).unwrap();
        assert_eq!(output, input);
        assert!(bounded_output(&mut Command::new("/usr/bin/false"), None).is_err());
    }
}
