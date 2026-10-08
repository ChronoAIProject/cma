//! One bounded CMA restore shared by concurrent callers on each cold channel.
use super::cache::{CacheEntry, ChannelCache, Lookup};
use super::observer::ChannelIndexObserver;
use crate::{
    records::{RecordClient, RecordError},
    turns::observer::NoopObserver,
    ChannelCacheConfig,
};
use cmaeg_core::{
    channel::ChannelDefinition,
    ids::ChannelId,
    problem::{Problem, ProblemCode},
    seal::{open_blob, ChannelPointer, MasterKeyRing, OpenError},
};
use dashmap::{mapref::entry::Entry, DashMap};
use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use std::{
    num::NonZeroU32,
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::sync::Notify;
use tracing::{instrument::WithSubscriber, Instrument};

pub use crate::telemetry::names::RESTORE_DURATION as RESTORE_DURATION_METRIC;

#[derive(Debug)]
pub struct ResolvedChannel {
    pub id: ChannelId,
    pub pointer: ChannelPointer,
    pub definition: ChannelDefinition,
    pub agent_key_hash: [u8; 32],
    pub loaded_at: Instant,
    pub version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    Invalid,
    Tampered,
    NotFound,
    InvalidDefinition,
    RateLimited,
    Unavailable,
}
impl ResolveError {
    pub fn problem(&self) -> Problem {
        let (code, status, detail) = match self {
            Self::Invalid => (ProblemCode::InvalidRequest, 400, "malformed channel ID"),
            Self::Tampered | Self::NotFound => {
                (ProblemCode::ChannelNotFound, 404, "channel not found")
            }
            Self::InvalidDefinition => (
                ProblemCode::ChannelInvalid,
                503,
                "stored channel is invalid",
            ),
            Self::RateLimited => (
                ProblemCode::RateLimited,
                429,
                "channel restore rate exceeded",
            ),
            Self::Unavailable => (
                ProblemCode::ChannelRestoreUnavailable,
                503,
                "channel restore is unavailable",
            ),
        };
        Problem::new(code, status, detail)
    }
}
impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.problem().fmt(f)
    }
}
impl std::error::Error for ResolveError {}

struct Flight {
    notify: Notify,
    result: Mutex<Option<Result<Arc<ResolvedChannel>, ResolveError>>>,
    invalidated: std::sync::atomic::AtomicBool,
}
impl Flight {
    fn new() -> Self {
        Self {
            notify: Notify::new(),
            result: Mutex::new(None),
            invalidated: false.into(),
        }
    }
    async fn wait(&self) -> Result<Arc<ResolvedChannel>, ResolveError> {
        loop {
            // Register before inspecting the result, so completion cannot lose a wakeup.
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(result) = self.result.lock().unwrap().clone() {
                return result;
            }
            notified.await;
        }
    }
}

