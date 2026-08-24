use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use super::*;

pub fn run_cli(arguments: &[String]) -> Result<bool> {
    let Some(mode) = arguments.first().map(String::as_str) else {
        return Ok(false);
    };
    if mode == "agent" {
        run_agent_setup_cli(&arguments[1..])?;
        return Ok(true);
    }
    if mode == "+skill" {
        print!("{AUTOMATION_SKILL}");
        return Ok(true);
    }
    if !matches!(mode, "+pane" | "+agent") {
        return Ok(false);
    }
    let command = parse_cli_command(mode, &arguments[1..])?;
    if matches!(command, AutomationCommand::AgentKinds) {
        println!("{}", serde_json::to_string_pretty(&agent_kinds_payload())?);
        return Ok(true);
    }
    let socket = std::env::var_os("VIBRA_AUTOMATION_SOCKET")
        .map(PathBuf::from)
        .context("VIBRA_AUTOMATION_SOCKET no está disponible; ejecuta esto dentro de Vibra")?;
    let pane_id = std::env::var("VIBRA_PANE_ID")
        .context("falta VIBRA_PANE_ID")?
        .parse()?;
    let token = std::env::var("VIBRA_AUTOMATION_TOKEN")
        .context("falta VIBRA_AUTOMATION_TOKEN")?
        .parse()?;
    let envelope = AutomationEnvelope {
        pane_id,
        token,
        command,
    };
    let mut stream = UnixStream::connect(&socket)
        .with_context(|| format!("no se pudo conectar a {}", socket.display()))?;
    stream.write_all(&serde_json::to_vec(&envelope)?)?;
    stream.shutdown(std::net::Shutdown::Write)?;
    let mut response = Vec::new();
    stream
        .take(MAX_AUTOMATION_RESPONSE_BYTES + 1)
        .read_to_end(&mut response)?;
    if response.len() as u64 > MAX_AUTOMATION_RESPONSE_BYTES {
        bail!("la respuesta de automatización supera 4 MiB");
    }
    let response: AutomationResponse = serde_json::from_slice(&response)?;
    if !response.ok {
        bail!(
            response
                .error
                .unwrap_or_else(|| "automatización falló".into())
        );
    }
    if let Some(data) = response.data {
        if let Some(text) = data.as_str() {
            println!("{text}");
        } else {
            println!("{}", serde_json::to_string_pretty(&data)?);
        }
    }
    Ok(true)
}

pub fn agent_launch_command(kind: AgentKind, args: &[String]) -> String {
    let mut parts = Vec::with_capacity(args.len() + 1);
    parts.push(shell_quote(kind.executable()));
    for arg in args {
        parts.push(shell_quote(arg));
    }
    parts.join(" ")
}

pub fn agent_kinds_payload() -> Value {
    let kinds: Vec<_> = AgentKind::ALL
        .iter()
        .map(|kind| {
            serde_json::json!({
                "kind": kind.cli_name(),
                "executable": kind.executable(),
                "displayName": kind.display_name(),
                "capabilities": {
                    "launch": true,
                    "detect": true,
                    "prompt": true,
                    "activityTracking": kind.activity_tracking(),
                    "managedHooks": kind.supports_managed_hooks(),
                    "reliableWaitRequiresHooks": true,
                },
            })
        })
        .collect();
    serde_json::json!({ "kinds": kinds })
}

