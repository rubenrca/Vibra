//! JSON-RPC 2.0 framing for ACP: one JSON message per line, in both directions.

use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc};

use serde_json::{Value, json};

pub(super) const METHOD_NOT_FOUND: i64 = -32601;
pub(super) const INVALID_PARAMS: i64 = -32602;
pub(super) const INTERNAL_ERROR: i64 = -32603;

/// A JSON-RPC error object. Agents report a missing sign-in as code -32000.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RpcError {
    pub code: i64,
    pub message: String,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn method_not_found(method: &str) -> Self {
        Self::new(
            METHOD_NOT_FOUND,
            format!("Vibra does not support {method}."),
        )
    }

    /// Reads an error object. Agents often put the real cause in `data`
    /// (`{"details": "…"}` or a plain string) under a generic message such as
    /// "Internal error", so the cause is appended in parentheses.
    fn from_json(value: &Value) -> Self {
        let message = value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("The agent returned an unknown error.");
        let details = value.get("data").and_then(|data| {
            data.get("details")
                .and_then(Value::as_str)
                .or_else(|| data.as_str())
                .map(str::trim)
                .filter(|details| !details.is_empty() && *details != message)
        });
        Self::new(
            value
                .get("code")
                .and_then(Value::as_i64)
                .unwrap_or(INTERNAL_ERROR),
            match details {
                Some(details) => format!("{message} ({details})"),
                None => message.to_owned(),
            },
        )
    }
}

/// One decoded message from the agent.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Incoming {
    /// Answer to a request we sent. `id` is the id we chose.
    Response {
        id: Value,
        result: Result<Value, RpcError>,
    },
    /// The agent asks us something and waits for a response with `id`.
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
}

/// Why the agent's output stopped being readable.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum StreamEnd {
    /// EOF or a read error: the agent closed its stdout.
    Closed,
    /// A line that is not a JSON-RPC message. The stream cannot be trusted after it.
    Malformed(String),
}

fn parse_message(value: Value) -> Result<Incoming, String> {
    let Value::Object(mut fields) = value else {
        return Err("the message is not a JSON object".to_owned());
    };
    let id = fields.get("id").filter(|id| !id.is_null()).cloned();
    let method = fields
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_owned);
    match (method, id) {
        (Some(method), Some(id)) => Ok(Incoming::Request {
            id,
            method,
            params: fields.remove("params").unwrap_or(Value::Null),
        }),
        (Some(method), None) => Ok(Incoming::Notification {
            method,
            params: fields.remove("params").unwrap_or(Value::Null),
        }),
        (None, Some(id)) => {
            let result = match fields.remove("error").filter(|error| !error.is_null()) {
                Some(error) => Err(RpcError::from_json(&error)),
                None => Ok(fields.remove("result").unwrap_or(Value::Null)),
            };
            Ok(Incoming::Response { id, result })
        }
        (None, None) => Err("the message has neither a method nor an id".to_owned()),
    }
}

/// Reads newline-delimited messages until the stream ends or breaks.
pub(super) fn read_messages(
    mut reader: impl BufRead,
    mut on_message: impl FnMut(Incoming),
) -> StreamEnd {
    let mut line = Vec::new();
    loop {
        match reader.read_until(b'\n', &mut line) {
            Ok(0) => return StreamEnd::Closed,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return StreamEnd::Closed,
        }
        if !line.iter().all(u8::is_ascii_whitespace) {
            let parsed = serde_json::from_slice::<Value>(&line)
                .map_err(|error| error.to_string())
                .and_then(parse_message);
            match parsed {
                Ok(message) => on_message(message),
                Err(error) => return StreamEnd::Malformed(error),
            }
        }
        line.clear();
    }
}

pub(super) fn request(id: i64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

pub(super) fn notification(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": method, "params": params})
}

pub(super) fn result_response(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

pub(super) fn error_response(id: &Value, error: &RpcError) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": error.code, "message": error.message},
    })
}

