//! Atomic turn admission, cancellation-safe completion and expiring tool-reply dedupe.
use crate::{http::readiness::Readiness, ingress::launch::LaunchRequest};
use dashmap::DashMap;
use futures_util::future::{AbortHandle, Abortable};
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Instant,
};
use tokio::sync::Notify;
use tracing::{instrument::WithSubscriber, Instrument};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TurnKey {
    pub channel_id: String,
    pub event_id: String,
    // Two relay duplicates may coexist while the provider decides idempotency.
    pub launch_id: u64,
}
struct TurnHandle {
    abort: AbortHandle,
    started_at: Instant,
}
pub struct TurnTracker {
    active: DashMap<TurnKey, TurnHandle>,
    gate: Mutex<()>,
    draining: AtomicBool,
    next: AtomicU64,
    changed: Notify,
    readiness: Readiness,
    replied: Mutex<std::collections::HashMap<[u8; 32], i64>>,
}
impl TurnTracker {
    pub fn new(readiness: Readiness) -> Arc<Self> {
        Arc::new(Self {
            active: DashMap::new(),
            gate: Mutex::new(()),
            draining: false.into(),
            next: 0.into(),
            changed: Notify::new(),
            readiness,
            replied: Mutex::new(Default::default()),
        })
    }
    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::Acquire)
    }
    pub fn len(&self) -> usize {
        self.active.len()
    }
    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
    }
    pub fn oldest_started_at(&self) -> Option<Instant> {
        self.active.iter().map(|entry| entry.started_at).min()
    }
    pub fn spawn(
        self: &Arc<Self>,
        request: &LaunchRequest,
        future: impl Future<Output = ()> + Send + 'static,
    ) -> Result<(), &'static str> {
        let _gate = self.gate.lock().unwrap();
        if self.is_draining() {
            return Err("draining");
        }
        if self.active.len() >= 4096 {
            return Err("turn_capacity");
        }
        let key = TurnKey {
            channel_id: request.channel.id.to_string(),
            event_id: request.activity.event_id.to_string(),
            launch_id: self.next.fetch_add(1, Ordering::Relaxed),
        };
        let (abort, registration) = AbortHandle::new_pair();
        self.active.insert(
            key.clone(),
            TurnHandle {
                abort,
                started_at: request.received_at,
            },
        );
        metrics::gauge!(crate::telemetry::names::TURNS_INFLIGHT).increment(1);
        let guard = TurnGuard {
            tracker: self.clone(),
            key,
        };
        // Real routed children still hold their lease before execute starts;
        // the unleased base request tracks a finalizer, not a default turn.
        let routed_child =
            request.activity.routing.is_some() && request.lease.lock().unwrap().is_some();
        let consumer = routed_child.then(|| {
            request
                .consumer
                .as_ref()
                .map_or("default", cmaeg_core::ids::ConsumerId::as_str)
        });
        tokio::spawn(
            async move {
                let _guard = guard;
                let _ = Abortable::new(future, registration).await;
            }
            .instrument(tracing::info_span!("cmaeg.turn",
                channel = %crate::telemetry::channel_hash(request.channel.id.as_str()),
                event_id = %request.activity.event_id,
                consumer))
            .with_current_subscriber(),
        );
        Ok(())
    }
    pub fn start_draining(&self) {
        let _gate = self.gate.lock().unwrap();
        self.draining.store(true, Ordering::Release);
        self.readiness.start_draining();
    }
    /// Returns false if the deadline forced cancellation of remaining local consumers.
    pub async fn drain(&self, deadline: Instant) -> bool {
        self.start_draining();
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_empty() {
                return true;
            }
            if tokio::time::timeout_at(deadline.into(), notified)
                .await
                .is_err()
            {
                for turn in &self.active {
                    turn.abort.abort();
                }
                return false;
            }
        }
    }
    pub fn record_tool_reply(
        &self,
        event_ref: &str,
        expires_at: i64,
        now: i64,
    ) -> Result<(), &'static str> {
        if expires_at <= now || expires_at > now.saturating_add(1800) {
            return Err("event_ref_expired");
        }
        let digest = Sha256::digest(event_ref.as_bytes()).into();
        let mut replies = self.replied.lock().unwrap();
        replies.retain(|_, expires| *expires > now);
        if replies.len() >= 50_000 && !replies.contains_key(&digest) {
            return Err("dedupe_capacity");
        }
        replies.insert(digest, expires_at);
        Ok(())
    }
    pub fn has_tool_reply(&self, event_ref: &str, now: i64) -> bool {
        let digest: [u8; 32] = Sha256::digest(event_ref.as_bytes()).into();
        let mut replies = self.replied.lock().unwrap();
        if replies.get(&digest).is_some_and(|expires| *expires > now) {
            true
        } else {
            replies.remove(&digest);
            false
        }
    }
}
struct TurnGuard {
    tracker: Arc<TurnTracker>,
    key: TurnKey,
}
impl Drop for TurnGuard {
    fn drop(&mut self) {
        self.tracker.active.remove(&self.key);
        metrics::gauge!(crate::telemetry::names::TURNS_INFLIGHT).decrement(1);
        self.tracker.changed.notify_waiters();
    }
}