pub(super) fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".into();
    }
    if value.chars().all(|ch| {
        ch.is_ascii_alphanumeric()
            || matches!(ch, '-' | '_' | '.' | '/' | '=' | ':' | '@' | '+' | ',')
    }) {
        return value.to_owned();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(super) fn parse_cli_command(mode: &str, arguments: &[String]) -> Result<AutomationCommand> {
    let operation = arguments.first().map(String::as_str).unwrap_or("status");
    if mode == "+agent" {
        return match operation {
            "status" => Ok(AutomationCommand::AgentStatus {
                target: parse_agent_target(&arguments[1..])?,
            }),
            "list" => Ok(AutomationCommand::AgentList),
            "kinds" => Ok(AutomationCommand::AgentKinds),
            "open" => parse_agent_open(&arguments[1..]),
            "start" => parse_agent_start(&arguments[1..]),
            "prompt" => parse_agent_prompt(&arguments[1..]),
            "wait" => parse_agent_wait(&arguments[1..]),
            "read" => parse_agent_read(&arguments[1..]),
            "rename" => parse_agent_rename(&arguments[1..]),
            "presence" => parse_agent_presence(&arguments[1..]),
            "attention" => parse_agent_attention(&arguments[1..]),
            "hook" => parse_agent_hook(&arguments[1..]),
            "clear" => Ok(AutomationCommand::ClearAgentPresence {
                session_id: parse_agent_flag(&arguments[1..], "--session")?,
            }),
            "idle" => Ok(AutomationCommand::SetAgentState {
                state: AgentRuntimeState::Idle,
            }),
            "working" => Ok(AutomationCommand::SetAgentState {
                state: AgentRuntimeState::Working,
            }),
            "waiting" => Ok(AutomationCommand::SetAgentState {
                state: AgentRuntimeState::Waiting,
            }),
            _ => bail!(
                "uso: +agent [status|list|kinds|open|start|prompt|wait|read|rename|idle|working|waiting|presence|attention|hook|clear]"
            ),
        };
    }
    match operation {
        "list" | "status" => Ok(AutomationCommand::List),
        "send" | "run" => parse_pane_send_or_run(operation, &arguments[1..]),
        "split" => parse_pane_split(&arguments[1..]),
        "tab" => parse_pane_tab(&arguments[1..]),
        "focus" => {
            let direction = parse_direction(arguments.get(1).map(String::as_str))?;
            Ok(AutomationCommand::Focus { direction })
        }
        "close" => Ok(AutomationCommand::Close),
        "zoom" => Ok(AutomationCommand::Zoom),
        _ => bail!("uso: +pane [list|send|run|split|tab|focus|close|zoom]"),
    }
}

fn parse_pane_send_or_run(operation: &str, arguments: &[String]) -> Result<AutomationCommand> {
    let mut target_pane = None;
    let mut text_parts = Vec::new();
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--pane" => {
                let value = arguments
                    .get(index + 1)
                    .context("falta valor para --pane")?;
                target_pane = Some(
                    value
                        .parse()
                        .with_context(|| format!("pane id inválido: {value}"))?,
                );
                index += 2;
            }
            "--" => {
                text_parts.extend(arguments[index + 1..].iter().cloned());
                break;
            }
            other => {
                text_parts.push(other.to_owned());
                index += 1;
            }
        }
    }
    let text = text_parts.join(" ");
    if text.is_empty() {
        bail!("uso: +pane {operation} [--pane <id>] <texto>");
    }
    Ok(AutomationCommand::Send {
        text,
        newline: operation == "run",
        target_pane,
    })
}

fn parse_pane_split(arguments: &[String]) -> Result<AutomationCommand> {
    let direction = parse_direction(arguments.first().map(String::as_str))?;
    let rest = if arguments.is_empty() {
        &[][..]
    } else {
        &arguments[1..]
    };
    let no_focus = rest.iter().any(|argument| argument == "--no-focus");
    let cwd = parse_agent_flag(rest, "--cwd")?.map(PathBuf::from);
    Ok(AutomationCommand::Split {
        direction,
        no_focus,
        cwd,
    })
}

fn parse_pane_tab(arguments: &[String]) -> Result<AutomationCommand> {
    let no_focus = arguments.iter().any(|argument| argument == "--no-focus");
    let cwd = parse_agent_flag(arguments, "--cwd")?.map(PathBuf::from);
    Ok(AutomationCommand::CreateTab { no_focus, cwd })
}

