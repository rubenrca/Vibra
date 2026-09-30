//! Drafts a commit message with the agent CLI the user already has installed.
//! It runs through the login shell, so it sees the same PATH and credentials
//! as the terminal, and nothing is sent anywhere the user's CLI would not.

use std::path::Path;
use std::time::Duration;

use anyhow::{Result, bail};

use crate::infrastructure::process::{CommandLimits, command_output};

const TIMEOUT: Duration = Duration::from_secs(120);
const MAX_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_MESSAGE_CHARS: usize = 2_000;

const INSTRUCTIONS: &str = "Write a git commit message for the changes below. \
Use the imperative mood and a subject line of at most 72 characters; add a short \
body only when the change needs explaining. Follow the language and style of the \
recent commit subjects. Reply with the commit message only: no quotes, no code \
fences, no preamble.";

/// Tries Claude Code, then Gemini CLI, then Codex, whichever is installed.
const SCRIPT: &str = r#"
if command -v claude >/dev/null 2>&1; then
  exec claude -p "$VIBRA_COMMIT_PROMPT"
elif command -v gemini >/dev/null 2>&1; then
  exec gemini -p "$VIBRA_COMMIT_PROMPT"
elif command -v codex >/dev/null 2>&1; then
  exec codex exec --skip-git-repo-check -
else
  echo "vibra: no agent CLI" >&2
  exit 127
fi
"#;

pub fn generate_commit_message(root: &Path, context: &str) -> Result<String> {
    let mut command = super::login_shell::command(SCRIPT);
    command
        .current_dir(root)
        .env("VIBRA_COMMIT_PROMPT", INSTRUCTIONS);
    let input = format!("{INSTRUCTIONS}\n\n{context}");
    let captured = command_output(
        &mut command,
        Some(input.into_bytes()),
        CommandLimits {
            timeout: TIMEOUT,
            stdout: MAX_OUTPUT_BYTES,
            stderr: MAX_OUTPUT_BYTES,
        },
    )?;
    if captured.stdout.len() > MAX_OUTPUT_BYTES {
        bail!("the agent response exceeds the 64 KiB limit");
    }
    let status = captured.status;
    let output = String::from_utf8(captured.stdout)
        .map_err(|_| anyhow::anyhow!("the agent returned a message that is not UTF-8"))?;
    let errors = String::from_utf8_lossy(&captured.stderr);
    if status.code() == Some(127) && errors.contains("vibra: no agent CLI") {
        bail!("install Claude Code, Gemini CLI, or Codex to generate messages");
    }
    if !status.success() {
        let detail = errors.lines().rev().find(|line| !line.trim().is_empty());
        bail!(
            "the agent failed{}",
            detail
                .map(|line| format!(": {}", line.trim()))
                .unwrap_or_default()
        );
    }
    let message = clean_message(&output);
    if message.is_empty() {
        bail!("the agent did not return a message");
    }
    Ok(message)
}

/// Drops fences, quotes, and "Commit message:" preambles agents add anyway.
pub fn clean_message(output: &str) -> String {
    let lines: Vec<&str> = output
        .lines()
        .filter(|line| !line.trim_start().starts_with("```"))
        .collect();
    let mut text = lines.join("\n").trim().to_owned();
    for prefix in ["Commit message:", "commit message:", "Mensaje de commit:"] {
        if let Some(rest) = text.strip_prefix(prefix) {
            text = rest.trim().to_owned();
        }
    }
    let text = text
        .trim_matches(|character| character == '"' || character == '`')
        .trim();
    text.chars().take(MAX_MESSAGE_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_output_is_reduced_to_the_message() {
        assert_eq!(
            clean_message("```\nCommit message: Add tabs\n\nBody line\n```\n"),
            "Add tabs\n\nBody line"
        );
        assert_eq!(clean_message("\"Fix crash\"\n"), "Fix crash");
        assert_eq!(clean_message("   \n"), "");
    }
}
