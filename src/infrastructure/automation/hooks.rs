use anyhow::{Context, Result, anyhow, bail};
use directories::BaseDirs;
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use super::cli::shell_quote;
use super::types::AgentKind;

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

#[derive(Clone, Copy)]
struct HookEntry {
    slot: &'static str,
    matcher: Option<&'static str>,
    event: &'static str,
}

impl HookEntry {
    const fn new(slot: &'static str, matcher: Option<&'static str>, event: &'static str) -> Self {
        Self {
            slot,
            matcher,
            event,
        }
    }
}

const CLAUDE_HOOKS: &[HookEntry] = &[
    HookEntry::new("SessionStart", Some(""), "session-start"),
    HookEntry::new("UserPromptSubmit", None, "prompt"),
    HookEntry::new("Stop", None, "stop"),
    HookEntry::new("PermissionRequest", None, "permission"),
    HookEntry::new("SessionEnd", Some(""), "session-end"),
    HookEntry::new(
        "Notification",
        Some("idle_prompt|permission_prompt"),
        "notification",
    ),
];

const CODEX_HOOKS: &[HookEntry] = &[
    HookEntry::new(
        "SessionStart",
        Some("startup|resume|clear|compact"),
        "session-start",
    ),
    HookEntry::new("UserPromptSubmit", None, "prompt"),
    HookEntry::new("Stop", None, "stop"),
    HookEntry::new("PermissionRequest", None, "permission"),
    HookEntry::new("SessionEnd", None, "session-end"),
];

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
    let (operation, selected, dry_run) = parse_agent_setup_arguments(arguments)?;
    let home = BaseDirs::new()
        .map(|directories| directories.home_dir().to_path_buf())
        .context("no se pudo resolver el directorio de usuario")?;
    let report = manage_agent_hooks(&home, &selected, operation, dry_run)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn parse_agent_setup_arguments(
    arguments: &[String],
) -> Result<(AgentHookOperation, HashSet<AgentKind>, bool)> {
    let operation = match arguments.first().map(String::as_str) {
        Some("setup") | None => AgentHookOperation::Install,
        Some("status") => AgentHookOperation::Status,
        Some("uninstall") => AgentHookOperation::Uninstall,
        _ => bail!("uso: agent [setup|status|uninstall] [claude|codex|all] [--dry-run]"),
    };
    let mut dry_run = false;
    let mut all = false;
    let mut selected = HashSet::new();
    for argument in arguments.iter().skip(1) {
        match argument.as_str() {
            "--dry-run" if !dry_run => dry_run = true,
            "all" if !all => all = true,
            _ => {
                let Some(kind @ (AgentKind::Claude | AgentKind::Codex)) =
                    AgentKind::parse(argument)
                else {
                    bail!("agente u opción no reconocido: {argument}");
                };
                if !selected.insert(kind) {
                    bail!("agente repetido: {argument}");
                }
            }
        }
    }
    if dry_run && operation != AgentHookOperation::Install {
        bail!("--dry-run solo se puede usar con agent setup");
    }
    if all && !selected.is_empty() {
        bail!("all no se puede combinar con nombres de agente");
    }
    if all || selected.is_empty() {
        selected.extend([AgentKind::Claude, AgentKind::Codex]);
    }
    Ok((operation, selected, dry_run))
}

pub(super) fn manage_agent_hooks(
    home: &Path,
    selected: &HashSet<AgentKind>,
    operation: AgentHookOperation,
    dry_run: bool,
) -> Result<Value> {
    let managed_directory = home.join(".vibra").join("agent-hooks");
    let directory_secure = managed_hooks_directory_secure(home)?;
    let selected_agents: Vec<_> = [AgentKind::Claude, AgentKind::Codex]
        .into_iter()
        .filter(|kind| selected.contains(kind))
        .collect();
    if !dry_run && operation != AgentHookOperation::Status {
        // Validate every provider before changing the first one. A malformed
        // second configuration must not leave the first integration changed.
        for &kind in &selected_agents {
            manage_one_agent_hooks(
                home,
                &managed_directory,
                directory_secure,
                kind,
                operation,
                true,
            )
            .map_err(|error| {
                anyhow!(
                    "no se pudo preparar hooks de {}: {error:#}",
                    kind.display_name()
                )
            })?;
        }
        prepare_managed_hooks_directory(home, operation == AgentHookOperation::Install)?;
    }
    let mut reports = Vec::new();
    for kind in selected_agents {
        let report = manage_one_agent_hooks(
            home,
            &managed_directory,
            directory_secure,
            kind,
            operation,
            dry_run,
        )
        .map_err(|error| {
            let action = match operation {
                AgentHookOperation::Install => "instalar",
                AgentHookOperation::Status => "consultar",
                AgentHookOperation::Uninstall => "desinstalar",
            };
            anyhow!(
                "no se pudo {action} hooks de {} ({} integraciones anteriores completadas): {error:#}",
                kind.display_name(),
                reports.len(),
            )
        })?;
        reports.push(report);
    }
    Ok(serde_json::json!({ "agents": reports }))
}

