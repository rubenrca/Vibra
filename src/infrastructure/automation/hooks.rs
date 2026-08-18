use anyhow::{Context, Result, anyhow, bail};
use directories::BaseDirs;
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::*;

const CLAUDE_HOOK_SCRIPT: &str = r#"#!/bin/sh
# Managed by Vibra. This is deliberately a no-op outside a Vibra pane.
[ -n "$VIBRA_CLI" ] && [ -n "$VIBRA_AUTOMATION_SOCKET" ] && [ -n "$VIBRA_PANE_ID" ] || exit 0
"$VIBRA_CLI" +agent hook claude "$1" >/dev/null 2>&1 || true
exit 0
"#;

const CODEX_HOOK_SCRIPT: &str = r#"#!/bin/sh
# Managed by Vibra. Never fail a Codex hook: PermissionRequest treats errors as policy input.
[ -n "$VIBRA_CLI" ] && [ -n "$VIBRA_AUTOMATION_SOCKET" ] && [ -n "$VIBRA_PANE_ID" ] || exit 0
"$VIBRA_CLI" +agent hook codex "$1" >/dev/null 2>&1 || true
exit 0
"#;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AgentHookStatus {
    pub claude_installed: bool,
    pub codex_installed: bool,
}

impl AgentHookStatus {
    pub const fn any_installed(self) -> bool {
        self.claude_installed || self.codex_installed
    }

