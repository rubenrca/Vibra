//! Ask the provider's own CLI for the quota. The CLI renews its login when it has
//! to, so it still answers after the stored access token expired or was replaced.
//! Vibra itself never refreshes a token. Runs in the login shell, like the terminal.

use std::time::Duration;

use serde_json::{Value, json};

use super::Provider;
use crate::infrastructure::login_shell;
use crate::infrastructure::process::{CommandLimits, command_reply};

const TIMEOUT: Duration = Duration::from_secs(20);
const MAX_REPLY: usize = 1_048_576;
const REQUEST_ID: &str = "vibra-usage";

/// The CLI's quota reading, in the provider's own format. `None` when the CLI is
/// missing, signed out, too old to report quotas, or did not answer in time.
pub(super) fn read(provider: Provider) -> Option<Value> {
    let (script, input) = match provider {
        // No settings, MCP servers or commands: the reading starts no hooks or tools,
        // and no prompt is sent, so it uses no quota.
        Provider::Claude => (
            "exec claude -p --input-format stream-json --output-format stream-json --verbose \
             --no-session-persistence --setting-sources '' --strict-mcp-config \
             --disable-slash-commands",
            lines(&[json!({
                "type": "control_request",
                "request_id": REQUEST_ID,
                "request": { "subtype": "get_usage", "skip_behaviors": true }
            })]),
        ),
        Provider::Codex => (
            "exec codex app-server",
            lines(&[
                json!({
                    "id": 1,
                    "method": "initialize",
                    "params": { "clientInfo": client_info() }
                }),
                json!({ "method": "initialized" }),
                json!({ "id": REQUEST_ID, "method": "account/rateLimits/read" }),
            ]),
        ),
        Provider::Grok => (
            "exec grok agent stdio",
            lines(&[
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": 1,
                        "clientCapabilities": {},
                        "clientInfo": client_info()
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": REQUEST_ID,
                    "method": "_x.ai/billing",
                    "params": {}
                }),
            ]),
        ),
    };
    let mut command = login_shell::command(script);
    // A neutral folder: the reading must not pick up a project's configuration.
    command.current_dir(std::env::temp_dir());
    let reply = command_reply(
        &mut command,
        input.as_bytes(),
        CommandLimits {
            timeout: TIMEOUT,
            stdout: MAX_REPLY,
            stderr: 0,
        },
        |line| reply(provider, line),
    )
    .ok()??;
    reply.is_object().then_some(reply)
}

fn client_info() -> Value {
    json!({ "name": "vibra", "title": "Vibra", "version": env!("CARGO_PKG_VERSION") })
}

fn lines(messages: &[Value]) -> String {
    messages
        .iter()
        .map(|message| format!("{message}\n"))
        .collect()
}

/// The answer to Vibra's request among the CLI's other output. A refusal is an
/// answer too, as `Null`, so the caller stops waiting.
fn reply(provider: Provider, line: &[u8]) -> Option<Value> {
    let mut message: Value = serde_json::from_slice(line).ok()?;
    let reading = match provider {
        Provider::Claude => {
            if message["response"]["request_id"].as_str() != Some(REQUEST_ID) {
                return None;
            }
            message.pointer_mut("/response/response").filter(|reading| {
                reading["rate_limits_available"] == true && reading["rate_limits"].is_object()
            })
        }
        Provider::Codex | Provider::Grok => {
            if message["id"].as_str() != Some(REQUEST_ID) {
                return None;
            }
            message.get_mut("result")
        }
    };
    Some(reading.map_or(Value::Null, Value::take))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_its_own_answer_and_treats_a_refusal_as_no_reading() {
        let claude = |line: &str| reply(Provider::Claude, line.as_bytes());
        assert_eq!(claude("zsh banner"), None);
        assert_eq!(claude("42"), None);
        assert_eq!(claude(r#"{"type":"system","subtype":"init"}"#), None);
        assert_eq!(
            claude(
                r#"{"type":"control_response","response":{"request_id":"other","response":{}}}"#
            ),
            None
        );
        let reading = claude(
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"vibra-usage","response":{"subscription_type":"max","rate_limits_available":true,"rate_limits":{"five_hour":{"utilization":4}}}}}"#,
        )
        .unwrap();
        assert_eq!(reading["rate_limits"]["five_hour"]["utilization"], 4);
        assert_eq!(reading["subscription_type"], "max");
        assert_eq!(
            claude(
                r#"{"type":"control_response","response":{"subtype":"success","request_id":"vibra-usage","response":{"rate_limits_available":false}}}"#
            ),
            Some(Value::Null)
        );
        assert_eq!(
            claude(
                r#"{"type":"control_response","response":{"subtype":"error","request_id":"vibra-usage","error":"Unsupported"}}"#
            ),
            Some(Value::Null)
        );

        let codex = |line: &str| reply(Provider::Codex, line.as_bytes());
        assert_eq!(codex(r#"{"id":1,"result":{"codexHome":"/tmp"}}"#), None);
        assert_eq!(codex(r#"{"method":"account/updated","params":{}}"#), None);
        assert_eq!(
            codex(r#"{"id":"vibra-usage","result":{"rateLimits":{"planType":"plus"}}}"#).unwrap()["rateLimits"]
                ["planType"],
            "plus"
        );
        assert_eq!(
            codex(r#"{"id":"vibra-usage","error":{"code":-32600,"message":"Not signed in"}}"#),
            Some(Value::Null)
        );
    }

    #[test]
    fn requests_are_one_json_message_per_line() {
        let input = lines(&[json!({ "id": 1 }), json!({ "text": "a\nb" })]);
        assert_eq!(input.lines().count(), 2);
        assert!(input.ends_with('\n'));
    }
}
