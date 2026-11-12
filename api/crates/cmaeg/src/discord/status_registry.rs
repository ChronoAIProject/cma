//! At most 64 selected-source observations; no I/O or secrets in this registry.
use super::{NativeDiscordConnectionStatus, Outcome, Stage, StatusAttempt};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use tokio::time::Instant;

pub(super) const SUPERVISOR_TTL: Duration = Duration::from_secs(30);
#[derive(Clone, Default)]
pub(crate) struct StatusRegistry(Arc<Mutex<Registry>>);
#[derive(Default)]
struct Registry {
    entries: BTreeMap<String, Entry>,
    generation: u64,
    log_window: Option<Instant>,
    log_count: u32,
}
pub(super) struct Entry {
    pub generation: u64,
    pub revision: Option<String>,
    pub value: NativeDiscordConnectionStatus,
    pub supervisor_until: Instant,
    pub lease_until: Option<Instant>,
    pub heartbeat_until: Option<Instant>,
    pub failed: bool,
    pub finished: bool,
    pub last_log: Option<Instant>,
}
impl Entry {
    fn new(generation: u64, revision: Option<&str>, now: Instant) -> Self {
        Self {
            generation,
            revision: revision.map(str::to_owned),
            value: NativeDiscordConnectionStatus {
                stage: Stage::Resolve,
                outcome: Outcome::Pending,
                observed_at: timestamp(),
                ..Default::default()
            },
            supervisor_until: now + SUPERVISOR_TTL,
            lease_until: None,
            heartbeat_until: None,
            failed: false,
            finished: false,
            last_log: None,
        }
    }
    pub fn touch(&mut self) {
        self.supervisor_until = Instant::now() + SUPERVISOR_TTL;
        self.value.observed_at = timestamp();
    }
    pub fn disconnect(&mut self) {
        self.value.connected = Some(false);
        self.value.authenticated = Some(false);
        self.value.ready = Some(false);
        self.heartbeat_until = None;
    }
    fn project(&self, revision: Option<&str>) -> NativeDiscordConnectionStatus {
        if self
            .revision
            .as_deref()
            .is_some_and(|saved| Some(saved) != revision)
        {
            return Default::default();
        }
        let now = Instant::now();
        let mut value = self.value.clone();
        let deadline = if value.connected == Some(true) {
            self.lease_until
                .zip(self.heartbeat_until)
                .map(|(lease, heartbeat)| lease.min(heartbeat).min(self.supervisor_until))
        } else {
            Some(self.supervisor_until)
        };
        value.stale = deadline.is_none_or(|deadline| deadline <= now);
        value.valid_until =
            deadline.and_then(|deadline| timestamp_after(deadline.saturating_duration_since(now)));
        if value.stale {
            value.connected = None;
            value.authenticated = None;
            value.ready = None;
            value.outcome = Outcome::Unknown;
        }
        value
    }
}
impl StatusRegistry {
    pub(crate) fn begin(&self, record: &str, revision: Option<&str>) -> StatusAttempt {
        let Ok(mut registry) = self.0.lock() else {
            return Default::default();
        };
        if !registry.entries.contains_key(record) && registry.entries.len() >= 64 {
            return Default::default();
        }
        let Some(generation) = registry.generation.checked_add(1) else {
            return Default::default();
        };
        registry.generation = generation;
        let mut entry = Entry::new(generation, revision, Instant::now());
        if let Some(old) = registry
            .entries
            .get(record)
            .filter(|old| old.revision.as_deref() == revision)
        {
            entry.value.last_ready_at = old.value.last_ready_at.clone();
            entry.value.last_event_at = old.value.last_event_at.clone();
            entry.value.last_event_committed_at = old.value.last_event_committed_at.clone();
            entry.value.last_dispatch_at = old.value.last_dispatch_at.clone();
            entry.value.last_dispatch_outcome = old.value.last_dispatch_outcome.clone();
            entry.value.last_failure_stage = old.value.last_failure_stage;
            entry.value.last_failure_code = old.value.last_failure_code;
            entry.value.last_failure_at = old.value.last_failure_at.clone();
            entry.last_log = old.last_log;
        }
        registry.entries.insert(record.to_owned(), entry);
        StatusAttempt {
            registry: self.clone(),
            record: record.into(),
            generation,
        }
    }
    pub(crate) fn remove(&self, record: &str) {
        if let Ok(mut registry) = self.0.lock() {
            registry.entries.remove(record);
        }
    }
    pub(crate) fn view(
        &self,
        record: &str,
        revision: Option<&str>,
    ) -> NativeDiscordConnectionStatus {
        self.0
            .lock()
            .ok()
            .and_then(|r| r.entries.get(record).map(|e| e.project(revision)))
            .unwrap_or_default()
    }
    pub(super) fn update(&self, record: &str, generation: u64, update: impl FnOnce(&mut Entry)) {
        let Ok(mut registry) = self.0.lock() else {
            return;
        };
        let now = Instant::now();
        let Some(entry) = registry
            .entries
            .get_mut(record)
            .filter(|e| e.generation == generation)
        else {
            return;
        };
        let before = (
            entry.value.stage,
            entry.value.outcome,
            entry.value.last_failure_code,
        );
        let was_connected = entry.value.connected;
        update(entry);
        let after = (
            entry.value.stage,
            entry.value.outcome,
            entry.value.last_failure_code,
        );
        let changed_failure = before.2 != after.2;
        let became_ready = was_connected != Some(true) && entry.value.connected == Some(true);
        let notable = changed_failure
            || matches!(
                after.1,
                Outcome::Blocked | Outcome::Retrying | Outcome::Stopped
            )
            || became_ready;
        let log = before != after
            && notable
            && (changed_failure
                || became_ready
                || entry
                    .last_log
                    .is_none_or(|last| now.duration_since(last) >= Duration::from_secs(5)));
        if !log {
            return;
        }
        if registry
            .log_window
            .is_none_or(|start| now.duration_since(start) >= Duration::from_secs(60))
        {
            registry.log_window = Some(now);
            registry.log_count = 0;
        }
        if registry.log_count >= 64 {
            return;
        }
        registry.log_count += 1;
        if let Some(entry) = registry.entries.get_mut(record) {
            entry.last_log = Some(now);
        }
        drop(registry);
        // No parent span: even a caller's request identity must not escape here.
        tracing::info!(parent: None, stage = after.0.as_str(), outcome = after.1.as_str(),
            reason = if after.1 == Outcome::Ok { "none" } else { after.2.map_or("none", |code| code.as_str()) },
            "native Discord connection state changed");
    }
    pub(crate) fn diagnostic(&self, cleanup: bool) {
        if !self.allow_diagnostic() {
            return;
        }
        if cleanup {
            tracing::warn!(parent: None, reason = "cleanup_failed", "native Discord connection cleanup failed");
        } else {
            tracing::warn!(parent: None, reason = "scan_unavailable", "native Discord inbox scan failed");
        }
    }
    pub(super) fn allow_diagnostic(&self) -> bool {
        let Ok(mut registry) = self.0.lock() else {
            return false;
        };
        let now = Instant::now();
        if registry
            .log_window
            .is_none_or(|start| now.duration_since(start) >= Duration::from_secs(60))
        {
            registry.log_window = Some(now);
            registry.log_count = 0;
        }
        if registry.log_count >= 64 {
            return false;
        }
        registry.log_count += 1;
        true
    }
    pub(crate) fn dispatch(&self, record: &str, revision: &str, outcome: &'static str) {
        if !matches!(
            outcome,
            "admitted" | "pending" | "refused" | "expired" | "invalid"
        ) {
            return;
        }
        if let Ok(mut registry) = self.0.lock() {
            if let Some(entry) = registry
                .entries
                .get_mut(record)
                .filter(|e| e.revision.as_deref() == Some(revision))
            {
                entry.value.last_dispatch_at = timestamp();
                entry.value.last_dispatch_outcome = outcome.into();
            }
        }
    }
}
pub(super) fn timestamp() -> Option<String> {
    OffsetDateTime::now_utc().format(&Rfc3339).ok()
}
pub(super) fn timestamp_after(delay: Duration) -> Option<String> {
    let bounded = delay.min(Duration::from_secs(7 * 86_400));
    (OffsetDateTime::now_utc() + bounded).format(&Rfc3339).ok()
}