fn parse_agent_target(arguments: &[String]) -> Result<Option<String>> {
    if let Some(value) = parse_agent_flag(arguments, "--pane")? {
        return Ok(Some(value));
    }
    if let Some(value) = parse_agent_flag(arguments, "--name")? {
        return Ok(Some(value));
    }
    if let Some(value) = parse_agent_flag(arguments, "--target")? {
        return Ok(Some(value));
    }
    // Positional target when it is not a flag.
    Ok(arguments
        .first()
        .filter(|value| !value.starts_with('-'))
        .cloned())
}

fn parse_until_states(arguments: &[String]) -> Result<Vec<AgentRuntimeState>> {
    let mut states = Vec::new();
    let mut index = 0;
    while index < arguments.len() {
        if arguments[index] == "--until" {
            let value = arguments
                .get(index + 1)
                .map(String::as_str)
                .context("falta valor para --until")?;
            states.push(parse_agent_runtime_state(Some(value))?);
            index += 2;
            continue;
        }
        index += 1;
    }
    Ok(states)
}

fn parse_agent_prompt(arguments: &[String]) -> Result<AutomationCommand> {
    let (flags, trailing) = split_agent_args(arguments);
    let mut target = None;
    if let Some(value) = parse_agent_flag(&flags, "--pane")? {
        target = Some(value);
    } else if let Some(value) = parse_agent_flag(&flags, "--name")? {
        target = Some(value);
    } else if let Some(value) = parse_agent_flag(&flags, "--target")? {
        target = Some(value);
    }
    let mut wait = true;
    let mut timeout_ms = DEFAULT_AGENT_WAIT_TIMEOUT_MS;
    let until = parse_until_states(&flags)?;
    if let Some(timeout) = parse_agent_flag(&flags, "--timeout")? {
        timeout_ms = parse_timeout_ms(Some(timeout.as_str()))?;
    }
    let mut free = Vec::new();
    let mut index = 0;
    while index < flags.len() {
        match flags[index].as_str() {
            "--pane" | "--name" | "--target" | "--until" | "--timeout" => index += 2,
            "--wait" => {
                wait = true;
                index += 1;
            }
            "--no-wait" => {
                wait = false;
                index += 1;
            }
            other if other.starts_with('-') => {
                bail!("flag desconocida en +agent prompt: {other}");
            }
            other => {
                free.push(other.to_owned());
                index += 1;
            }
        }
    }
    free.extend(trailing);
    // `+agent prompt reviewer Review the diff` → target + text when target not set by flags.
    if target.is_none() && free.len() >= 2 {
        target = Some(free.remove(0));
    }
    let text = free.join(" ");
    if text.is_empty() {
        bail!(
            "uso: +agent prompt [<target>] [--wait|--no-wait] [--timeout ms] [--until state] <texto>"
        );
    }
    Ok(AutomationCommand::AgentPrompt {
        target,
        text,
        wait,
        timeout_ms,
        until,
    })
}

fn parse_agent_wait(arguments: &[String]) -> Result<AutomationCommand> {
    let target = parse_agent_target(arguments)?;
    let mut timeout_ms = DEFAULT_AGENT_WAIT_TIMEOUT_MS;
    if let Some(timeout) = parse_agent_flag(arguments, "--timeout")? {
        timeout_ms = parse_timeout_ms(Some(timeout.as_str()))?;
    }
    let until = parse_until_states(arguments)?;
    Ok(AutomationCommand::AgentWait {
        target,
        timeout_ms,
        until,
    })
}

fn parse_agent_read(arguments: &[String]) -> Result<AutomationCommand> {
    let target = parse_agent_target(arguments)?;
    let mut lines = DEFAULT_AGENT_READ_LINES;
    if let Some(value) = parse_agent_flag(arguments, "--lines")? {
        lines = value
            .parse()
            .with_context(|| format!("--lines inválido: {value}"))?;
        if lines == 0 || lines > 2_000 {
            bail!("--lines debe estar entre 1 y 2000");
        }
    }
    Ok(AutomationCommand::AgentRead { target, lines })
}

