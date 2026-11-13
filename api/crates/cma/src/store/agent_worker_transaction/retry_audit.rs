//! Process-local audit age is only a reason to do more verification.
pub(super) mod telemetry;

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use layer2_core::{sandbox::Agent, AgentId};

// Approved integrity contract: audit on the next retry at one hour, never an
// operator-tunable interval. Restart and eviction require the first audit again.
const AUDIT_INTERVAL: Duration = Duration::from_secs(60 * 60);
const MAX_AUDITS: usize = 10_000;

#[derive(Default)]
pub(in crate::store) struct RetryAudits(Mutex<HashMap<AgentId, Audit>>);

struct Audit {
    creation_key: String,
    fingerprint: String,
    at: Instant,
}

impl RetryAudits {
    pub(super) fn recent(&self, agent: &Agent) -> bool {
        self.0
            .lock()
            .expect("retry audit lock poisoned")
            .get(&agent.id)
            .is_some_and(|audit| {
                audit.creation_key == agent.creation_key
                    && audit.fingerprint == agent.creation_fingerprint
                    && audit.at.elapsed() < AUDIT_INTERVAL
            })
    }

    pub(super) fn record(&self, agent: &Agent, at: Instant) {
        let mut audits = self.0.lock().expect("retry audit lock poisoned");
        if audits.len() >= MAX_AUDITS {
            // Eviction can only cause an additional complete audit.
            audits.retain(|_, audit| audit.at.elapsed() < AUDIT_INTERVAL);
            if audits.len() >= MAX_AUDITS {
                if let Some(oldest) = audits
                    .iter()
                    .min_by_key(|(_, a)| a.at)
                    .map(|(id, _)| id.clone())
                {
                    audits.remove(&oldest);
                }
            }
        }
        audits.insert(
            agent.id.clone(),
            Audit {
                creation_key: agent.creation_key.clone(),
                fingerprint: agent.creation_fingerprint.clone(),
                at,
            },
        );
    }

    #[cfg(test)]
    pub(in crate::store) fn expire(&self, agent: &Agent) {
        self.record(agent, Instant::now() - AUDIT_INTERVAL);
    }
}
