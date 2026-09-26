//! Small bounded HTTPS reader. Secrets travel through stdin, never process args.

use std::io::{Read, Write};
use std::process::{Command, Stdio};

use serde_json::Value;

use super::UsageFailure;
use crate::domain::usage::{now_timestamp, timestamp};

const MAX_RESPONSE: u64 = 1_048_576;

pub(super) fn get_json(url: &str, headers: &[(&str, &str)]) -> Result<Value, UsageFailure> {
    let config = header_config(headers)?;
    let mut child = Command::new("/usr/bin/curl")
        .args([
            "--disable",
            "--silent",
            "--proto",
            "=https",
            "--connect-timeout",
            "5",
            "--max-time",
            "15",
            "--max-filesize",
            "1048576",
            "--suppress-connect-headers",
            "--dump-header",
            "-",
            "--user-agent",
            "Vibra/0.3 subscription-usage",
            "--config",
            "-",
            url,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| UsageFailure::connection())?;
    let written = child.stdin.take().unwrap().write_all(config.as_bytes());
    if written.is_err() {
        let _ = child.kill();
        let _ = child.wait();
        return Err(UsageFailure::connection());
    }
    let mut response = Vec::new();
    let read = child
        .stdout
        .take()
        .unwrap()
        .take(MAX_RESPONSE + 1)
        .read_to_end(&mut response);
    if read.is_err() || response.len() as u64 > MAX_RESPONSE {
        let _ = child.kill();
        let _ = child.wait();
        return Err(UsageFailure::invalid_response());
    }
    let status = child.wait().map_err(|_| UsageFailure::connection())?;
    if !status.success() {
        return Err(UsageFailure::connection());
    }
    parse_response(&response)
}

fn header_config(headers: &[(&str, &str)]) -> Result<String, UsageFailure> {
    let mut config = String::from("header = \"Accept: application/json\"\n");
    for (name, value) in headers {
        if name.chars().chain(value.chars()).any(|c| c.is_control()) {
            return Err(UsageFailure::new(
                "Credencial no válida. Vuelve a iniciar sesión en el CLI.",
            ));
        }
        let header = format!("{name}: {value}")
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        config.push_str(&format!("header = \"{header}\"\n"));
    }
    Ok(config)
}

fn parse_response(mut bytes: &[u8]) -> Result<Value, UsageFailure> {
    // Handle informational responses; redirects are deliberately not followed.
    loop {
        let split = bytes
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .ok_or_else(UsageFailure::invalid_response)?;
        let headers =
            std::str::from_utf8(&bytes[..split]).map_err(|_| UsageFailure::invalid_response())?;
        let status: u16 = headers
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|status| status.parse().ok())
            .ok_or_else(UsageFailure::invalid_response)?;
        bytes = &bytes[split + 4..];
        if (100..200).contains(&status) {
            continue;
        }
        match status {
            200..300 => {
                return serde_json::from_slice(bytes).map_err(|_| UsageFailure::invalid_response());
            }
            401 | 403 => {
                return Err(UsageFailure::new(
                    "La sesión no permite consultar cuotas. Abre el CLI y vuelve a iniciar sesión.",
                ));
            }
            429 => {
                let retry = headers
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
                    .and_then(|(_, value)| retry_after(value.trim(), now_timestamp()));
                return Err(UsageFailure {
                    message: "El servicio limitó las consultas. Vibra reintentará más tarde."
                        .into(),
                    retry_after: Some(retry.unwrap_or(900).max(300)),
                });
            }
            _ => {
                return Err(UsageFailure::new(format!(
                    "No se pudieron consultar las cuotas (HTTP {status})."
                )));
            }
        }
    }
}

fn retry_after(value: &str, now: i64) -> Option<u64> {
    value.parse::<u64>().ok().or_else(|| {
        let date = chrono::DateTime::parse_from_rfc2822(value)
            .ok()?
            .to_rfc3339();
        Some(timestamp(&date)?.saturating_sub(now).max(0) as u64)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_success_and_never_surfaces_server_error_bodies() {
        let data =
            parse_response(b"HTTP/2 200\r\nContent-Type: application/json\r\n\r\n{\"ok\":true}")
                .unwrap();
        assert_eq!(data["ok"], true);
        let error = parse_response(b"HTTP/2 401\r\n\r\nprivate-token-and-account").unwrap_err();
        assert!(!error.message.contains("private-token"));
        assert!(parse_response(b"HTTP/2 302\r\nLocation: https://example.com\r\n\r\n").is_err());
    }

    #[test]
    fn honors_rate_limit_cooldown_and_rejects_header_injection() {
        let error = parse_response(b"HTTP/2 429\r\nRetry-After: 3600\r\n\r\n{}").unwrap_err();
        assert_eq!(error.retry_after, Some(3600));
        assert!(
            header_config(&[("Authorization", "Bearer secret\nurl = https://evil.test")]).is_err()
        );
        assert_eq!(
            header_config(&[("X-Value", "a\"b\\c")]).unwrap(),
            "header = \"Accept: application/json\"\nheader = \"X-Value: a\\\"b\\\\c\"\n"
        );
        let now = timestamp("2026-09-26T12:00:00Z").unwrap();
        assert_eq!(
            retry_after("Sat, 26 Sep 2026 13:00:00 GMT", now),
            Some(3600)
        );
    }
}