fn manage_one_agent_hooks(
    home: &Path,
    managed_directory: &Path,
    directory_secure: bool,
    kind: AgentKind,
    operation: AgentHookOperation,
    dry_run: bool,
) -> Result<Value> {
    let (config_path, script_name, script, entries) = match kind {
        AgentKind::Claude => (
            home.join(".claude").join("settings.json"),
            "vibra-claude.sh",
            CLAUDE_HOOK_SCRIPT,
            CLAUDE_HOOKS,
        ),
        AgentKind::Codex => (
            home.join(".codex").join("hooks.json"),
            "vibra-codex.sh",
            CODEX_HOOK_SCRIPT,
            CODEX_HOOKS,
        ),
        _ => unreachable!(),
    };
    let script_path = managed_directory.join(script_name);
    let script_command = shell_quote(&script_path.to_string_lossy());
    let commands: Vec<_> = entries
        .iter()
        .map(|entry| format!("{script_command} {}", entry.event))
        .collect();
    let installed = directory_secure
        && hooks_installed(&config_path, entries, &commands, &script_path, script)?;
    if operation == AgentHookOperation::Status {
        return Ok(serde_json::json!({
            "agent": kind.display_name(),
            "installed": installed,
            "config": config_path,
            "script": script_path,
        }));
    }

    let changed = match operation {
        AgentHookOperation::Install => {
            let mut config = read_hook_config(&config_path)?;
            let mut config_changed = false;
            for (entry, command) in entries.iter().zip(&commands) {
                config_changed |=
                    ensure_hook_entry(&mut config, entry.slot, entry.matcher, command)?;
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
            config_changed || script_needs_write || !directory_secure
        }
        AgentHookOperation::Uninstall => {
            let mut config = read_hook_config(&config_path)?;
            let config_changed = remove_hook_entries(&mut config, &script_path)?;
            if !dry_run && config_changed {
                backup_if_exists(&config_path)?;
                write_json_atomically(&config_path, &config)?;
            }
            let script_exists = match fs::symlink_metadata(&script_path) {
                Ok(_) => true,
                Err(error) if error.kind() == ErrorKind::NotFound => false,
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("no se pudo inspeccionar {}", script_path.display())
                    });
                }
            };
            let script_removed = if !dry_run && script_exists {
                fs::remove_file(&script_path)?;
                true
            } else {
                script_exists
            };
            config_changed || script_removed
        }
        AgentHookOperation::Status => false,
    };
    Ok(serde_json::json!({
        "agent": kind.display_name(),
        "operation": match operation {
            AgentHookOperation::Install => if dry_run { "dry-run" } else { "setup" },
            AgentHookOperation::Uninstall => "uninstall",
            AgentHookOperation::Status => "status",
        },
        "changed": changed,
        "config": config_path,
        "script": script_path,
    }))
}

/// Scripts run as the user. Reject path aliases and foreign owners, then keep
/// both directory components private before creating executable files.
fn managed_hooks_directory_secure(home: &Path) -> Result<bool> {
    let mut secure = true;
    for directory in [home.join(".vibra"), home.join(".vibra/agent-hooks")] {
        match fs::symlink_metadata(&directory) {
            Ok(metadata) => {
                if !metadata.file_type().is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
                    bail!(
                        "{} no es un directorio propio y seguro",
                        directory.display()
                    );
                }
                secure &= metadata.permissions().mode() & 0o077 == 0;
            }
            Err(error) if error.kind() == ErrorKind::NotFound => secure = false,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("no se pudo inspeccionar {}", directory.display()));
            }
        }
    }
    Ok(secure)
}

