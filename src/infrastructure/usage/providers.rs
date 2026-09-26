//! Translate each provider's subscription response into Vibra's quota model.

use std::collections::BTreeMap;

use serde_json::Value;

use super::{Provider, UsageFailure, auth::Credential, http};
use crate::domain::usage::{ProviderUsage, UsageResource, timestamp};

pub(super) const CACHE_SECONDS: i64 = 300;

pub(super) fn fetch(
    provider: Provider,
    credential: &Credential,
    now: i64,
) -> Result<ProviderUsage, UsageFailure> {
    if credential.is_expired() {
        return Err(UsageFailure::new(format!(
            "La sesión venció. Abre {} para renovarla y pulsa Actualizar.",
            provider.name()
        )));
    }
    let authorization = format!("Bearer {}", credential.token);
    let mut headers = vec![("Authorization", authorization.as_str())];
    let url = match provider {
        Provider::Claude => {
            headers.push(("anthropic-beta", "oauth-2025-04-20"));
            "https://api.anthropic.com/api/oauth/usage"
        }
        Provider::Codex => {
            if let Some(account) = &credential.account_id {
                headers.push(("ChatGPT-Account-Id", account));
            }
            "https://chatgpt.com/backend-api/wham/usage"
        }
        Provider::Grok => {
            headers.push(("X-XAI-Token-Auth", "xai-grok-cli"));
            "https://cli-chat-proxy.grok.com/v1/billing?format=credits"
        }
    };
    let value = http::get_json(url, &headers)?;
    let mut usage = match provider {
        Provider::Claude => claude(&value, now)?,
        Provider::Codex => codex(&value, now)?,
        Provider::Grok => grok(&value, now)?,
    };
    if usage.plan.is_none() {
        usage.plan = credential.plan.clone();
    }
    Ok(usage)
}

pub(super) fn empty_provider(provider: Provider) -> ProviderUsage {
    ProviderUsage {
        display_name: provider.name().into(),
        plan: None,
        fetched_at: None,
        expires_at: None,
        stale: true,
        resources: BTreeMap::new(),
    }
}

fn snapshot(provider: Provider, now: i64) -> ProviderUsage {
    ProviderUsage {
        fetched_at: iso(now),
        expires_at: iso(now + CACHE_SECONDS),
        stale: false,
        ..empty_provider(provider)
    }
}

fn claude(body: &Value, now: i64) -> Result<ProviderUsage, UsageFailure> {
    if !["five_hour", "seven_day", "limits", "extra_usage"]
        .iter()
        .any(|key| body.get(key).is_some())
    {
        return Err(UsageFailure::invalid_response());
    }
    let mut usage = snapshot(Provider::Claude, now);
    for (key, source) in [
        ("session", "five_hour"),
        ("weekly", "seven_day"),
        ("sonnet", "seven_day_sonnet"),
        ("opus", "seven_day_opus"),
    ] {
        if let Some(percent) = optional_number(&body[source]["utilization"])? {
            usage.resources.insert(
                key.into(),
                quota(
                    percent,
                    body[source]["resets_at"].as_str().map(str::to_owned),
                )?,
            );
        }
    }
    if let Some(limits) = body["limits"].as_array() {
        for limit in limits {
            if limit["kind"].as_str() != Some("weekly_scoped") {
                continue;
            }
            if let (Some(name), Some(percent)) = (
                limit["scope"]["model"]["display_name"].as_str(),
                optional_number(&limit["percent"])?,
            ) {
                let key = match name.to_lowercase().as_str() {
                    "sonnet" => "sonnet".to_owned(),
                    "opus" => "opus".to_owned(),
                    "fable" => "fable".to_owned(),
                    _ => format!("{name} · semana"),
                };
                usage.resources.insert(
                    key,
                    quota(percent, limit["resets_at"].as_str().map(str::to_owned))?,
                );
            }
        }
    }
    let extra = &body["extra_usage"];
    if extra["is_enabled"] == true
        && let Some(used) = optional_number(&extra["used_credits"])?
    {
        usage.resources.insert(
            "extraUsage".into(),
            UsageResource {
                kind: "consumption".into(),
                unit: "usd".into(),
                used: Some(used / 100.0),
                limit: optional_number(&extra["monthly_limit"])?
                    .filter(|limit| *limit > 0.0)
                    .map(|limit| limit / 100.0),
                utilization: None,
                available: None,
                resets_at: None,
            },
        );
    }
    Ok(usage)
}

