//! Vibra's subscription monitor: current CLI credentials, direct provider reads,
//! isolated caches and bounded retries. When the stored login has expired, the
//! provider's CLI is asked instead. No companion usage app or local daemon.

mod auth;
mod cli;
mod http;
mod providers;

use std::collections::BTreeMap;

use crate::domain::usage::{ProviderUsage, UsageError, UsageSnapshot, now_timestamp};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Provider {
    Claude,
    Codex,
    Grok,
}

impl Provider {
    const ALL: [Self; 3] = [Self::Claude, Self::Codex, Self::Grok];
    fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Grok => "grok",
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::Grok => "Grok",
        }
    }
}

#[derive(Debug)]
struct UsageFailure {
    message: String,
    retry_after: Option<u64>,
    /// The stored login is expired or was rejected; only the CLI can renew it.
    session: bool,
}

impl UsageFailure {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retry_after: None,
            session: false,
        }
    }
    fn session(message: impl Into<String>) -> Self {
        Self {
            session: true,
            ..Self::new(message)
        }
    }
    fn rate_limited(retry_after: u64) -> Self {
        Self {
            retry_after: Some(retry_after),
            ..Self::new("The service rate-limited requests. Vibra will retry later.")
        }
    }
    fn invalid_response() -> Self {
        Self::new("The service returned an unrecognized quota format.")
    }
    fn connection() -> Self {
        Self::new("Could not connect to the quota service. Retrying later.")
    }
}

#[derive(Default)]
struct CachedProvider {
    fingerprint: String,
    snapshot: Option<ProviderUsage>,
    error: Option<String>,
    next_poll: i64,
    manual_after: i64,
    failures: u32,
}

impl CachedProvider {
    fn prepare(&mut self, fingerprint: String, now: i64, manual: bool) -> bool {
        if self.fingerprint != fingerprint {
            *self = Self {
                fingerprint,
                ..Self::default()
            };
        }
        now >= self.next_poll || (manual && now >= self.manual_after)
    }

    fn apply(&mut self, result: Result<ProviderUsage, UsageFailure>, now: i64) {
        match result {
            Ok(snapshot) => {
                self.snapshot = Some(snapshot);
                self.error = None;
                self.failures = 0;
                self.next_poll = now + providers::CACHE_SECONDS;
                self.manual_after = now + 30;
            }
            Err(error) => {
                self.failures = self.failures.saturating_add(1);
                let delay = error
                    .retry_after
                    .unwrap_or(60 * (1_u64 << self.failures.min(4)));
                self.next_poll = now.saturating_add(delay.min(i64::MAX as u64) as i64);
                // A manual click never bypasses a provider's rate-limit cooldown.
                self.manual_after = if error.retry_after.is_some() {
                    self.next_poll
                } else {
                    now + 30
                };
                self.error = Some(error.message);
            }
        }
    }
}

#[derive(Default)]
pub struct UsageMonitor {
    cache: BTreeMap<Provider, CachedProvider>,
}