fn prepare_managed_hooks_directory(home: &Path, create: bool) -> Result<()> {
    for directory in [home.join(".vibra"), home.join(".vibra/agent-hooks")] {
        match fs::symlink_metadata(&directory) {
            Ok(metadata) => {
                if !metadata.file_type().is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
                    bail!(
                        "{} no es un directorio propio y seguro",
                        directory.display()
                    );
                }
                fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
            }
            Err(error) if error.kind() == ErrorKind::NotFound && create => {
                fs::create_dir(&directory)
                    .with_context(|| format!("no se pudo crear {}", directory.display()))?;
                fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
            }
            Err(error) if error.kind() == ErrorKind::NotFound => break,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("no se pudo inspeccionar {}", directory.display()));
            }
        }
    }
    Ok(())
}

fn read_hook_config(path: &Path) -> Result<Value> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(serde_json::json!({}));
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("no se pudo inspeccionar {}", path.display()));
        }
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
        let matcher_matches = hook_matcher_matches(group, matcher);
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

fn hook_matcher_matches(group: &Value, matcher: Option<&str>) -> bool {
    match matcher {
        Some(matcher) => group.get("matcher").and_then(Value::as_str) == Some(matcher),
        None => group.get("matcher").is_none(),
    }
}

fn hook_group_contains_handler(group: &Value, matcher: Option<&str>, command: &str) -> bool {
    hook_matcher_matches(group, matcher)
        && group
            .get("hooks")
            .and_then(Value::as_array)
            .is_some_and(|handlers| {
                handlers.iter().any(|handler| {
                    handler.get("type").and_then(Value::as_str) == Some("command")
                        && handler.get("command").and_then(Value::as_str) == Some(command)
                        && handler.get("timeout").and_then(Value::as_u64) == Some(3)
                        && !handler
                            .get("async")
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                })
            })
}

fn hooks_installed(
    path: &Path,
    entries: &[HookEntry],
    commands: &[String],
    script_path: &Path,
    script: &str,
) -> Result<bool> {
    let script_ready = fs::symlink_metadata(script_path)
        .ok()
        .is_some_and(|metadata| {
            metadata.file_type().is_file()
                && metadata.uid() == unsafe { libc::geteuid() }
                && metadata.permissions().mode() & 0o777 == 0o700
        })
        && fs::read_to_string(script_path).ok().as_deref() == Some(script);
    if !script_ready {
        return Ok(false);
    }
    let config = read_hook_config(path)?;
    Ok(entries.iter().zip(commands).all(|(entry, command)| {
        config["hooks"]
            .get(entry.slot)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .any(|group| hook_group_contains_handler(group, entry.matcher, command))
    }))
}

fn script_changed(path: &Path, script: &str) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_file() => {
            bail!("{} no es un archivo regular", path.display());
        }
        Ok(metadata) => Ok(metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o777 != 0o700
            || fs::read_to_string(path).ok().as_deref() != Some(script)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(true),
        Err(error) => {
            Err(error).with_context(|| format!("no se pudo inspeccionar {}", path.display()))
        }
    }
}

fn backup_if_exists(path: &Path) -> Result<()> {
    use crate::infrastructure::paths::{AtomicWriteOptions, atomic_write_with};

    let data = match fs::read(path) {
        Ok(data) => data,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("no se pudo leer {}", path.display()));
        }
    };
    let mut backup_name = path.as_os_str().to_os_string();
    backup_name.push(".vibra-backup");
    atomic_write_with(
        &PathBuf::from(backup_name),
        &data,
        AtomicWriteOptions {
            unix_mode: Some(0o600),
        },
    )
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
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_parser_selects_only_the_requested_agents() {
        let (_, selected, dry_run) =
            parse_agent_setup_arguments(&["setup".into(), "codex".into(), "--dry-run".into()])
                .unwrap();
        assert_eq!(selected, [AgentKind::Codex].into_iter().collect());
        assert!(dry_run);

        let (_, selected, dry_run) = parse_agent_setup_arguments(&["status".into()]).unwrap();
        assert_eq!(
            selected,
            [AgentKind::Claude, AgentKind::Codex].into_iter().collect()
        );
        assert!(!dry_run);
    }
}