fn codex(body: &Value, now: i64) -> Result<ProviderUsage, UsageFailure> {
    if body.get("rate_limit").is_none() && body.get("credits").is_none() {
        return Err(UsageFailure::invalid_response());
    }
    let mut usage = snapshot(Provider::Codex, now);
    usage.plan = body["plan_type"].as_str().map(str::to_owned);
    codex_windows(&mut usage, &body["rate_limit"], "", now)?;
    if let Some(additional) = body["additional_rate_limits"].as_array() {
        for limit in additional {
            let Some(name) = limit["limit_name"]
                .as_str()
                .or_else(|| limit["metered_feature"].as_str())
            else {
                continue;
            };
            codex_windows(&mut usage, &limit["rate_limit"], name, now)?;
        }
    }
    if let Some(balance) = optional_number(&body["credits"]["balance"])? {
        usage
            .resources
            .insert("credits".into(), balance_resource(balance, "credits"));
    } else if body["credits"]["has_credits"] == false {
        usage
            .resources
            .insert("credits".into(), balance_resource(0.0, "credits"));
    }
    if let Some(count) = optional_number(&body["rate_limit_reset_credits"]["available_count"])? {
        usage
            .resources
            .insert("rateLimitResets".into(), balance_resource(count, "resets"));
    }
    Ok(usage)
}

fn codex_windows(
    usage: &mut ProviderUsage,
    limits: &Value,
    prefix: &str,
    now: i64,
) -> Result<(), UsageFailure> {
    for (slot, fallback) in [
        ("primary_window", "session"),
        ("secondary_window", "weekly"),
    ] {
        let window = &limits[slot];
        let Some(percent) = optional_number(&window["used_percent"])? else {
            continue;
        };
        let duration = optional_number(&window["limit_window_seconds"])?;
        let key = match duration {
            Some(604800.0) => "weekly".to_owned(),
            Some(86400.0) => "daily".to_owned(),
            Some(seconds) if seconds > 0.0 && seconds <= 18000.0 => "session".to_owned(),
            Some(seconds) if seconds > 0.0 => format!("Ventana de {:.0} h", seconds / 3600.0),
            _ => fallback.into(),
        };
        let key = if prefix.is_empty() {
            key
        } else {
            format!("{prefix} · {}", crate::domain::usage::resource_label(&key))
        };
        let reset = window["reset_at"].as_i64().or_else(|| {
            window["reset_after_seconds"]
                .as_i64()
                .map(|after| now.saturating_add(after))
        });
        usage
            .resources
            .insert(key, quota(percent, reset.and_then(iso))?);
    }
    Ok(())
}

fn grok(body: &Value, now: i64) -> Result<ProviderUsage, UsageFailure> {
    let config = &body["config"];
    let period = &config["currentPeriod"];
    let key = match period["type"].as_str() {
        Some("USAGE_PERIOD_TYPE_WEEKLY") => "weekly",
        Some("USAGE_PERIOD_TYPE_MONTHLY") => "monthly",
        Some("USAGE_PERIOD_TYPE_DAILY") => "daily",
        _ => return Err(UsageFailure::invalid_response()),
    };
    let start = period["start"]
        .as_str()
        .and_then(timestamp)
        .ok_or_else(UsageFailure::invalid_response)?;
    let end = period["end"]
        .as_str()
        .and_then(timestamp)
        .ok_or_else(UsageFailure::invalid_response)?;
    if end <= start {
        return Err(UsageFailure::invalid_response());
    }
    // Proto3 omits scalar fields at zero. This default is specific to this API.
    let percent = match config.get("creditUsagePercent") {
        None => 0.0,
        Some(value) => optional_number(value)?.ok_or_else(UsageFailure::invalid_response)?,
    };
    let mut usage = snapshot(Provider::Grok, now);
    usage
        .resources
        .insert(key.into(), quota(percent, iso(end))?);
    Ok(usage)
}