#[derive(Clone)]
pub struct ChannelResolver {
    inner: Arc<Inner>,
}
struct Inner {
    records: Arc<RecordClient>,
    observer: Arc<dyn ChannelIndexObserver>,
    keys: Arc<MasterKeyRing>,
    config: ChannelCacheConfig,
    cache: ChannelCache,
    flights: DashMap<String, Arc<Flight>>,
    limiter: DefaultKeyedRateLimiter<String>,
    // Invalidation and restore publication share a short synchronous critical section.
    publication: Mutex<()>,
    restores: std::sync::atomic::AtomicUsize,
}
impl ChannelResolver {
    pub fn new(
        records: Arc<RecordClient>,
        keys: Arc<MasterKeyRing>,
        config: ChannelCacheConfig,
    ) -> Self {
        Self::observed(records, keys, config, Arc::new(NoopObserver))
    }
    /// A resolver that reports each channel id it restores to `observer`.
    pub fn observed(
        records: Arc<RecordClient>,
        keys: Arc<MasterKeyRing>,
        config: ChannelCacheConfig,
        observer: Arc<dyn ChannelIndexObserver>,
    ) -> Self {
        let quota =
            Quota::per_minute(config.restores_per_minute).allow_burst(NonZeroU32::new(3).unwrap());
        Self {
            inner: Arc::new(Inner {
                records,
                observer,
                keys,
                cache: ChannelCache::new(&config),
                config,
                flights: DashMap::new(),
                limiter: RateLimiter::keyed(quota),
                publication: Mutex::new(()),
                restores: 0.into(),
            }),
        }
    }
    pub fn cache_len(&self) -> usize {
        self.inner.cache.len()
    }
    pub(crate) fn record_client(&self) -> RecordClient {
        (*self.inner.records).clone()
    }
    /// Recheck live channel authority without accepting a cached definition.
    pub async fn resolve_current(
        &self,
        raw_id: &str,
    ) -> Result<Arc<ResolvedChannel>, ResolveError> {
        let id = ChannelId::new(raw_id).map_err(|_| ResolveError::Invalid)?;
        let pointer = id
            .open(&self.inner.keys)
            .map_err(|_| ResolveError::Tampered)?
            .payload;
        if self
            .inner
            .records
            .record_url(pointer.record_id.as_str())
            .map_err(|_| ResolveError::Invalid)?
            .as_str()
            != pointer.record_url
        {
            return Err(ResolveError::Tampered);
        }
        tokio::time::timeout(self.inner.config.restore_timeout, self.fetch(id, pointer))
            .await
            .unwrap_or(Err(ResolveError::Unavailable))
    }
    pub fn invalidate(&self, id: &str) {
        let _guard = self.inner.publication.lock().unwrap();
        self.inner.cache.invalidate(id);
        if let Some(flight) = self.inner.flights.get(id) {
            flight
                .invalidated
                .store(true, std::sync::atomic::Ordering::Release);
        }
    }
    pub async fn resolve(&self, raw_id: &str) -> Result<Arc<ResolvedChannel>, ResolveError> {
        self.resolve_observed(raw_id, None).await
    }
    pub async fn resolve_observed(
        &self,
        raw_id: &str,
        timing: Option<&crate::telemetry::timing::IngressTiming>,
    ) -> Result<Arc<ResolvedChannel>, ResolveError> {
        let _duration = timing.map(|timing| timing.restore());
        loop {
            let stale = match self.inner.cache.lookup(raw_id) {
                Lookup::Fresh(channel) => return Ok(channel),
                Lookup::Negative(problem) => return Err(negative_error(&problem)),
                Lookup::Stale(channel) => Some(channel),
                Lookup::Miss => None,
            };
            let id = ChannelId::new(raw_id).map_err(|_| ResolveError::Invalid)?;
            let opened = id.open(&self.inner.keys).map_err(|error| match error {
                OpenError::Format | OpenError::WrongKind => ResolveError::Invalid,
                _ => ResolveError::Tampered,
            })?;
            let pointer = opened.payload;
            if self
                .inner
                .records
                .record_url(pointer.record_id.as_str())
                .map_err(|_| ResolveError::Invalid)?
                .as_str()
                != pointer.record_url
            {
                return Err(ResolveError::Tampered);
            }
            let flight = {
                let _guard = self.inner.publication.lock().unwrap();
                // Another restore may have published after the initial cache lookup.
                match self.inner.cache.lookup(raw_id) {
                    Lookup::Fresh(channel) => return Ok(channel),
                    Lookup::Negative(problem) => return Err(negative_error(&problem)),
                    _ => {}
                }
                match self.inner.flights.entry(raw_id.to_owned()) {
                    Entry::Occupied(entry) => entry.get().clone(),
                    Entry::Vacant(entry) => {
                        if let Some(timing) = timing {
                            timing.triggered_restore();
                        }
                        let flight = Arc::new(Flight::new());
                        entry.insert(flight.clone());
                        let resolver = self.clone();
                        let pending = flight.clone();
                        let span = tracing::info_span!("cmaeg.restore", channel = %crate::telemetry::channel_hash(raw_id), event_id = tracing::field::Empty, kid = opened.kid);
                        // A caller cancellation must not strand the shared flight or its waiters.
                        tokio::spawn(
                            async move {
                                resolver.restore(id, pointer, pending).await;
                            }
                            .instrument(span)
                            .with_current_subscriber(),
                        );
                        flight
                    }
                }
            };
            if let Some(channel) = stale {
                return Ok(channel);
            }
            let result = flight.wait().await;
            if !flight
                .invalidated
                .load(std::sync::atomic::Ordering::Acquire)
            {
                return result;
            }
        }
    }
    async fn restore(&self, id: ChannelId, pointer: ChannelPointer, flight: Arc<Flight>) {
        let started = Instant::now();
        if self
            .inner
            .restores
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .is_multiple_of(256)
        {
            self.inner.limiter.retain_recent();
        }
        let result = if self.inner.limiter.check_key(&id.to_string()).is_err() {
            Err(ResolveError::RateLimited)
        } else {
            tokio::time::timeout(
                self.inner.config.restore_timeout,
                self.fetch(id.clone(), pointer),
            )
            .await
            .unwrap_or(Err(ResolveError::Unavailable))
        };
        tracing::info!(
            metric = RESTORE_DURATION_METRIC,
            duration_seconds = started.elapsed().as_secs_f64(),
            success = result.is_ok(),
            "channel restore finished"
        );
        metrics::histogram!(crate::telemetry::names::RESTORE_DURATION,
            crate::telemetry::names::OUTCOME => if result.is_ok() { "success" } else { "error" })
        .record(started.elapsed().as_secs_f64());
        let _guard = self.inner.publication.lock().unwrap();
        if !flight
            .invalidated
            .load(std::sync::atomic::Ordering::Acquire)
        {
            let entry = match &result {
                Ok(channel) => Some(CacheEntry::Present(
                    channel.clone(),
                    Instant::now() + self.inner.config.ttl,
                )),
                Err(ResolveError::NotFound) => Some(CacheEntry::Negative(
                    ResolveError::NotFound.problem(),
                    Instant::now() + self.inner.config.negative_ttl,
                )),
                _ => None,
            };
            if let Some(entry) = entry {
                self.inner.cache.insert(id.to_string(), entry);
            }
        }
        *flight.result.lock().unwrap() = Some(result);
        self.inner.flights.remove(id.as_str());
        flight.notify.notify_waiters();
    }
    async fn fetch(
        &self,
        id: ChannelId,
        pointer: ChannelPointer,
    ) -> Result<Arc<ResolvedChannel>, ResolveError> {
        let record =
            self.inner
                .records
                .get(&pointer.record_url)
                .await
                .map_err(|error| match error {
                    RecordError::NotFound => ResolveError::NotFound,
                    _ => ResolveError::Unavailable,
                })?;
        let opened = open_blob(&self.inner.keys, &record.blob)
            .map_err(|_| ResolveError::InvalidDefinition)?;
        if opened.needs_rekey {
            tracing::info!(kid = opened.kid, "restored channel blob needs rekey");
        }
        let mut definition: ChannelDefinition =
            serde_json::from_slice(&opened.payload).map_err(|_| ResolveError::InvalidDefinition)?;
        definition
            .validate()
            .map_err(|_| ResolveError::InvalidDefinition)?;
        if definition.record_id != pointer.record_id
            || definition.profile_digest != definition.profile.digest()
        {
            return Err(ResolveError::InvalidDefinition);
        }
        if definition
            .creation
            .as_ref()
            .is_some_and(|value| !value.ready)
        {
            return Err(ResolveError::Unavailable);
        }
        let mut agent_key_hash = [0; 32];
        for (index, byte) in agent_key_hash.iter_mut().enumerate() {
            *byte = u8::from_str_radix(
                &definition.secrets.agent_key_hash[7 + index * 2..9 + index * 2],
                16,
            )
            .map_err(|_| ResolveError::InvalidDefinition)?;
        }
        definition.version = record.version;
        self.inner.observer.observed(&pointer.record_id, &id);
        Ok(Arc::new(ResolvedChannel {
            id,
            pointer,
            definition,
            agent_key_hash,
            loaded_at: Instant::now(),
            version: record.version,
        }))
    }
}

fn negative_error(problem: &Problem) -> ResolveError {
    if problem.code == ProblemCode::ChannelNotFound {
        ResolveError::NotFound
    } else {
        ResolveError::InvalidDefinition
    }
}
