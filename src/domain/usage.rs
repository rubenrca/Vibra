//! Subscription quota snapshots. Missing values are never treated as zero usage.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct UsageSnapshot {
    pub providers: BTreeMap<String, ProviderUsage>,
    #[serde(default)]
    pub errors: Vec<UsageError>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageError {
    pub provider_id: String,
    pub message: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsage {
    pub display_name: String,
    pub plan: Option<String>,
    pub fetched_at: Option<String>,
    pub expires_at: Option<String>,
    #[serde(default)]
    pub stale: bool,
    pub resources: BTreeMap<String, UsageResource>,
}

impl ProviderUsage {
    /// The most consumed bounded quota is the one closest to blocking work.
    pub fn tightest_quota(&self) -> Option<(&str, &UsageResource)> {
        self.resources
            .iter()
            .filter_map(|(key, resource)| {
                resource
                    .percent_used()
                    .map(|percent| (key, resource, percent))
            })
            .max_by(|(_, _, left), (_, _, right)| left.total_cmp(right))
            .map(|(key, resource, _)| (key.as_str(), resource))
    }

    pub fn is_stale(&self, now: i64) -> bool {
        self.stale
            || self
                .expires_at
                .as_deref()
                .and_then(timestamp)
                .is_some_and(|expires| expires <= now)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageResource {
    pub kind: String,
    pub unit: String,
    pub used: Option<f64>,
    pub limit: Option<f64>,
    pub utilization: Option<f64>,
    pub available: Option<f64>,
    pub resets_at: Option<String>,
}

impl UsageResource {
    pub fn percent_used(&self) -> Option<f64> {
        if self.kind != "consumption" {
            return None;
        }
        let percent = match (self.used, self.limit) {
            (Some(used), Some(limit)) if limit > 0.0 => used / limit * 100.0,
            _ => self.utilization? * 100.0,
        };
        (percent.is_finite() && percent >= 0.0).then_some(percent)
    }

    pub fn value_label(&self) -> String {
        if self.kind == "balance" {
            return self.available.map_or_else(
                || "No data".to_owned(),
                |available| format!("{} available", self.format_value(available)),
            );
        }
        if let Some(percent) = self.percent_used() {
            if self.unit == "percent" {
                return format!("{percent:.0}% used");
            }
            if let (Some(used), Some(limit)) = (self.used, self.limit) {
                return format!("{} / {}", self.format_value(used), self.format_value(limit));
            }
            return format!("{percent:.0}% used");
        }
        self.used.map_or_else(
            || "No data".to_owned(),
            |used| format!("{} used", self.format_value(used)),
        )
    }

    fn format_value(&self, value: f64) -> String {
        match self.unit.as_str() {
            "usd" => format!("US${value:.2}"),
            "percent" => format!("{value:.0}%"),
            "credits" => format!("{value:.0} credits"),
            "requests" => format!("{value:.0} requests"),
            "resets" => format!("{value:.0} resets"),
            unit => format!("{value:.0} {unit}"),
        }
    }
}

pub fn resource_label(key: &str) -> &str {
    match key {
        "session" => "Session",
        "weekly" => "Week",
        "monthly" => "Month",
        "daily" => "Day",
        "totalUsage" => "Total usage",
        "extraUsage" => "Extra usage",
        "credits" => "Credits",
        "creditValue" | "balance" => "Balance",
        "rateLimitResets" => "Quota resets",
        "premiumCredits" => "Premium credits",
        "onDemand" => "On demand",
        "requests" => "Requests",
        "sonnet" => "Sonnet",
        "opus" => "Opus",
        "fable" => "Fable",
        "spark" => "Spark · session",
        "sparkWeekly" => "Spark · week",
        _ => key,
    }
}

pub fn now_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub fn timestamp(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|date| date.timestamp())
}

pub fn reset_label(value: &str, now: i64) -> String {
    let Some(reset) = timestamp(value) else {
        return "Reset date unavailable".to_owned();
    };
    let seconds = reset.saturating_sub(now);
    if seconds <= 0 {
        return "Reset pending refresh".to_owned();
    }
    let minutes = (seconds + 59) / 60;
    if minutes < 60 {
        format!("Resets in {minutes} min")
    } else if minutes < 24 * 60 {
        format!("Resets in {} h {} min", minutes / 60, minutes % 60)
    } else {
        format!(
            "Resets in {} d {} h",
            minutes / (24 * 60),
            minutes / 60 % 24
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn quotas_preserve_overage_and_do_not_turn_balances_into_usage() {
        let resource: UsageResource = serde_json::from_value(json!({
            "kind": "consumption",
            "unit": "usd",
            "used": 120,
            "limit": 100
        }))
        .unwrap();
        assert_eq!(resource.percent_used(), Some(120.0));
        assert_eq!(resource.value_label(), "US$120.00 / US$100.00");
        let resource: UsageResource = serde_json::from_value(json!({
            "kind": "balance",
            "unit": "credits",
            "available": 0
        }))
        .unwrap();
        assert_eq!(resource.percent_used(), None);
        assert_eq!(resource.value_label(), "0 credits available");
        let resource: UsageResource = serde_json::from_value(json!({
            "kind": "consumption",
            "unit": "percent"
        }))
        .unwrap();
        assert_eq!(resource.percent_used(), None);
        assert_eq!(resource.value_label(), "No data");
    }

    #[test]
    fn tightest_window_and_expiry_follow_the_provider_data() {
        let provider: ProviderUsage = serde_json::from_value(json!({
            "displayName": "Codex",
            "expiresAt": "2026-09-26T12:05:00Z",
            "resources": {
                "session": {
                    "kind": "consumption",
                    "unit": "percent",
                    "used": 30,
                    "limit": 100
                },
                "weekly": {
                    "kind": "consumption",
                    "unit": "percent",
                    "utilization": 0.9
                },
                "credits": {
                    "kind": "balance",
                    "unit": "credits",
                    "available": 1000
                }
            }
        }))
        .unwrap();
        assert_eq!(provider.tightest_quota().unwrap().0, "weekly");
        let now = timestamp("2026-09-26T12:00:00Z").unwrap();
        assert!(!provider.is_stale(now));
        assert!(provider.is_stale(now + 300));
        assert_eq!(
            reset_label("2026-09-26T10:30:00-03:00", now),
            "Resets in 1 h 30 min"
        );
        assert_eq!(
            reset_label("2026-09-26T12:00:00Z", now),
            "Reset pending refresh"
        );
    }
}
