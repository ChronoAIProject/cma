//! Process-local stop-loss state. Providers supply opaque boot/process identities
//! and content-free termination evidence; durable work remains authoritative.
use crate::runtime_startup::{StartupHistory, StartupIncarnation, StartupTermination};
use crate::{
    AdapterError, RuntimeHandle, RuntimeStartupFailure, RuntimeStartupFailureClass,
    RuntimeStartupProbe,
};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};
use tokio::time::Instant;

const MAX_RECORDS: usize = 1024;
const THRESHOLD: u32 = 3;
const FAST: Duration = Duration::from_secs(60);
const FIRST_PROBE: Duration = Duration::from_secs(15 * 60);
const MAX_PROBE: Duration = Duration::from_secs(60 * 60);

#[derive(Default)]
pub(super) struct StartupBreaker {
    entries: Mutex<BTreeMap<RuntimeHandle, StartupRecord>>,
}

struct StartupRecord {
    sandbox_id: String,
    generation: u64,
    boot: String,
    last_delivery: Option<Delivery>,
    recovery_incarnation: Option<StartupIncarnation>,
    failure: RuntimeStartupFailure,
    next_probe: Option<Instant>,
    probe_interval: Duration,
}

impl StartupRecord {
    fn failure_at(&self, handle: &RuntimeHandle, next_probe_at: Instant) -> RuntimeStartupFailure {
        self.failure.clone().with_probe(RuntimeStartupProbe {
            runtime: handle.clone(),
            sandbox_id: self.sandbox_id.clone(),
            generation: self.generation,
            next_probe_at,
        })
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Delivery {
    incarnation: StartupIncarnation,
    delivered_at: SystemTime,
    inspected: bool,
}

impl StartupBreaker {
    pub(super) fn needs_recovery(
        &self,
        handle: &RuntimeHandle,
        incarnation: &StartupIncarnation,
    ) -> bool {
        incarnation.restart >= THRESHOLD && !self.entries.lock().unwrap().contains_key(handle)
    }

    /// Qualifying history can shorten relearning after a freshly acknowledged
    /// failed startup. The ordinary first adoption can still prove serving.
    pub(super) fn recover(
        &self,
        handle: &RuntimeHandle,
        incarnation: &StartupIncarnation,
        sandbox_id: String,
        generation: u64,
        history: StartupHistory,
    ) {
        if incarnation.restart < THRESHOLD || history.duration.is_zero() || history.duration >= FAST
        {
            return;
        }
        let mut entries = self.entries.lock().unwrap();
        if entries.contains_key(handle) {
            return;
        }
        if entries.len() >= MAX_RECORDS {
            entries.pop_first();
        }
        let record = StartupRecord {
            sandbox_id,
            generation,
            boot: incarnation.boot.clone(),
            last_delivery: None,
            recovery_incarnation: Some(incarnation.clone()),
            failure: RuntimeStartupFailure::new(history.class, 0),
            next_probe: None,
            probe_interval: FIRST_PROBE,
        };
        entries.insert(handle.clone(), record);
    }

    pub(super) fn reset(&self, handle: &RuntimeHandle) {
        if let Some(record) = self.entries.lock().unwrap().remove(handle) {
            if record.next_probe.is_some() {
                transition(&record, "closed");
            }
        }
    }

    pub(super) fn reset_boot(&self, handle: &RuntimeHandle, boot: &str) {
        let mut entries = self.entries.lock().unwrap();
        if entries
            .get(handle)
            .is_some_and(|record| record.boot != boot)
        {
            let record = entries.remove(handle).unwrap();
            if record.next_probe.is_some() {
                transition(&record, "closed");
            }
        }
    }

    pub(super) fn failure(&self, handle: &RuntimeHandle) -> Option<RuntimeStartupFailure> {
        self.entries.lock().unwrap().get(handle).and_then(|record| {
            record
                .next_probe
                .map(|next| record.failure_at(handle, next))
        })
    }

    pub(super) fn hold_before_wait(&self, handle: &RuntimeHandle) -> Option<AdapterError> {
        let entries = self.entries.lock().unwrap();
        let record = entries.get(handle)?;
        let next = record.next_probe?;
        (Instant::now() < next).then(|| suppressed(record.failure_at(handle, next)))
    }

    pub(super) fn evidence_needed(
        &self,
        handle: &RuntimeHandle,
        current: &StartupIncarnation,
    ) -> Option<Delivery> {
        let entries = self.entries.lock().unwrap();
        let record = entries.get(handle)?;
        let delivery = record.last_delivery.as_ref()?;
        (record.boot == current.boot
            && !delivery.inspected
            && (current.process != delivery.incarnation.process
                || current.restart > delivery.incarnation.restart))
            .then(|| delivery.clone())
    }

    pub(super) fn observe(
        &self,
        handle: &RuntimeHandle,
        delivered: &Delivery,
        evidence: Option<StartupTermination>,
    ) {
        let mut entries = self.entries.lock().unwrap();
        let Some(record) = entries.get_mut(handle) else {
            return;
        };
        // A concurrent reset or successful serving proof invalidates the read.
        if record.last_delivery.as_ref() != Some(delivered) {
            return;
        }
        let fast = evidence.filter(|evidence| {
            evidence.process == delivered.incarnation.process
                && evidence.failed
                && evidence.finished_at <= delivered.delivered_at + FAST
        });
        let Some(fast) = fast else {
            if record.next_probe.is_some() {
                transition(record, "closed");
            }
            entries.remove(handle);
            return;
        };
        record.last_delivery.as_mut().unwrap().inspected = true;
        let relearned = record.recovery_incarnation.take().as_ref() == Some(&delivered.incarnation);
        record.failure.consecutive = record.failure.consecutive.saturating_add(1);
        record.failure.class = fast.class;
        if (relearned || record.failure.consecutive >= THRESHOLD) && record.next_probe.is_none() {
            record.probe_interval = FIRST_PROBE;
            record.next_probe = Some(Instant::now() + FIRST_PROBE);
            transition(record, "opened");
        }
    }

    pub(super) fn admit(&self, handle: &RuntimeHandle) -> crate::Result<()> {
        let mut entries = self.entries.lock().unwrap();
        let Some(record) = entries.get_mut(handle) else {
            return Ok(());
        };
        let Some(next) = record.next_probe else {
            return Ok(());
        };
        let now = Instant::now();
        if now < next {
            return Err(suppressed(record.failure_at(handle, next)));
        }
        // Claim before delivery, including failed or unacknowledged probes.
        record.probe_interval = (record.probe_interval * 2).min(MAX_PROBE);
        record.next_probe = Some(now + record.probe_interval);
        transition(record, "probe");
        Ok(())
    }

    pub(super) fn delivered(
        &self,
        handle: &RuntimeHandle,
        incarnation: StartupIncarnation,
        sandbox_id: String,
        generation: u64,
        delivered_at: SystemTime,
    ) {
        let mut entries = self.entries.lock().unwrap();
        if !entries.contains_key(handle) && entries.len() >= MAX_RECORDS {
            entries.pop_first();
        }
        let delivery = Delivery {
            incarnation: incarnation.clone(),
            delivered_at,
            inspected: false,
        };
        if let Some(record) = entries.get_mut(handle).filter(|record| {
            record.boot == incarnation.boot
                && record.sandbox_id == sandbox_id
                && record.generation == generation
        }) {
            // Duplicate acknowledgements are still delivered normally. They
            // must not re-arm an already inspected failure of this incarnation
            // or replace evidence of a newer acknowledged incarnation.
            if record
                .recovery_incarnation
                .as_ref()
                .is_some_and(|prior| prior != &incarnation)
            {
                record.recovery_incarnation = None;
            }
            if record.last_delivery.as_ref().is_none_or(|previous| {
                incarnation != previous.incarnation
                    && incarnation.restart >= previous.incarnation.restart
            }) {
                record.last_delivery = Some(delivery);
            }
        } else {
            entries.insert(
                handle.clone(),
                StartupRecord {
                    sandbox_id,
                    generation,
                    boot: incarnation.boot,
                    last_delivery: Some(delivery),
                    recovery_incarnation: None,
                    failure: RuntimeStartupFailure::new(
                        RuntimeStartupFailureClass::ExitedBeforeServing,
                        0,
                    ),
                    next_probe: None,
                    probe_interval: FIRST_PROBE,
                },
            );
        }
    }
}

fn suppressed(failure: RuntimeStartupFailure) -> AdapterError {
    metrics::counter!("cma_runtime_startup_breaker_suppressed_total",
        "class" => failure.class.as_str())
    .increment(1);
    AdapterError::RuntimeStartupHold(failure)
}

fn transition(record: &StartupRecord, transition: &'static str) {
    metrics::counter!("cma_runtime_startup_breaker_transitions_total",
        "transition" => transition, "class" => record.failure.class.as_str())
    .increment(1);
    if transition != "probe" {
        tracing::warn!(parent: None, sandbox_id = %record.sandbox_id,
            class = record.failure.class.as_str(), transition,
            "runtime startup breaker transition");
    }
}

#[cfg(test)]
#[path = "agent_sandbox_startup_breaker_tests.rs"]
mod tests;