impl UsageMonitor {
    pub fn refresh(&mut self, manual: bool) -> UsageSnapshot {
        let mut result = UsageSnapshot::default();
        for provider in Provider::ALL {
            let credential = match auth::load(provider, manual) {
                Ok(Some(credential)) => credential,
                Ok(None) => {
                    self.cache.remove(&provider);
                    continue;
                }
                Err(error) => {
                    // Identity cannot be verified; never show a previous account's quota.
                    self.cache.remove(&provider);
                    result
                        .providers
                        .insert(provider.id().into(), providers::empty_provider(provider));
                    result.errors.push(UsageError {
                        provider_id: provider.id().into(),
                        message: error.message,
                    });
                    continue;
                }
            };
            let now = now_timestamp();
            let cached = self.cache.entry(provider).or_default();
            if cached.prepare(credential.fingerprint(), now, manual) {
                let response = providers::fetch(provider, &credential, now);
                // A login may have changed while the request was in flight.
                let current = auth::load(provider, false)
                    .map(|current| current.map(|current| current.fingerprint()));
                match (current, response) {
                    // The CLI answered for the login it has now, renewed or not.
                    (Ok(Some(current)), Ok(reading)) if reading.from_cli => {
                        cached.fingerprint = current;
                        cached.apply(Ok(reading.usage), now_timestamp())
                    }
                    (Ok(Some(current)), response) if current == credential.fingerprint() => {
                        cached.apply(response.map(|reading| reading.usage), now_timestamp())
                    }
                    (Ok(_), _) => {
                        self.cache.remove(&provider);
                        result.errors.push(UsageError {
                            provider_id: provider.id().into(),
                            message: "The session changed during the query. Retrying later.".into(),
                        });
                        continue;
                    }
                    (Err(error), _) => {
                        self.cache.remove(&provider);
                        result
                            .providers
                            .insert(provider.id().into(), providers::empty_provider(provider));
                        result.errors.push(UsageError {
                            provider_id: provider.id().into(),
                            message: error.message,
                        });
                        continue;
                    }
                }
            }
            let cached = &self.cache[&provider];
            let mut snapshot = cached
                .snapshot
                .clone()
                .unwrap_or_else(|| providers::empty_provider(provider));
            if let Some(error) = &cached.error {
                snapshot.stale = true;
                result.errors.push(UsageError {
                    provider_id: provider.id().into(),
                    message: error.clone(),
                });
            }
            result.providers.insert(provider.id().into(), snapshot);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_keeps_last_good_values_on_failure_but_not_after_account_changes() {
        let mut cache = CachedProvider::default();
        assert!(cache.prepare("account-a".into(), 100, false));
        cache.apply(Ok(providers::empty_provider(Provider::Claude)), 100);
        assert!(!cache.prepare("account-a".into(), 110, true));
        assert!(!cache.prepare("account-a".into(), 399, false));
        assert!(cache.prepare("account-a".into(), 400, false));
        cache.apply(Err(UsageFailure::connection()), 400);
        assert!(cache.snapshot.is_some());
        assert!(cache.error.is_some());
        assert!(cache.prepare("account-b".into(), 401, false));
        assert!(cache.snapshot.is_none());
        assert!(cache.error.is_none());
    }

    #[test]
    fn providers_have_independent_backoff_and_manual_refresh_respects_429() {
        let mut cache = CachedProvider::default();
        cache.prepare("a".into(), 100, false);
        cache.apply(Err(UsageFailure::rate_limited(3600)), 100);
        assert!(!cache.prepare("a".into(), 3699, true));
        assert!(cache.prepare("a".into(), 3700, false));
        let mut other = CachedProvider::default();
        assert!(other.prepare("b".into(), 101, false));
    }

    #[test]
    #[ignore = "Explicit opt-in: reads current CLI logins and contacts provider quota endpoints"]
    fn live_subscription_probe() {
        let snapshot = UsageMonitor::default().refresh(false);
        for (id, provider) in &snapshot.providers {
            println!("{id}: {} quota metrics", provider.resources.len());
        }
        for error in &snapshot.errors {
            println!("{}: {}", error.provider_id, error.message);
        }
        assert!(
            !snapshot.providers.is_empty(),
            "No supported CLI logins found"
        );
    }

    #[test]
    #[ignore = "Explicit opt-in: starts the installed CLIs, which contact their quota services"]
    fn live_expired_session_probe() {
        // An expired login is never sent anywhere: the CLI has to answer instead.
        let expired = auth::Credential {
            token: "expired".into(),
            account_id: None,
            plan: None,
            expires_at: Some(0),
        };
        let mut answered = 0;
        for provider in Provider::ALL {
            match providers::fetch(provider, &expired, now_timestamp()) {
                Ok(reading) => {
                    assert!(reading.from_cli);
                    answered += 1;
                    println!(
                        "{}: {} quota metrics",
                        provider.id(),
                        reading.usage.resources.len()
                    );
                }
                Err(error) => println!("{}: {}", provider.id(), error.message),
            }
        }
        assert!(answered > 0, "No CLI reported its quota");
    }
}