fn optional_number(value: &Value) -> Result<Option<f64>, UsageFailure> {
    if value.is_null() {
        return Ok(None);
    }
    let number = value
        .as_f64()
        .or_else(|| value.as_str().and_then(|value| value.parse::<f64>().ok()))
        .filter(|value| value.is_finite() && *value >= 0.0)
        .ok_or_else(UsageFailure::invalid_response)?;
    Ok(Some(number))
}

fn quota(percent: f64, resets_at: Option<String>) -> Result<UsageResource, UsageFailure> {
    if !percent.is_finite() || percent < 0.0 {
        return Err(UsageFailure::invalid_response());
    }
    Ok(UsageResource {
        kind: "consumption".into(),
        unit: "percent".into(),
        used: Some(percent),
        limit: Some(100.0),
        utilization: None,
        available: None,
        resets_at,
    })
}

fn balance_resource(available: f64, unit: &str) -> UsageResource {
    UsageResource {
        kind: "balance".into(),
        unit: unit.into(),
        used: None,
        limit: None,
        utilization: None,
        available: Some(available),
        resets_at: None,
    }
}

fn iso(value: i64) -> Option<String> {
    chrono::DateTime::from_timestamp(value, 0)
        .map(|date| date.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn claude_maps_windows_scoped_models_and_cents_without_inventing_limits() {
        let usage = claude(&json!({
            "five_hour":{"utilization":20,"resets_at":"2026-09-27T00:00:00Z"},
            "seven_day":null,
            "limits":[{"kind":"weekly_scoped","percent":75,"scope":{"model":{"display_name":"Fable"}}}],
            "extra_usage":{"is_enabled":true,"used_credits":1250,"monthly_limit":5000}
        }), 1_800_000_000).unwrap();
        assert_eq!(usage.resources["session"].percent_used(), Some(20.0));
        assert!(!usage.resources.contains_key("weekly"));
        assert_eq!(usage.resources["fable"].percent_used(), Some(75.0));
        assert_eq!(usage.resources["extraUsage"].used, Some(12.5));
        assert_eq!(usage.resources["extraUsage"].limit, Some(50.0));
        assert!(claude(&json!({"error":"invalid"}), 0).is_err());
    }

    #[test]
    fn codex_classifies_a_weekly_primary_and_preserves_zero_credits() {
        let usage = codex(&json!({
            "plan_type":"pro", "rate_limit":{"primary_window":{"used_percent":71,"limit_window_seconds":604800,"reset_at":1800000010}},
            "credits":{"balance":"0"}, "rate_limit_reset_credits":{"available_count":2},
            "additional_rate_limits":[{"limit_name":"Spark","rate_limit":{"primary_window":{"used_percent":5,"limit_window_seconds":18000}}}]
        }), 1800000000).unwrap();
        assert!(!usage.resources.contains_key("session"));
        assert_eq!(usage.resources["weekly"].percent_used(), Some(71.0));
        assert_eq!(usage.resources["credits"].available, Some(0.0));
        assert_eq!(usage.resources["rateLimitResets"].available, Some(2.0));
        assert_eq!(usage.resources["Spark · Sesión"].percent_used(), Some(5.0));
    }

    #[test]
    fn grok_distinguishes_proto_zero_from_invalid_data_and_monthly_from_weekly() {
        let mut body = json!({"config":{"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY","start":"2026-09-20T00:00:00Z","end":"2026-09-27T00:00:00Z"}}});
        assert_eq!(
            grok(&body, 0).unwrap().resources["weekly"].percent_used(),
            Some(0.0)
        );
        body["config"]["currentPeriod"]["type"] = json!("USAGE_PERIOD_TYPE_MONTHLY");
        assert!(grok(&body, 0).unwrap().resources.contains_key("monthly"));
        body["config"]["creditUsagePercent"] = json!("bad");
        assert!(grok(&body, 0).is_err());
        body["config"]["creditUsagePercent"] = Value::Null;
        assert!(grok(&body, 0).is_err());
        assert!(grok(&json!({}), 0).is_err());
    }
}
