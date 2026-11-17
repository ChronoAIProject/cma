//! Closed, content-free scan diagnostics. No record mutation or execution authority.
use super::Error;
use crate::ingress::routing_store::AdmissionError;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use tokio::time::{Duration, Instant};

macro_rules! labels {
    ($name:ident { $($variant:ident => $label:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
        pub enum $name { $(#[serde(rename = $label)] $variant),+ }
        impl $name {
            pub fn as_str(self) -> &'static str { match self { $(Self::$variant => $label),+ } }
        }
    };
}
labels!(Class {
    Corrupt => "corrupt", Expired => "expired", MissingItem => "missing_item",
    MissingPlan => "missing_plan", Mismatch => "mismatch", Unavailable => "unavailable",
    Busy => "busy", Capacity => "capacity",
});
labels!(Stage {
    HeadRead => "head_read", HeadDecode => "head_decode", HeadValidate => "head_validate",
    Front => "front", ItemRead => "item_read", ItemCheck => "item_check",
    PlanLoad => "plan_load", PlanCheck => "plan_check", Placement => "placement",
    AdmissionScan => "admission_scan", Evidence => "evidence", Effect => "effect",
    QueueRead => "queue_read", Clock => "clock", Observation => "observation",
});
impl From<Error> for Class {
    fn from(error: Error) -> Self {
        match error {
            Error::Corrupt => Self::Corrupt,
            Error::Expired => Self::Expired,
            Error::Busy | Error::Lost => Self::Busy,
            Error::Unavailable => Self::Unavailable,
            Error::Capacity => Self::Capacity,
        }
    }
}
impl From<AdmissionError> for Class {
    fn from(error: AdmissionError) -> Self {
        match error {
            AdmissionError::Expired => Self::Expired,
            AdmissionError::Unavailable
            | AdmissionError::Uncertain
            | AdmissionError::ExpiredOrUnavailable => Self::Unavailable,
            AdmissionError::Conflict | AdmissionError::GuardConflict => Self::Busy,
            AdmissionError::TooLarge => Self::Capacity,
            AdmissionError::Invalid | AdmissionError::Corrupt | AdmissionError::MissingChunk => {
                Self::Corrupt
            }
        }
    }
}

pub(crate) fn partition_hash(partition: &str) -> String {
    format!("{:x}", Sha256::digest(partition.as_bytes()))[..12].into()
}

#[derive(Clone, Debug)]
pub struct ScanError {
    pub class: Class,
    pub stage: Stage,
    pub partition_hash: String,
    pub partition_known: bool,
    pub age_seconds: Option<i64>,
}
impl ScanError {
    pub fn new(class: impl Into<Class>, stage: Stage, partition: Option<&str>) -> Self {
        Self {
            class: class.into(),
            stage,
            partition_hash: partition.map(partition_hash).unwrap_or_else(|| "-".into()),
            partition_known: partition.is_some(),
            age_seconds: None,
        }
    }
    pub fn aged(mut self, seconds: i64) -> Self {
        self.age_seconds = Some(seconds);
        self
    }
    pub fn admission(&self) -> AdmissionError {
        match self.class {
            Class::Expired | Class::MissingItem => AdmissionError::Expired,
            Class::Unavailable | Class::Busy => AdmissionError::Unavailable,
            Class::Capacity => AdmissionError::TooLarge,
            _ => AdmissionError::Corrupt,
        }
    }
    pub fn report(&self, logs: &mut LogLimit) {
        self.report_scope(logs, false);
    }
    pub fn report_scan(&self, logs: &mut LogLimit) {
        self.report_scope(logs, true);
    }
    fn report_scope(&self, logs: &mut LogLimit, scan_wide: bool) {
        metrics::counter!("cmaeg_replica_scan_failures_total", "class" => self.class.as_str(), "stage" => self.stage.as_str()).increment(1);
        if logs.allow(self) {
            let message = if scan_wide {
                "routed admission scan unavailable; retaining cursor"
            } else {
                "routed scan isolated failure; evidence retained"
            };
            tracing::warn!(class = self.class.as_str(), stage = self.stage.as_str(), partition_hash = %self.partition_hash, partition_known = self.partition_known, "{message}");
        }
    }
}

#[derive(Default)]
pub struct LogLimit(BTreeMap<String, Instant>);
impl LogLimit {
    fn allow(&mut self, error: &ScanError) -> bool {
        // A scan-wide error has no partition. Distinguish its closed stages;
        // never mislabel the last cursor as the partition that failed.
        let key = if error.partition_known {
            error.partition_hash.clone()
        } else {
            error.stage.as_str().into()
        };
        let now = Instant::now();
        if self
            .0
            .get(&key)
            .is_some_and(|at| now.duration_since(*at) < Duration::from_secs(180))
        {
            return false;
        }
        self.0.insert(key, now);
        true
    }
    pub fn prune(&mut self) {
        self.0
            .retain(|_, at| at.elapsed() < Duration::from_secs(180));
    }
}
struct Isolated {
    stage: Stage,
    retry_at: Instant,
    attempts: u32,
}
#[derive(Default)]
pub(super) struct Isolation {
    entries: BTreeMap<String, Isolated>,
    logs: LogLimit,
}
impl Isolation {
    pub fn failed(&mut self, id: &str, error: &ScanError) {
        error.report(&mut self.logs);
        let attempts = self
            .entries
            .get(id)
            .map_or(1, |entry| (entry.attempts + 1).min(7));
        let seconds = if matches!(error.class, Class::Unavailable | Class::Busy) {
            (1u64 << (attempts - 1)).min(60)
        } else {
            60
        };
        self.entries.insert(
            id.into(),
            Isolated {
                stage: error.stage,
                retry_at: Instant::now() + Duration::from_secs(seconds),
                attempts,
            },
        );
        self.gauge();
    }
    pub fn report(&mut self, error: &ScanError) {
        if error.partition_known {
            error.report(&mut self.logs);
        } else {
            error.report_scan(&mut self.logs);
        }
    }
    #[cfg(test)]
    pub fn retry_now(&mut self, id: &str) {
        if let Some(entry) = self.entries.get_mut(id) {
            entry.retry_at = Instant::now();
        }
    }
    pub fn ready(&self, id: &str) -> bool {
        self.entries
            .get(id)
            .is_none_or(|entry| Instant::now() >= entry.retry_at)
    }
    pub fn healthy(&mut self, id: &str) {
        self.entries.remove(id);
        self.gauge();
    }
    pub fn valid_head(&mut self, id: &str) {
        if self.entries.get(id).is_some_and(|entry| {
            matches!(
                entry.stage,
                Stage::HeadDecode | Stage::HeadValidate | Stage::Front
            )
        }) {
            self.healthy(id);
        }
    }
    pub fn retain(&mut self, active: &BTreeSet<String>) {
        self.entries.retain(|id, _| active.contains(id));
        self.logs.prune();
        self.gauge();
    }
    fn gauge(&self) {
        metrics::gauge!("cmaeg_replica_isolated_heads").set(self.entries.len() as f64);
    }
}

#[cfg(test)]
#[path = "diagnostics_tests.rs"]
mod tests;
