use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use super::*;

/// Handles hook setup as well as the small internal bridge used by agent hooks.
///
/// Agent processes are still launched by the user in a terminal. Vibra only
/// accepts lifecycle updates here; it does not expose commands for creating
/// panes, launching other agents, or sending input to them.
pub fn run_cli(arguments: &[String]) -> Result<bool> {
    let Some(mode) = arguments.first().map(String::as_str) else {
        return Ok(false);
    };
    if mode == "agent" {
        run_agent_setup_cli(&arguments[1..])?;
        return Ok(true);
    }
    if matches!(mode, "+pane" | "+skill") {
        bail!("la CLI de orquestación de panes y agentes ya no está disponible");
    }
    if mode != "+agent" {
        return Ok(false);
    }

    let command = parse_cli_command(mode, &arguments[1..])?;
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
        bail!("la respuesta de seguimiento supera 4 MiB");
    }
    let response: AutomationResponse = serde_json::from_slice(&response)?;
    if !response.ok {
        bail!(
            response
                .error
                .unwrap_or_else(|| "no se pudo actualizar el seguimiento del agente".into())
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
    if mode != "+agent" {
        bail!("el puente de agentes solo se puede usar con +agent");
    }
    let operation = arguments
        .first()
        .map(String::as_str)
        .context("uso: +agent [presence|attention|hook|clear|idle|working|waiting]")?;
    match operation {
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
        _ => bail!("uso: +agent [presence|attention|hook|clear|idle|working|waiting]"),
    }
}

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
        task_title: None,
        kind,
        state,
        attention,
        model: parse_agent_flag(arguments, "--model")?,
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
        task_title: None,
        kind,
        state: AgentRuntimeState::Waiting,
        attention: Some(attention),
        model: parse_agent_flag(arguments, "--model")?,
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
    let model = agent_hook_model(payload);
    let presence = |state, attention| AutomationCommand::SetAgentPresence {
        task_title: (event == "prompt")
            .then(|| {
                payload
                    .get("prompt")
                    .and_then(Value::as_str)
                    .and_then(agent_task_title)
            })
            .flatten(),
        kind,
        state,
        attention,
        model: model.clone(),
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

/// Hook payloads are provider-owned and have changed names over time. Accept
/// the common scalar forms while leaving the model absent when it is not
/// explicitly provided; presenting an invented default would be misleading.
fn agent_hook_model(payload: &Value) -> Option<String> {
    let value = payload
        .get("model")
        .and_then(|model| {
            model
                .as_str()
                .or_else(|| model.get("id").and_then(Value::as_str))
                .or_else(|| model.get("name").and_then(Value::as_str))
        })
        .or_else(|| payload.get("model_name").and_then(Value::as_str))
        .or_else(|| payload.get("model_id").and_then(Value::as_str))?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
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

/// A local, bounded label from the user's first prose line. Follow-up acknowledgements
/// do not replace the task; code and injected context are not useful sidebar labels.
fn agent_task_title(prompt: &str) -> Option<String> {
    let line = prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    if line.starts_with(['<', '/', '#']) || line.starts_with("```") {
        return None;
    }
    let clean: String = line.chars().filter(|ch| !ch.is_control()).collect();
    let mut text = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    let normalized = text
        .trim_matches(|ch: char| !ch.is_alphanumeric())
        .to_lowercase();
    if matches!(
        normalized.as_str(),
        "sí" | "si"
            | "no"
            | "ok"
            | "okay"
            | "dale"
            | "haz eso"
            | "hazlo"
            | "continúa"
            | "continua"
            | "sigue"
            | "perfecto"
            | "gracias"
            | "yes"
            | "continue"
            | "go ahead"
            | "do it"
            | "thanks"
            | "proceed"
    ) {
        return None;
    }
    text = text.trim_start_matches(['¿', '¡']).trim().to_owned();
    for prefix in [
        "por favor, ",
        "por favor ",
        "puedes ",
        "podrías ",
        "quiero que ",
        "necesito que ",
        "please ",
        "can you ",
        "could you ",
    ] {
        if text.to_lowercase().starts_with(prefix) {
            text = text[prefix.len()..].trim().to_owned();
        }
    }
    let text = text
        .trim_matches(|ch: char| ch.is_whitespace() || matches!(ch, '¿' | '?' | '¡' | '!' | '.'));
    if text.chars().count() < 4 {
        return None;
    }
    let mut title = String::new();
    for word in text.split_whitespace().take(9) {
        if title.chars().count() + usize::from(!title.is_empty()) + word.chars().count() > 56 {
            break;
        }
        if !title.is_empty() {
            title.push(' ');
        }
        title.push_str(word);
    }
    if title.is_empty() {
        title = text.chars().take(56).collect();
    }
    if title != text {
        title.push('…');
    }
    let mut chars = title.chars();
    Some(chars.next()?.to_uppercase().collect::<String>() + chars.as_str())
}

#[cfg(test)]
mod task_title_tests {
    use super::*;

    #[test]
    fn prompt_hooks_carry_titles_but_lifecycle_events_do_not() {
        for kind in [AgentKind::Claude, AgentKind::Codex] {
            let payload =
                serde_json::json!({"prompt": "¿Puedes corregir el login?", "session_id": "one"});
            assert!(
                matches!(agent_hook_command(kind, "prompt", &payload).unwrap(),
                AutomationCommand::SetAgentPresence { task_title: Some(title), state: AgentRuntimeState::Working, .. }
                if title == "Corregir el login")
            );
            for event in ["stop", "session-start", "permission"] {
                assert!(matches!(
                    agent_hook_command(kind, event, &payload).unwrap(),
                    AutomationCommand::SetAgentPresence {
                        task_title: None,
                        ..
                    }
                ));
            }
        }
    }

    #[test]
    fn titles_ignore_acknowledgements_and_non_prose_and_bound_unicode() {
        for prompt in [
            "Sí!",
            "continúa",
            "haz eso",
            "OK",
            "",
            "<environment_context>",
            "```rust",
            "/help",
        ] {
            assert_eq!(agent_task_title(prompt), None, "{prompt}");
        }
        assert_eq!(
            agent_task_title("por favor corregir el login\nDetalles adicionales"),
            Some("Corregir el login".into())
        );
        let title = agent_task_title(&"á".repeat(100)).unwrap();
        assert_eq!(title.chars().count(), 57);
        assert!(title.ends_with('…'));
        let title = agent_task_title("Agregar soporte para notificaciones cuando el agente termine de ejecutar todas las pruebas").unwrap();
        assert!(title.chars().count() <= 57);
        assert!(title.ends_with('…'));
    }
}
