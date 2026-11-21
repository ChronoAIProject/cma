//! Rate-limit optional maintenance discovery, never connection recovery or writes.
use std::{collections::HashMap, sync::Mutex, time::Duration};

use layer2_core::{sandbox::store::AgentConnectionRecoveryCandidate, AgentId};
use tokio::time::Instant;

const RETRY_INTERVAL: Duration = Duration::from_secs(30);
const MAX_TRACKED: usize = 1024;

struct Observation {
    candidate: AgentConnectionRecoveryCandidate,
    desired_revision: u64,
    retry_at: Instant,
}

#[derive(Default)]
pub(in crate::sandbox::agent_worker) struct MaintenanceChecks(Mutex<HashMap<AgentId, Observation>>);

impl MaintenanceChecks {
    pub(super) fn should_check(
        &self,
        candidate: &AgentConnectionRecoveryCandidate,
        desired_revision: u64,
    ) -> bool {
        let now = Instant::now();
        let mut entries = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if entries.get(&candidate.agent_id).is_some_and(|entry| {
            entry.candidate == *candidate
                && entry.desired_revision == desired_revision
                && now < entry.retry_at
        }) {
            return false;
        }
        if entries.len() >= MAX_TRACKED {
            entries.retain(|_, entry| now < entry.retry_at);
        }
        // Saturation fails open: tracking is only an optimization. No result
        // here grants authority, and mutable parent/runtime state is rechecked
        // at least every 30 seconds even when the Agent itself has not changed.
        if entries.len() < MAX_TRACKED || entries.contains_key(&candidate.agent_id) {
            entries.insert(
                candidate.agent_id.clone(),
                Observation {
                    candidate: candidate.clone(),
                    desired_revision,
                    retry_at: now + RETRY_INTERVAL,
                },
            );
        }
        true
    }
}

#[cfg(test)]
mod tests;
