//! Per-process negative provider backoff. Successful identity evidence is never cached.

use std::{collections::HashMap, sync::Arc, sync::Mutex};

use tokio::time::{timeout_at, Instant};

use super::{
    identity_flight::TokenHash, identity_throttle::RetryAfter, verification_diagnostics::Endpoint,
    verification_error::ProviderVerificationError as Error, AuthMode, AuthState,
};

const MAX_SCOPES: usize = 1024;
type Entry = tokio::sync::Mutex<Option<RetryAfter>>;

#[derive(Hash, PartialEq, Eq)]
struct Scope {
    provider: String,
    client: String,
    credential: TokenHash,
    endpoint: String,
    kind: &'static str,
}

pub(super) struct Cooldowns {
    entries: Mutex<HashMap<Scope, Arc<Entry>>>,
    capacity: usize,
}

impl Default for Cooldowns {
    fn default() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            capacity: MAX_SCOPES,
        }
    }
}

impl Cooldowns {
    fn acquire(&self, scope: Scope) -> Result<Arc<Entry>, Error> {
        let mut entries = self.entries.lock().expect("identity cooldown map");
        // Keep entries with a caller/probe or an unexpired provider backoff.
        // Never evict a live backoff just to admit another credential.
        entries.retain(|_, entry| {
            Arc::strong_count(entry) > 1
                || entry
                    .try_lock()
                    .is_ok_and(|hint| hint.is_some_and(|hint| !hint.remaining().is_zero()))
        });
        if let Some(entry) = entries.get(&scope) {
            return Ok(entry.clone());
        }
        if entries.len() >= self.capacity {
            return Err(Error::Unavailable(
                "identity cooldown capacity exhausted".into(),
            ));
        }
        let entry = Arc::new(Entry::new(None));
        entries.insert(scope, entry.clone());
        Ok(entry)
    }
}

pub(super) async fn json(
    auth: &AuthState,
    credential: &str,
    url: &str,
    endpoint: Endpoint,
    request: reqwest::RequestBuilder,
    deadline: Instant,
) -> Result<serde_json::Value, Error> {
    let AuthMode::Provider {
        base_url,
        client_id,
        ..
    } = &auth.mode
    else {
        return Err(Error::Unavailable(
            "no identity provider is configured".into(),
        ));
    };
    let entry = auth.identity_cooldowns.acquire(Scope {
        provider: base_url.clone(),
        client: client_id.clone(),
        credential: auth.identity_store.key(credential),
        endpoint: url.to_owned(),
        kind: endpoint.label(),
    })?;
    // Serialize the endpoint probe, including the first response. This also
    // protects callers arriving through different cookie/bearer flights.
    // Cancellation drops the guard; no task or retry outlives its caller.
    let mut hint = timeout_at(deadline, entry.lock())
        .await
        .map_err(|_| Error::Unavailable("identity cooldown admission deadline elapsed".into()))?;
    if let Some(backoff) = *hint {
        if !backoff.remaining().is_zero() {
            metrics::counter!("cma_identity_cooldown_hits_total", "endpoint" => endpoint.label())
                .increment(1);
            return Err(Error::RateLimited(backoff));
        }
    }
    // No completed success is retained or shared. Once backoff expires this
    // caller must obtain fresh evidence, even if its predecessor succeeded.
    *hint = None;
    let result = super::identity_http::json(request, deadline, endpoint).await;
    if let Err(Error::RateLimited(backoff)) = &result {
        *hint = Some(*backoff);
    }
    result
}

#[cfg(test)]
#[path = "identity_cooldown_tests.rs"]
mod tests;
