//! Cancellation-independent, single-flight machine token refresh.
use super::{bounded, DocumentError, REQUEST_TIMEOUT};
use reqwest::Client;
use serde::Deserialize;
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::{watch, Mutex},
    time::Instant,
};
use zeroize::Zeroizing;

const REFRESH_BACKOFF_MIN: Duration = Duration::from_secs(2);
const REFRESH_BACKOFF_MAX: Duration = Duration::from_secs(30);
const REFRESH_AHEAD_FRACTION: f64 = 0.2;
const EXPIRY_MARGIN: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TokenGeneration(u64);

type TokenResult = Result<(Zeroizing<String>, TokenGeneration), DocumentError>;
type Refresh = watch::Receiver<Option<TokenResult>>;

#[derive(Clone)]
pub(crate) struct StorageToken {
    http: Client,
    endpoint: url::Url,
    client_id: Arc<str>,
    client_secret: Arc<Zeroizing<String>>,
    scope: &'static str,
    pub(super) state: Arc<Mutex<State>>,
    pub(super) jitter: fn(Duration) -> Duration,
}

pub(super) struct State {
    pub(super) cached: Option<CachedToken>,
    flight: Option<Refresh>,
    pub(super) retry_at: Instant,
    failures: u32,
    generation: u64,
}

pub(super) struct CachedToken {
    value: Zeroizing<String>,
    generation: TokenGeneration,
    expires: Instant,
    lifetime: Duration,
}

impl StorageToken {
    pub(crate) fn new(
        http: Client,
        mut origin: url::Url,
        client_id: String,
        client_secret: Zeroizing<String>,
        scope: &'static str,
    ) -> Self {
        origin.set_path("/oauth/token");
        Self {
            http,
            endpoint: origin,
            client_id: client_id.into(),
            client_secret: Arc::new(client_secret),
            scope,
            state: Arc::new(Mutex::new(State {
                cached: None,
                flight: None,
                retry_at: Instant::now(),
                failures: 0,
                generation: 0,
            })),
            jitter,
        }
    }

    /// The enclosing object operation bounds each caller's lock and flight wait.
    pub(crate) async fn current(&self, rejected: Option<TokenGeneration>) -> TokenResult {
        let mut state = self.state.lock().await;
        if state
            .cached
            .as_ref()
            .is_some_and(|token| Some(token.generation) == rejected)
        {
            state.cached = None;
        }
        let now = Instant::now();
        if let Some(token) = &state.cached {
            if token.expires > now + EXPIRY_MARGIN {
                let value = (token.value.clone(), token.generation);
                let ahead = token.expires - now < token.lifetime.mul_f64(REFRESH_AHEAD_FRACTION);
                if ahead && state.flight.is_none() && now >= state.retry_at {
                    self.start(&mut state);
                }
                return Ok(value);
            }
        }
        if state.flight.is_none() {
            if now < state.retry_at {
                return Err(DocumentError::Unavailable);
            }
            self.start(&mut state);
        }
        let mut refresh = state.flight.as_ref().unwrap().clone();
        drop(state);
        let result = refresh
            .wait_for(|result| result.is_some())
            .await
            .map_err(|_| DocumentError::Unavailable)?;
        result.as_ref().unwrap().clone()
    }

    fn start(&self, state: &mut State) {
        let (sender, receiver) = watch::channel(None);
        state.flight = Some(receiver);
        let token = self.clone();
        // No caller owns this task. In particular, cancellation of the first
        // waiter must neither cancel HTTP nor let another POST start in parallel.
        tokio::spawn(async move {
            let result = tokio::time::timeout(REQUEST_TIMEOUT, token.exchange())
                .await
                .unwrap_or(Err(DocumentError::Unavailable));
            let mut state = token.state.lock().await;
            let result = result.and_then(|(value, lifetime)| {
                state.generation = state
                    .generation
                    .checked_add(1)
                    .ok_or(DocumentError::Unavailable)?;
                let generation = TokenGeneration(state.generation);
                state.cached = Some(CachedToken {
                    value: value.clone(),
                    generation,
                    expires: Instant::now() + lifetime,
                    lifetime,
                });
                Ok((value, generation))
            });
            if result.is_ok() {
                state.failures = 0;
                state.retry_at = Instant::now();
            } else {
                state.failures = state.failures.saturating_add(1).min(5);
                let delay =
                    (REFRESH_BACKOFF_MIN * (1 << (state.failures - 1))).min(REFRESH_BACKOFF_MAX);
                state.retry_at = Instant::now() + (token.jitter)(delay);
            }
            state.flight = None;
            sender.send_replace(Some(result));
        });
    }

    async fn exchange(&self) -> Result<(Zeroizing<String>, Duration), DocumentError> {
        let response = self
            .http
            .post(self.endpoint.clone())
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", self.client_id.as_ref()),
                ("client_secret", self.client_secret.as_str()),
                ("scope", self.scope),
            ])
            .send()
            .await
            .map_err(|_| DocumentError::Unavailable)?;
        if !response.status().is_success() {
            return Err(DocumentError::Unavailable);
        }
        let bytes = bounded(response, 16 * 1024).await?;
        let auth: TokenReply =
            serde_json::from_slice(&bytes).map_err(|_| DocumentError::Unavailable)?;
        if auth.access_token.is_empty()
            || auth.access_token.len() > 16 * 1024
            || !(120..=86_400).contains(&auth.expires_in)
        {
            return Err(DocumentError::Unavailable);
        }
        Ok((
            Zeroizing::new(auth.access_token),
            Duration::from_secs(auth.expires_in),
        ))
    }
}

#[derive(Deserialize)]
struct TokenReply {
    access_token: String,
    expires_in: u64,
}

// Match background work's equal jitter: [75%, 100%] of the bounded delay.
fn jitter(delay: Duration) -> Duration {
    let millis = delay.as_millis() as u64;
    let spread = millis / 4;
    let random = uuid::Uuid::new_v4().as_u128() as u64;
    Duration::from_millis(millis - spread + random % (spread + 1))
}