    pub const fn all_installed(self) -> bool {
        self.claude_installed && self.codex_installed
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum AgentHookOperation {
    Install,
    Status,
    Uninstall,
}

pub fn agent_hook_status() -> Result<AgentHookStatus> {
    manage_current_user_agent_hooks(AgentHookOperation::Status)
}

pub fn install_agent_hooks() -> Result<AgentHookStatus> {
    manage_current_user_agent_hooks(AgentHookOperation::Install)
}

pub fn uninstall_agent_hooks() -> Result<AgentHookStatus> {
    manage_current_user_agent_hooks(AgentHookOperation::Uninstall)
}

fn manage_current_user_agent_hooks(operation: AgentHookOperation) -> Result<AgentHookStatus> {
    let home = BaseDirs::new()
        .map(|directories| directories.home_dir().to_path_buf())
        .context("no se pudo resolver el directorio de usuario")?;
    let selected = [AgentKind::Claude, AgentKind::Codex].into_iter().collect();
    let report = manage_agent_hooks(&home, &selected, operation, false)?;
    let report = if operation == AgentHookOperation::Status {
        report
    } else {
        manage_agent_hooks(&home, &selected, AgentHookOperation::Status, false)?
    };
    Ok(agent_hook_status_from_report(&report))
}

pub(super) fn agent_hook_status_from_report(report: &Value) -> AgentHookStatus {
    let installed = |agent| {
        report["agents"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|entry| entry["agent"] == agent)
            .and_then(|entry| entry["installed"].as_bool())
            .unwrap_or(false)
    };
    AgentHookStatus {
        claude_installed: installed("Claude"),
        codex_installed: installed("Codex"),
    }
}

pub(super) fn run_agent_setup_cli(arguments: &[String]) -> Result<()> {
    let operation = match arguments.first().map(String::as_str) {
        Some("setup") | None => AgentHookOperation::Install,
        Some("status") => AgentHookOperation::Status,
        Some("uninstall") => AgentHookOperation::Uninstall,
        _ => bail!("uso: agent [setup|status|uninstall] [claude|codex|all] [--dry-run]"),
    };
    let dry_run = arguments.iter().any(|argument| argument == "--dry-run");
    if dry_run && operation != AgentHookOperation::Install {
        bail!("--dry-run solo se puede usar con agent setup");
    }
    let selected: HashSet<_> = arguments
        .iter()
        .filter_map(|argument| AgentKind::parse(argument))
        .filter(|kind| matches!(kind, AgentKind::Claude | AgentKind::Codex))
        .collect();
    let selected = if selected.is_empty() {
        [AgentKind::Claude, AgentKind::Codex].into_iter().collect()
    } else {
        selected
    };
    let home = BaseDirs::new()
        .map(|directories| directories.home_dir().to_path_buf())
        .context("no se pudo resolver el directorio de usuario")?;
    let report = manage_agent_hooks(&home, &selected, operation, dry_run)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

pub(super) fn manage_agent_hooks(
    home: &Path,
    selected: &HashSet<AgentKind>,
    operation: AgentHookOperation,
    dry_run: bool,
) -> Result<Value> {
    let managed_directory = home.join(".vibra").join("agent-hooks");
    let mut reports = Vec::new();
    for kind in [AgentKind::Claude, AgentKind::Codex] {
        if !selected.contains(&kind) {
            continue;
        }
        let (config_path, script_name, script, entries) = match kind {
            AgentKind::Claude => (
                home.join(".claude").join("settings.json"),
                "vibra-claude.sh",
                CLAUDE_HOOK_SCRIPT,
                vec![
                    ("SessionStart", Some(""), "session-start"),
                    ("UserPromptSubmit", None, "prompt"),
                    ("Stop", None, "stop"),
                    ("PermissionRequest", None, "permission"),
                    ("SessionEnd", Some(""), "session-end"),
                    (
                        "Notification",
                        Some("idle_prompt|permission_prompt"),
                        "notification",
                    ),
                ],
            ),
            AgentKind::Codex => (
                home.join(".codex").join("hooks.json"),
                "vibra-codex.sh",
                CODEX_HOOK_SCRIPT,
                vec![
                    (
                        "SessionStart",
                        Some("startup|resume|clear|compact"),
                        "session-start",
                    ),
                    ("UserPromptSubmit", None, "prompt"),
                    ("Stop", None, "stop"),
                    ("PermissionRequest", None, "permission"),
                    ("SessionEnd", None, "session-end"),
                ],
            ),
            _ => unreachable!(),
        };
        let script_path = managed_directory.join(script_name);
        let script_command = shell_quote(&script_path.to_string_lossy());
        let commands: Vec<_> = entries
            .iter()
            .map(|(_, _, event)| format!("{script_command} {event}"))
            .collect();
        let installed = hooks_installed(&config_path, &entries, &commands, &script_path, script)?;
        if operation == AgentHookOperation::Status {
            reports.push(serde_json::json!({
                "agent": kind.display_name(),
                "installed": installed,
                "config": config_path,
                "script": script_path,
            }));
            continue;
        }

        let changed = match operation {
            AgentHookOperation::Install => {
                let mut config = read_hook_config(&config_path)?;
                let mut config_changed = false;
                for ((slot, matcher, _), command) in entries.iter().zip(&commands) {
                    config_changed |= ensure_hook_entry(&mut config, slot, *matcher, command)?;
                }
                let script_needs_write = script_changed(&script_path, script)?;
                if !dry_run {
                    if config_changed {
                        backup_if_exists(&config_path)?;
                        write_json_atomically(&config_path, &config)?;
                    }
                    if script_needs_write {
                        write_script_atomically(&script_path, script)?;
                    }
                }
                config_changed || script_needs_write
            }
            AgentHookOperation::Uninstall => {
                let mut config = read_hook_config(&config_path)?;
                let config_changed = remove_hook_entries(&mut config, &script_path)?;
                if !dry_run && config_changed {
                    backup_if_exists(&config_path)?;
                    write_json_atomically(&config_path, &config)?;
                }
                let script_removed = if !dry_run && script_path.exists() {
                    fs::remove_file(&script_path)?;
                    true
                } else {
                    script_path.exists()
                };
                config_changed || script_removed
            }
            AgentHookOperation::Status => false,
        };
        reports.push(serde_json::json!({
            "agent": kind.display_name(),
            "operation": match operation {
                AgentHookOperation::Install => if dry_run { "dry-run" } else { "setup" },
                AgentHookOperation::Uninstall => "uninstall",
                AgentHookOperation::Status => "status",
            },
            "changed": changed,
            "config": config_path,
            "script": script_path,
        }));
    }
    Ok(serde_json::json!({ "agents": reports }))
}

fn read_hook_config(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(serde_json::json!({}));
    }
    let content =
        fs::read_to_string(path).with_context(|| format!("no se pudo leer {}", path.display()))?;
    let value: Value = serde_json::from_str(&content)
        .with_context(|| format!("{} no contiene JSON válido", path.display()))?;
    value
        .is_object()
        .then_some(value)
        .ok_or_else(|| anyhow!("{} debe contener un objeto JSON", path.display()))
}

pub(super) fn ensure_hook_entry(
    config: &mut Value,
    slot: &str,
    matcher: Option<&str>,
    command: &str,
) -> Result<bool> {
    let root = config
        .as_object_mut()
        .context("configuración JSON inválida")?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .context("hooks debe contener un objeto")?;
    let groups = hooks
        .entry(slot)
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .with_context(|| format!("hooks.{slot} debe contener una lista"))?;
    let handler = serde_json::json!({
        "type": "command",
        "command": command,
        "timeout": 3,
    });
    let mut found_correct_group = false;
    let mut changed = false;
    for group in groups.iter_mut() {
        let matcher_matches = match matcher {
            Some(matcher) => group.get("matcher").and_then(Value::as_str) == Some(matcher),
            None => group.get("matcher").is_none(),
        };
        let Some(handlers) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
            continue;
        };
        handlers.retain_mut(|existing| {
            if existing.get("command").and_then(Value::as_str) != Some(command) {
                return true;
            }
            if matcher_matches && !found_correct_group {
                changed |= *existing != handler;
                *existing = handler.clone();
                found_correct_group = true;
                true
            } else {
                // Move managed handlers out of groups with a different matcher
                // without changing the behavior of neighboring user hooks.
                changed = true;
                false
            }
        });
    }
    groups.retain(|group| {
        group
            .get("hooks")
            .and_then(Value::as_array)
            .is_none_or(|handlers| !handlers.is_empty())
    });
    if found_correct_group {
        return Ok(changed);
    }
    let mut group = serde_json::json!({ "hooks": [handler] });
    if let Some(matcher) = matcher {
        group["matcher"] = Value::String(matcher.to_owned());
    }
    groups.push(group);
    Ok(true)
}

fn remove_hook_entries(config: &mut Value, script_path: &Path) -> Result<bool> {
    let Some(hooks) = config.get_mut("hooks").and_then(Value::as_object_mut) else {
        return Ok(false);
    };
    let raw_script_path = script_path.to_string_lossy();
    let quoted_script_path = shell_quote(&raw_script_path);
    let mut changed = false;
    hooks.retain(|_, groups| {
        let Some(groups) = groups.as_array_mut() else {
            return true;
        };
        groups.retain_mut(|group| {
            let Some(handlers) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                return true;
            };
            let before = handlers.len();
            handlers.retain(|handler| {
                !handler
                    .get("command")
                    .and_then(Value::as_str)
                    .is_some_and(|command| {
                        managed_hook_command_matches(
                            command,
                            raw_script_path.as_ref(),
                            &quoted_script_path,
                        )
                    })
            });
            changed |= handlers.len() != before;
            !handlers.is_empty()
        });
        !groups.is_empty()
    });
    Ok(changed)
}

