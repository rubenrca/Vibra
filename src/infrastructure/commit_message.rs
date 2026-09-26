//! Drafts a commit message with the agent CLI the user already has installed.
//! It runs through the login shell, so it sees the same PATH and credentials
//! as the terminal, and nothing is sent anywhere the user's CLI would not.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

const TIMEOUT: Duration = Duration::from_secs(120);
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
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(|| "/bin/zsh".to_owned());
    let mut child = Command::new(shell)
        .args(["-l", "-c", SCRIPT])
        .current_dir(root)
        .env("VIBRA_COMMIT_PROMPT", INSTRUCTIONS)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("no se pudo abrir la shell")?;
    let mut stdin = child.stdin.take().context("sin stdin")?;
    let input = format!("{INSTRUCTIONS}\n\n{context}");
    let writer = thread::spawn(move || {
        let _ = stdin.write_all(input.as_bytes());
    });
    let mut stdout = child.stdout.take().context("sin stdout")?;
    let mut stderr = child.stderr.take().context("sin stderr")?;
    let reader = thread::spawn(move || {
        let mut output = String::new();
        let _ = stdout.read_to_string(&mut output);
        output
    });
    let error_reader = thread::spawn(move || {
        let mut output = String::new();
        let _ = stderr.read_to_string(&mut output);
        output
    });

    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            bail!("el agente no respondió en {} s", TIMEOUT.as_secs());
        }
        thread::sleep(Duration::from_millis(100));
    };
    let _ = writer.join();
    let output = reader.join().unwrap_or_default();
    let errors = error_reader.join().unwrap_or_default();
    if status.code() == Some(127) && errors.contains("vibra: no agent CLI") {
        bail!("instala Claude Code, Gemini CLI o Codex para generar mensajes");
    }
    if !status.success() {
        let detail = errors.lines().rev().find(|line| !line.trim().is_empty());
        bail!(
            "el agente falló{}",
            detail
                .map(|line| format!(": {}", line.trim()))
                .unwrap_or_default()
        );
    }
    let message = clean_message(&output);
    if message.is_empty() {
        bail!("el agente no devolvió un mensaje");
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
