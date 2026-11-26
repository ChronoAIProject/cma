//! Process-local pacing for repeated stale acquisition of the same pending work.
use super::*;
use layer2_core::agent_ownership::{
    Authority, OwnershipCoordinator, OwnershipError, STORE_DEADLINE_MS,
};
use layer2_core::sandbox::store::PendingAgentWork;
use std::collections::BTreeMap;
use tokio::time::Instant;

const FIRST_DELAY: Duration = Duration::from_secs(2);
const BACKGROUND_CAP: Duration = Duration::from_secs(30);
const CONTROL_CAP: Duration = Duration::from_secs(4);
const FORGET_AFTER: Duration = Duration::from_secs(120);
const MAX_ENTRIES: usize = 1024;

struct Entry {
    work: PendingAgentWork,
    delay: Duration,
    retry_at: Instant,
    forget_at: Instant,
}

#[derive(Default)]
pub(super) struct Backoff(Mutex<HashMap<AgentId, Entry>>);

impl Backoff {
    fn entries(&self) -> std::sync::MutexGuard<'_, HashMap<AgentId, Entry>> {
        self.0.lock().unwrap_or_else(|error| {
            tracing::error!("Agent ownership acquisition backoff was poisoned");
            error.into_inner()
        })
    }

    fn tracking(&self, id: &AgentId, now: Instant) -> bool {
        let mut entries = self.entries();
        if entries.get(id).is_some_and(|entry| now >= entry.forget_at) {
            entries.remove(id);
        }
        entries.contains_key(id)
    }

    fn waiting(&self, id: &AgentId, work: &PendingAgentWork, now: Instant) -> bool {
        self.entries()
            .get(id)
            .is_some_and(|entry| &entry.work == work && now < entry.retry_at)
    }

    fn stale(&self, id: &AgentId, work: PendingAgentWork, sampled_before: bool, now: Instant) {
        let mut entries = self.entries();
        let previous = entries.remove(id);
        if entries.len() >= MAX_ENTRIES {
            // Evict the oldest observation when every slot is occupied.
            if let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, entry)| entry.forget_at)
                .map(|(agent, _)| agent.clone())
            {
                entries.remove(&oldest);
            }
        }
        let cap = if work.control_revision.is_some() {
            CONTROL_CAP
        } else {
            BACKGROUND_CAP
        };
        let delay = if sampled_before {
            previous
                .filter(|entry| entry.work == work && now < entry.forget_at)
                .map_or(FIRST_DELAY, |entry| (entry.delay * 2).max(FIRST_DELAY))
                .min(cap)
        } else {
            // The first read follows the refusal. Seed the work identity but
            // leave it eligible until a refusal observes it before acquisition.
            Duration::ZERO
        };
        if !delay.is_zero() {
            tracing::debug!(
                step = work.step.operation_name(),
                delay_ms = delay.as_millis() as u64,
                "Delayed repeated stale Agent ownership acquisition"
            );
        }
        entries.insert(
            id.clone(),
            Entry {
                work,
                delay,
                retry_at: now + delay,
                forget_at: now + FORGET_AFTER,
            },
        );
    }

    pub(super) fn cooling(&self, now: Instant) -> BTreeMap<AgentId, PendingAgentWork> {
        self.entries()
            .iter()
            .filter(|(_, entry)| now < entry.retry_at)
            .map(|(id, entry)| (id.clone(), entry.work.clone()))
            .collect()
    }

    fn clear(&self, id: &AgentId) {
        self.entries().remove(id);
    }
}

impl AgentWorker {
    pub(super) async fn acquire_pending_work(
        &self,
        coordinator: &OwnershipCoordinator,
        id: &AgentId,
    ) -> Result<Option<Arc<Authority>>, OwnershipError> {
        let before = if self.acquisition_backoff.tracking(id, Instant::now()) {
            self.pending_work_for_acquisition(id).await
        } else {
            None
        };
        if before
            .as_ref()
            .is_some_and(|work| self.acquisition_backoff.waiting(id, work, Instant::now()))
        {
            return Ok(None);
        }
        let result = coordinator.acquire(id).await;
        match &result {
            Ok(Some(_)) => self.acquisition_backoff.clear(id),
            Err(OwnershipError::Stale) => {
                let sampled_before = before.is_some();
                let work = match before {
                    Some(work) => Some(work),
                    None => self.pending_work_for_acquisition(id).await,
                };
                if let Some(work) = work {
                    self.acquisition_backoff
                        .stale(id, work, sampled_before, Instant::now());
                }
            }
            _ => {}
        }
        result
    }

    async fn pending_work_for_acquisition(&self, id: &AgentId) -> Option<PendingAgentWork> {
        match tokio::time::timeout(
            Duration::from_millis(STORE_DEADLINE_MS),
            self.dependencies.store.get_agent(id),
        )
        .await
        {
            Ok(Ok(agent)) => agent.as_ref().and_then(PendingAgentWork::from_agent),
            Ok(Err(_)) | Err(_) => {
                tracing::debug!("Could not verify pending Agent work for acquisition backoff");
                None
            }
        }
    }
}