fn parse_agent_rename(arguments: &[String]) -> Result<AutomationCommand> {
    let mut clear = false;
    let mut target = None;
    let mut name = None;
    let mut positional = Vec::new();
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--pane" | "--target" => {
                if target.is_some() {
                    bail!("el destino se indicó más de una vez");
                }
                target = Some(
                    arguments
                        .get(index + 1)
                        .cloned()
                        .context("falta valor para el destino")?,
                );
                index += 2;
            }
            // In `rename`, --name always means the new alias. This avoids the
            // ambiguity with the generic target parser used by read/wait/status.
            "--name" | "--to" => {
                if name.is_some() {
                    bail!("el nombre nuevo se indicó más de una vez");
                }
                name = Some(
                    arguments
                        .get(index + 1)
                        .cloned()
                        .context("falta valor para el nombre nuevo")?,
                );
                index += 2;
            }
            "--clear" => {
                clear = true;
                index += 1;
            }
            other if other.starts_with('-') => bail!("flag desconocida en +agent rename: {other}"),
            other => {
                positional.push(other.to_owned());
                index += 1;
            }
        }
    }
    match (target.is_some(), positional.as_slice()) {
        (false, [target_name, new_name]) if !clear && name.is_none() => {
            target = Some(target_name.clone());
            name = Some(new_name.clone());
        }
        (false, [target_name]) if clear => target = Some(target_name.clone()),
        (true, [new_name]) if !clear && name.is_none() => name = Some(new_name.clone()),
        (_, []) => {}
        _ => bail!(
            "uso: +agent rename <target> <name> | +agent rename --name <alias> | +agent rename <target> --clear"
        ),
    }
    if clear && name.is_some() {
        bail!("--clear no se puede combinar con un nombre nuevo");
    }
    if !clear && name.is_none() {
        bail!(
            "uso: +agent rename <target> <name> | +agent rename --name <alias> | +agent rename <target> --clear"
        );
    }
    if let Some(name) = name.as_ref() {
        validate_agent_name(name)?;
    }
    Ok(AutomationCommand::AgentRename {
        target,
        name,
        clear,
    })
}

pub fn validate_agent_name(name: &str) -> Result<()> {
    let valid = name.len() <= 32
        && name
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_lowercase())
        && name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '-' | '_'));
    if !valid {
        bail!("nombre de agente inválido (use [a-z][a-z0-9_-]{{0,31}}): {name}");
    }
    Ok(())
}

/// Default settled states for waits (idle or waiting/blocked).
pub fn default_settled_states() -> Vec<AgentRuntimeState> {
    vec![AgentRuntimeState::Idle, AgentRuntimeState::Waiting]
}