fn managed_hook_command_matches(command: &str, raw_path: &str, quoted_path: &str) -> bool {
    [raw_path, quoted_path].iter().any(|prefix| {
        command
            .strip_prefix(prefix)
            .is_some_and(|suffix| suffix.chars().next().is_some_and(char::is_whitespace))
    })
}

fn hooks_installed(
    path: &Path,
    entries: &[(&str, Option<&str>, &str)],
    commands: &[String],
    script_path: &Path,
    script: &str,
) -> Result<bool> {
    let script_ready = fs::read_to_string(script_path).ok().as_deref() == Some(script)
        && fs::metadata(script_path)
            .ok()
            .is_some_and(|metadata| metadata.permissions().mode() & 0o111 != 0);
    if !script_ready {
        return Ok(false);
    }
    let config = read_hook_config(path)?;
    Ok(entries
        .iter()
        .zip(commands)
        .all(|((slot, matcher, _), command)| {
            config["hooks"]
                .get(*slot)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .any(|group| {
                    let matcher_matches = match matcher {
                        Some(matcher) => {
                            group.get("matcher").and_then(Value::as_str) == Some(*matcher)
                        }
                        None => group.get("matcher").is_none(),
                    };
                    matcher_matches
                        && group
                            .get("hooks")
                            .and_then(Value::as_array)
                            .is_some_and(|handlers| {
                                handlers.iter().any(|handler| {
                                    handler.get("type").and_then(Value::as_str) == Some("command")
                                        && handler.get("command").and_then(Value::as_str)
                                            == Some(command)
                                        && handler.get("timeout").and_then(Value::as_u64) == Some(3)
                                        && !handler
                                            .get("async")
                                            .and_then(Value::as_bool)
                                            .unwrap_or(false)
                                })
                            })
                })
        }))
}

fn script_changed(path: &Path, script: &str) -> Result<bool> {
    Ok(!path.exists() || fs::read_to_string(path).ok().as_deref() != Some(script))
}

fn backup_if_exists(path: &Path) -> Result<()> {
    if path.exists() {
        fs::copy(
            path,
            PathBuf::from(format!("{}.vibra-backup", path.display())),
        )?;
    }
    Ok(())
}

fn write_json_atomically(path: &Path, value: &Value) -> Result<()> {
    write_text_atomically(
        path,
        &format!("{}\n", serde_json::to_string_pretty(value)?),
        0o600,
    )
}

fn write_script_atomically(path: &Path, script: &str) -> Result<()> {
    write_text_atomically(path, script, 0o700)
}

pub(super) fn write_text_atomically(path: &Path, text: &str, mode: u32) -> Result<()> {
    use crate::infrastructure::paths::{AtomicWriteOptions, atomic_write_with};
    let parent = path
        .parent()
        .context("ruta de configuración sin directorio padre")?;
    let parent_exists = parent.exists();
    fs::create_dir_all(parent)?;
    if !parent_exists {
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    atomic_write_with(
        path,
        text.as_bytes(),
        AtomicWriteOptions {
            unix_mode: Some(mode),
            sync: false,
            preserve_permissions_from: None,
        },
    )
}
