//! Read the CLI's current login each time. Never persist or rotate its tokens.

use std::ffi::CString;
use std::io::Read;
use std::path::{Path, PathBuf};

use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{Provider, UsageFailure};
use crate::domain::usage::{now_timestamp, timestamp};

// Deliberately no Debug or Serialize: these values must never enter diagnostics.
pub(super) struct Credential {
    pub token: String,
    pub account_id: Option<String>,
    pub plan: Option<String>,
    pub expires_at: Option<i64>,
}

impl Credential {
    pub fn fingerprint(&self) -> String {
        digest(&format!(
            "{}\0{}",
            self.token,
            self.account_id.as_deref().unwrap_or_default()
        ))
    }

    pub fn is_expired(&self) -> bool {
        self.expires_at
            .is_some_and(|expires| expires <= now_timestamp())
    }
}

pub(super) fn load(
    provider: Provider,
    interactive: bool,
) -> Result<Option<Credential>, UsageFailure> {
    let home = directories::BaseDirs::new()
        .ok_or_else(|| UsageFailure::new("No se encontró la carpeta de usuario."))?;
    match provider {
        Provider::Claude => {
            let custom = std::env::var("CLAUDE_CONFIG_DIR")
                .ok()
                .filter(|value| !value.is_empty());
            let directory = custom
                .as_ref()
                .map(PathBuf::from)
                .unwrap_or_else(|| home.home_dir().join(".claude"));
            let service = custom.map_or_else(
                || "Claude Code-credentials".to_owned(),
                |path| format!("Claude Code-credentials-{}", &digest(&path)[..8]),
            );
            let account = std::env::var("USER").ok();
            let data = match keychain_json(&service, account.as_deref(), interactive)? {
                Some(data) => Some(data),
                None => match keychain_json(&service, None, interactive)? {
                    Some(data) => Some(data),
                    None => read_json(&directory.join(".credentials.json"))?,
                },
            };
            data.map(|value| parse_claude(&value)).transpose()
        }
        Provider::Codex => {
            let directory = config_dir("CODEX_HOME", home.home_dir(), ".codex");
            let data = match read_json(&directory.join("auth.json"))? {
                Some(data) => Some(data),
                None => {
                    let canonical = directory.canonicalize().unwrap_or(directory);
                    let account = format!("cli|{}", &digest(&canonical.to_string_lossy())[..16]);
                    keychain_json("Codex Auth", Some(&account), interactive)?
                }
            };
            data.map(|value| parse_codex(&value)).transpose()
        }
        Provider::Grok => {
            let directory = config_dir("GROK_HOME", home.home_dir(), ".grok");
            read_json(&directory.join("auth.json"))?
                .map(|value| parse_grok(&value))
                .transpose()
        }
    }
}

fn config_dir(variable: &str, home: &Path, default: &str) -> PathBuf {
    std::env::var_os(variable)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(default))
}

fn read_json(path: &Path) -> Result<Option<Value>, UsageFailure> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(UsageFailure::new(
                "No se pudo leer la sesión local del CLI.",
            ));
        }
    };
    let mut bytes = Vec::new();
    file.take(1_048_577)
        .read_to_end(&mut bytes)
        .map_err(|_| UsageFailure::new("No se pudo leer la sesión local del CLI."))?;
    if bytes.len() > 1_048_576 {
        return Err(invalid_auth());
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| invalid_auth())
}

fn keychain_json(
    service: &str,
    account: Option<&str>,
    interactive: bool,
) -> Result<Option<Value>, UsageFailure> {
    unsafe extern "C" {
        fn vibra_copy_usage_credential(
            service: *const std::ffi::c_char,
            account: *const std::ffi::c_char,
            interactive: bool,
            length: *mut usize,
            status: *mut i32,
        ) -> *mut u8;
    }
    let service = CString::new(service).map_err(|_| invalid_auth())?;
    let account = account
        .map(CString::new)
        .transpose()
        .map_err(|_| invalid_auth())?;
    let mut length = 0;
    let mut status = 0;
    // SAFETY: C strings and out-parameters remain alive for the bridge call.
    let bytes = unsafe {
        vibra_copy_usage_credential(
            service.as_ptr(),
            account
                .as_ref()
                .map_or(std::ptr::null(), |account| account.as_ptr()),
            interactive,
            &mut length,
            &mut status,
        )
    };
    if bytes.is_null() {
        return match status {
            -25300 => Ok(None), // errSecItemNotFound
            -25308 | -25293 | -128 => Err(UsageFailure::new(
                "Pulsa Actualizar para autorizar el acceso al llavero de macOS.",
            )),
            _ => Err(UsageFailure::new(
                "No se pudo leer la sesión del llavero de macOS.",
            )),
        };
    }
    // SAFETY: bridge allocated `length` bytes; release once after parsing.
    let result = serde_json::from_slice(unsafe { std::slice::from_raw_parts(bytes, length) });
    unsafe { libc::free(bytes.cast()) };
    result.map(Some).map_err(|_| invalid_auth())
}

