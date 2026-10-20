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

    /// `value_label` without the words that only matter in a sentence:
    /// "84%" rather than "84% used", "1000 credits" rather than "… available".
    pub fn compact_value_label(&self) -> String {
        let value = self.value_label();
        value
            .strip_suffix(" used")
            .filter(|_| self.percent_used().is_some())
            .or_else(|| value.strip_suffix(" available"))
            .unwrap_or(&value)
            .to_owned()
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
        // Claude and Codex both meter their session in five-hour windows.
        "session" => "5 hours",
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

/// When a quota resets, in the viewer's time zone (`utc_offset` seconds east
/// of UTC): the time of day when it is today, the weekday within the next
/// week, and the date after that.
pub fn reset_label(value: &str, now: i64, utc_offset: i32) -> String {
    let local = |seconds: i64| {
        let offset = chrono::FixedOffset::east_opt(utc_offset)?;
        Some(chrono::DateTime::from_timestamp(seconds, 0)?.with_timezone(&offset))
    };
    let reset = timestamp(value);
    let (Some(reset), Some(reset_local), Some(now_local)) =
        (reset, reset.and_then(local), local(now))
    else {
        return "reset date unavailable".to_owned();
    };
    if reset <= now {
        return "reset pending refresh".to_owned();
    }
    let days = reset_local
        .date_naive()
        .signed_duration_since(now_local.date_naive())
        .num_days();
    match days {
        0 => format!("resets {}", reset_local.format("%-I:%M %p")),
        1..=6 => format!("resets {}", reset_local.format("%a")),
        _ => format!("resets {}", reset_local.format("%b %-d")),
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
    }

    #[test]
    fn reset_reads_as_a_time_today_a_weekday_this_week_or_a_date() {
        // Saturday 2026-09-26, 09:00 in UTC-3.
        let now = timestamp("2026-09-26T12:00:00Z").unwrap();
        let west = -3 * 3600;
        assert_eq!(
            reset_label("2026-09-26T10:30:00-03:00", now, west),
            "resets 10:30 AM"
        );
        // Already tomorrow in UTC, still today at the viewer's wall clock.
        assert_eq!(
            reset_label("2026-09-27T02:00:00Z", now, west),
            "resets 11:00 PM"
        );
        assert_eq!(reset_label("2026-09-27T12:00:00Z", now, west), "resets Sun");
        assert_eq!(reset_label("2026-09-29T15:00:00Z", now, west), "resets Tue");
        assert_eq!(
            reset_label("2026-10-03T15:00:00Z", now, west),
            "resets Oct 3"
        );
        assert_eq!(
            reset_label("2026-09-26T12:00:00Z", now, west),
            "reset pending refresh"
        );
        assert_eq!(
            reset_label("not a date", now, west),
            "reset date unavailable"
        );
    }

    #[test]
    fn compact_values_drop_sentence_words_only() {
        let value =
            |json: serde_json::Value| -> UsageResource { serde_json::from_value(json).unwrap() };
        assert_eq!(
            value(json!({"kind": "consumption", "unit": "percent", "used": 84, "limit": 100}))
                .compact_value_label(),
            "84%"
        );
        assert_eq!(
            value(json!({"kind": "consumption", "unit": "usd", "used": 12, "limit": 100}))
                .compact_value_label(),
            "US$12.00 / US$100.00"
        );
        assert_eq!(
            value(json!({"kind": "balance", "unit": "credits", "available": 40}))
                .compact_value_label(),
            "40 credits"
        );
    }
}