fn parse_agent_open(arguments: &[String]) -> Result<AutomationCommand> {
    let kind = arguments
        .first()
        .and_then(|kind| AgentKind::parse(kind))
        .with_context(|| {
            format!(
                "kind esperado: {}",
                AgentKind::ALL
                    .iter()
                    .map(|kind| kind.cli_name())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
    let (flags, args) = split_agent_args(&arguments[1..]);
    let mut placement = AgentPlacement::Split;
    let mut direction = AutomationDirection::Right;
    let mut no_focus = true;
    let mut name = None;
    let mut cwd = None;
    let mut timeout_ms = DEFAULT_AGENT_START_TIMEOUT_MS;
    let mut wait = true;
    let mut index = 0;
    while index < flags.len() {
        match flags[index].as_str() {
            "--tab" => {
                placement = AgentPlacement::Tab;
                index += 1;
            }
            "--current" => {
                placement = AgentPlacement::Current;
                index += 1;
            }
            "--split" => {
                placement = AgentPlacement::Split;
                let value = flags
                    .get(index + 1)
                    .map(String::as_str)
                    .filter(|value| !value.starts_with("--"));
                if let Some(value) = value {
                    direction = parse_direction(Some(value))?;
                    index += 2;
                } else {
                    direction = AutomationDirection::Right;
                    index += 1;
                }
            }
            "--no-focus" => {
                no_focus = true;
                index += 1;
            }
            "--focus" => {
                no_focus = false;
                index += 1;
            }
            "--name" => {
                name = Some(
                    flags
                        .get(index + 1)
                        .cloned()
                        .context("falta valor para --name")?,
                );
                index += 2;
            }
            "--cwd" => {
                cwd = Some(PathBuf::from(
                    flags
                        .get(index + 1)
                        .cloned()
                        .context("falta valor para --cwd")?,
                ));
                index += 2;
            }
            "--timeout" => {
                timeout_ms = parse_timeout_ms(flags.get(index + 1).map(String::as_str))?;
                index += 2;
            }
            "--wait" => {
                wait = true;
                index += 1;
            }
            "--no-wait" => {
                wait = false;
                index += 1;
            }
            other => bail!("flag desconocida en +agent open: {other}"),
        }
    }
    if let Some(name) = name.as_ref() {
        validate_agent_name(name)?;
    }
    Ok(AutomationCommand::AgentOpen {
        kind,
        placement,
        direction,
        no_focus,
        name,
        cwd,
        timeout_ms,
        wait,
        args,
    })
}

fn parse_agent_start(arguments: &[String]) -> Result<AutomationCommand> {
    let (flags, args) = split_agent_args(arguments);
    let mut kind = None;
    let mut pane = None;
    let mut name = None;
    let mut timeout_ms = DEFAULT_AGENT_START_TIMEOUT_MS;
    let mut wait = true;
    let mut index = 0;
    while index < flags.len() {
        match flags[index].as_str() {
            "--kind" => {
                let value = flags.get(index + 1).context("falta valor para --kind")?;
                kind = Some(
                    AgentKind::parse(value).with_context(|| format!("kind inválido: {value}"))?,
                );
                index += 2;
            }
            "--pane" => {
                let value = flags.get(index + 1).context("falta valor para --pane")?;
                pane = Some(
                    value
                        .parse()
                        .with_context(|| format!("pane id inválido: {value}"))?,
                );
                index += 2;
            }
            "--name" => {
                name = Some(
                    flags
                        .get(index + 1)
                        .cloned()
                        .context("falta valor para --name")?,
                );
                index += 2;
            }
            "--timeout" => {
                timeout_ms = parse_timeout_ms(flags.get(index + 1).map(String::as_str))?;
                index += 2;
            }
            "--wait" => {
                wait = true;
                index += 1;
            }
            "--no-wait" => {
                wait = false;
                index += 1;
            }
            other if kind.is_none() && AgentKind::parse(other).is_some() => {
                kind = AgentKind::parse(other);
                index += 1;
            }
            other => bail!("flag desconocida en +agent start: {other}"),
        }
    }
    let kind = kind.context("uso: +agent start --kind <kind> [--pane <id>] [--name <alias>]")?;
    if let Some(name) = name.as_ref() {
        validate_agent_name(name)?;
    }
    Ok(AutomationCommand::AgentStart {
        kind,
        pane,
        name,
        timeout_ms,
        wait,
        args,
    })
}

fn split_agent_args(arguments: &[String]) -> (Vec<String>, Vec<String>) {
    if let Some(index) = arguments.iter().position(|argument| argument == "--") {
        (arguments[..index].to_vec(), arguments[index + 1..].to_vec())
    } else {
        (arguments.to_vec(), Vec::new())
    }
}

fn parse_timeout_ms(value: Option<&str>) -> Result<u64> {
    let value = value.context("falta valor para --timeout")?;
    let timeout: u64 = value
        .parse()
        .with_context(|| format!("timeout inválido: {value}"))?;
    if !(3_000..=300_000).contains(&timeout) {
        bail!("--timeout debe estar entre 3000 y 300000 ms");
    }
    Ok(timeout)
}

fn parse_direction(value: Option<&str>) -> Result<AutomationDirection> {
    match value {
        Some("left") => Ok(AutomationDirection::Left),
        Some("right") => Ok(AutomationDirection::Right),
        Some("up") => Ok(AutomationDirection::Up),
        Some("down") => Ok(AutomationDirection::Down),
        _ => bail!("dirección esperada: left, right, up o down"),
    }
}

const AUTOMATION_SKILL: &str = r#"# Vibra automation skill

Use these commands only when the user asks to open another agent, split panes, prompt another agent, or control Vibra from inside a pane.

## Preconditions

```bash
test -n "$VIBRA_CLI" && test -n "$VIBRA_AUTOMATION_SOCKET" && test -n "$VIBRA_PANE_ID"
```

If any is missing, say you are not running inside Vibra and stop.

## Open an agent

Default layout is a sibling split to the right without stealing focus:

```bash
"$VIBRA_CLI" +agent open codex --split right --no-focus --name reviewer
"$VIBRA_CLI" +agent open claude --tab --no-focus --cwd "$PWD"
"$VIBRA_CLI" +agent open codex --split down --name builder -- -m o3
```

Supported kinds: aider, amp, claude, codex, cursor, gemini, goose, grok, opencode, pi.
Names must match `[a-z][a-z0-9_-]{0,31}` and be unique in the project.

```bash
"$VIBRA_CLI" +agent kinds
"$VIBRA_CLI" +agent list
```

Parse JSON from stdout. Do not invent pane IDs or names.

## Prompt, wait, and read

Targets are a pane UUID or a live agent name:

```bash
"$VIBRA_CLI" +agent prompt reviewer "Review the current diff" --wait --timeout 120000
"$VIBRA_CLI" +agent wait reviewer --until waiting --timeout 120000
"$VIBRA_CLI" +agent read reviewer --lines 120
"$VIBRA_CLI" +agent status reviewer
"$VIBRA_CLI" +agent rename <pane-id> --to reviewer
```

`--wait` (default on prompt) requires structured activity hooks and waits until the agent is settled (`idle` or `waiting`). Use `--until idle` / `--until waiting` / `--until working` to narrow it. For agents reported with heuristic-only activity tracking, use `--no-wait`, then `read` the pane explicitly.

## Layout primitives

```bash
"$VIBRA_CLI" +pane split right --no-focus --cwd "$PWD"
"$VIBRA_CLI" +pane tab --no-focus
"$VIBRA_CLI" +pane run --pane <id> "npm test"
"$VIBRA_CLI" +agent start --kind codex --pane <id> --name reviewer
"$VIBRA_CLI" +pane list
```

## Rules

- Prefer `+agent open <kind> --name <alias>` over manual split+run unless the user wants a bare shell.
- Keep the user's focus with `--no-focus` unless they asked to switch.
- Do not close panes/tabs you did not create.
- Honor the agent kind the user named.
- Extra agent CLI args go after `--`.
- After a wait returns `waiting`, read the pane before deciding what to send.
"#;

fn parse_agent_presence(arguments: &[String]) -> Result<AutomationCommand> {
    let kind = arguments
        .first()
        .and_then(|kind| AgentKind::parse(kind))
        .with_context(|| {
            format!(
                "agent esperado: {}",
                AgentKind::ALL
                    .iter()
                    .map(|kind| kind.cli_name())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
    let state = parse_agent_runtime_state(arguments.get(1).map(String::as_str))?;
    let attention = arguments
        .get(2)
        .filter(|value| !value.starts_with("--"))
        .and_then(|value| AgentAttention::parse(value))
        .or_else(|| (state == AgentRuntimeState::Waiting).then_some(AgentAttention::Notification));
    if arguments
        .get(2)
        .is_some_and(|value| !value.starts_with("--") && attention.is_none())
    {
        bail!("atención esperada: permission, question, plan o notification");
    }
    Ok(AutomationCommand::SetAgentPresence {
        kind,
        state,
        attention,
        session_id: parse_agent_flag(arguments, "--session")?,
    })
}

fn parse_agent_attention(arguments: &[String]) -> Result<AutomationCommand> {
    let kind = arguments
        .first()
        .and_then(|kind| AgentKind::parse(kind))
        .context("agent esperado después de attention")?;
    let attention = arguments
        .get(1)
        .and_then(|attention| AgentAttention::parse(attention))
        .unwrap_or(AgentAttention::Notification);
    Ok(AutomationCommand::SetAgentPresence {
        kind,
        state: AgentRuntimeState::Waiting,
        attention: Some(attention),
        session_id: parse_agent_flag(arguments, "--session")?,
    })
}

fn parse_agent_hook(arguments: &[String]) -> Result<AutomationCommand> {
    let kind = arguments
        .first()
        .and_then(|kind| AgentKind::parse(kind))
        .context("uso: +agent hook <claude|codex> <evento>")?;
    let event = arguments
        .get(1)
        .map(String::as_str)
        .context("falta evento de hook")?;
    let mut input = String::new();
    std::io::stdin()
        .take(MAX_AGENT_HOOK_BYTES + 1)
        .read_to_string(&mut input)
        .context("no se pudo leer el JSON del hook")?;
    if input.len() as u64 > MAX_AGENT_HOOK_BYTES {
        bail!("el payload del hook supera 1 MiB");
    }
    let payload = if input.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&input).context("JSON de hook inválido")?
    };
    agent_hook_command(kind, event, &payload)
}

pub(super) fn agent_hook_command(
    kind: AgentKind,
    event: &str,
    payload: &Value,
) -> Result<AutomationCommand> {
    let session_id = payload
        .get("session_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let presence = |state, attention| AutomationCommand::SetAgentPresence {
        kind,
        state,
        attention,
        session_id: session_id.clone(),
    };
    match (kind, event) {
        (AgentKind::Claude | AgentKind::Codex, "session-end") => {
            Ok(AutomationCommand::ClearAgentPresence { session_id })
        }
        (AgentKind::Claude | AgentKind::Codex, "prompt") => {
            Ok(presence(AgentRuntimeState::Working, None))
        }
        (AgentKind::Claude | AgentKind::Codex, "stop" | "session-start") => {
            Ok(presence(AgentRuntimeState::Idle, None))
        }
        (AgentKind::Claude | AgentKind::Codex, "permission") => Ok(presence(
            AgentRuntimeState::Waiting,
            Some(AgentAttention::Permission),
        )),
        (AgentKind::Claude, "notification") => {
            match payload.get("notification_type").and_then(Value::as_str) {
                Some("permission_prompt") => Ok(presence(
                    AgentRuntimeState::Waiting,
                    Some(AgentAttention::Permission),
                )),
                Some("idle_prompt") => Ok(presence(AgentRuntimeState::Idle, None)),
                _ => bail!("notificación de Claude no soportada"),
            }
        }
        (AgentKind::Claude | AgentKind::Codex, _) => bail!("evento de hook no soportado: {event}"),
        _ => bail!("Vibra todavía no incluye hooks para {}", kind.cli_name()),
    }
}

fn parse_agent_runtime_state(value: Option<&str>) -> Result<AgentRuntimeState> {
    match value {
        Some("idle") => Ok(AgentRuntimeState::Idle),
        Some("working") => Ok(AgentRuntimeState::Working),
        Some("waiting") => Ok(AgentRuntimeState::Waiting),
        _ => bail!("estado esperado: idle, working o waiting"),
    }
}

fn parse_agent_flag(arguments: &[String], flag: &str) -> Result<Option<String>> {
    let Some(index) = arguments.iter().position(|argument| argument == flag) else {
        return Ok(None);
    };
    arguments
        .get(index + 1)
        .cloned()
        .map(Some)
        .with_context(|| format!("falta valor para {flag}"))
}