/// Queue of outgoing messages. The writer thread drains it, so a slow agent
/// that stops reading stdin never blocks the reader.
pub(super) struct Outbox {
    sender: Mutex<Option<mpsc::Sender<Value>>>,
}

impl Outbox {
    pub(super) fn new() -> (Arc<Self>, mpsc::Receiver<Value>) {
        let (sender, receiver) = mpsc::channel();
        (
            Arc::new(Self {
                sender: Mutex::new(Some(sender)),
            }),
            receiver,
        )
    }

    /// Returns false once the outbox is closed or the writer has stopped.
    pub(super) fn send(&self, message: Value) -> bool {
        lock(&self.sender)
            .as_ref()
            .is_some_and(|sender| sender.send(message).is_ok())
    }

    /// Stops accepting messages. The writer exits after draining the queue and
    /// drops stdin, which tells the agent its input has ended.
    pub(super) fn close(&self) {
        lock(&self.sender).take();
    }
}

pub(super) fn write_messages(mut writer: impl Write, messages: mpsc::Receiver<Value>) {
    for message in messages {
        let mut line = message.to_string();
        line.push('\n');
        if writer.write_all(line.as_bytes()).is_err() || writer.flush().is_err() {
            break;
        }
    }
}

/// Locks ignoring poison: a panicked thread must not take the chat down with it.
pub(super) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn read_all(input: &str) -> (Vec<Incoming>, StreamEnd) {
        let mut messages = Vec::new();
        let end = read_messages(Cursor::new(input.as_bytes().to_vec()), |message| {
            messages.push(message)
        });
        (messages, end)
    }

    #[test]
    fn classifies_responses_requests_and_notifications() {
        let (messages, end) = read_all(concat!(
            r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":"x","error":{"code":-32000,"message":"auth"}}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":7,"method":"terminal/output","params":{}}"#,
            "\n",
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s"}}"#,
            "\n",
        ));
        assert_eq!(end, StreamEnd::Closed);
        assert_eq!(
            messages,
            vec![
                Incoming::Response {
                    id: json!(1),
                    result: Ok(json!({"ok": true})),
                },
                Incoming::Response {
                    id: json!("x"),
                    result: Err(RpcError::new(-32000, "auth")),
                },
                Incoming::Request {
                    id: json!(7),
                    method: "terminal/output".to_owned(),
                    params: json!({}),
                },
                Incoming::Notification {
                    method: "session/update".to_owned(),
                    params: json!({"sessionId": "s"}),
                },
            ]
        );
    }

    #[test]
    fn skips_blank_lines_and_stops_at_malformed_line() {
        let (messages, end) = read_all("\n   \n{\"method\":\"a\"}\nnot json\n{\"method\":\"b\"}\n");
        assert_eq!(messages.len(), 1);
        assert!(matches!(end, StreamEnd::Malformed(_)));
    }

    #[test]
    fn rejects_objects_without_method_or_id() {
        let (messages, end) = read_all("{\"result\":1}\n");
        assert!(messages.is_empty());
        assert!(matches!(end, StreamEnd::Malformed(_)));
    }

    #[test]
    fn outbox_stops_accepting_after_close() {
        let (outbox, receiver) = Outbox::new();
        assert!(outbox.send(json!({"n": 1})));
        outbox.close();
        assert!(!outbox.send(json!({"n": 2})));
        assert_eq!(receiver.recv().ok(), Some(json!({"n": 1})));
        assert!(receiver.recv().is_err());
    }

    #[test]
    fn writer_emits_one_json_line_per_message() {
        let (outbox, receiver) = Outbox::new();
        outbox.send(json!({"a": 1}));
        outbox.send(json!({"b": "é"}));
        outbox.close();
        let mut written = Vec::new();
        write_messages(&mut written, receiver);
        assert_eq!(
            String::from_utf8(written).unwrap(),
            "{\"a\":1}\n{\"b\":\"é\"}\n"
        );
    }
}