fn parse_claude(value: &Value) -> Result<Credential, UsageFailure> {
    let oauth = &value["claudeAiOauth"];
    if let Some(scopes) = oauth["scopes"].as_array()
        && !scopes
            .iter()
            .any(|scope| scope.as_str() == Some("user:profile"))
    {
        return Err(UsageFailure::new(
            "Esta sesión de Claude no permite leer cuotas. Inicia sesión con claude auth login.",
        ));
    }
    let token = token(&oauth["accessToken"])?;
    Ok(Credential {
        expires_at: oauth["expiresAt"]
            .as_i64()
            .map(|ms| ms / 1000)
            .or_else(|| token_expiry(&token)),
        token,
        account_id: None,
        plan: oauth["subscriptionType"].as_str().map(str::to_owned),
    })
}

fn parse_codex(value: &Value) -> Result<Credential, UsageFailure> {
    if value["auth_mode"].as_str() == Some("apikey")
        || value["OPENAI_API_KEY"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    {
        return Err(UsageFailure::new(
            "Las cuotas de Codex requieren una sesión de ChatGPT. Ejecuta codex login.",
        ));
    }
    let token = token(&value["tokens"]["access_token"])?;
    Ok(Credential {
        expires_at: token_expiry(&token),
        token,
        account_id: value["tokens"]["account_id"].as_str().map(str::to_owned),
        plan: None,
    })
}

fn parse_grok(value: &Value) -> Result<Credential, UsageFailure> {
    // Only the production CLI login; do not pick an arbitrary account/issuer.
    let entry = value
        .as_object()
        .ok_or_else(invalid_auth)?
        .iter()
        .find(|(key, _)| key.as_str() == "https://auth.x.ai::b1a00492-073a-47ea-816f-4c329264a828")
        .map(|(_, entry)| entry)
        .ok_or_else(|| {
            UsageFailure::new("No se encontró la sesión de Grok Build. Ejecuta grok login.")
        })?;
    let token = token(&entry["key"])?;
    Ok(Credential {
        expires_at: token_expiry(&token)
            .or_else(|| entry["expires_at"].as_str().and_then(timestamp)),
        token,
        account_id: entry["user_id"].as_str().map(str::to_owned),
        plan: None,
    })
}

fn token(value: &Value) -> Result<String, UsageFailure> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.chars().any(char::is_control))
        .map(str::to_owned)
        .ok_or_else(invalid_auth)
}

fn token_expiry(token: &str) -> Option<i64> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value["exp"].as_i64()
}

fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn invalid_auth() -> UsageFailure {
    UsageFailure::new("La sesión local no es válida. Vuelve a iniciar sesión en el CLI.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn credentials_are_typed_and_fingerprints_change_on_account_switch() {
        let mut first =
            parse_codex(&json!({"tokens":{"access_token":"secret", "account_id":"a"}})).unwrap();
        let old = first.fingerprint();
        first.account_id = Some("b".into());
        assert_ne!(old, first.fingerprint());
        assert!(!first.fingerprint().contains("secret"));
        assert!(
            parse_codex(&json!({"OPENAI_API_KEY":"sk-test","tokens":{"access_token":"old"}}))
                .is_err()
        );
        assert!(
            parse_claude(
                &json!({"claudeAiOauth":{"accessToken":"secret","scopes":["user:inference"]}})
            )
            .is_err()
        );
        let claude = parse_claude(
            &json!({"claudeAiOauth":{"accessToken":"secret","expiresAt":1900000000000i64}}),
        )
        .unwrap();
        assert_eq!(claude.expires_at, Some(1900000000));
    }

    #[test]
    fn grok_uses_the_cli_account_and_auth_errors_never_include_secrets() {
        let data = json!({
            "https://other.test::client":{"key":"wrong"},
            "https://auth.x.ai::b1a00492-073a-47ea-816f-4c329264a828":{"key":"correct","expires_at":"2030-01-01T00:00:00Z"}
        });
        assert_eq!(parse_grok(&data).unwrap().token, "correct");
        let error = token(&json!("secret\nheader")).err().unwrap();
        assert!(!error.message.contains("secret"));
    }
}
