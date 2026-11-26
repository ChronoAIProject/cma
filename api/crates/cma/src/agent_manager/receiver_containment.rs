//! Per-receiver containment of transient receiver housekeeping failures (#3069).
//!
//! A receiver handoff that fails with `Unavailable` has already revoked its
//! local authority and kept its durable lease: `OwnershipCoordinator::handoff`
//! releases the lease only after the receiving runtime confirms revocation.
//! Without containment that one error stops the whole process. With it, the
//! receiver stays fenced and is retried on later housekeeping ticks, paced by
//! the coordinator's cleanup pacing, while healthy owners keep serving.
//! Every other error, including Stale, IncompatibleProtocol, Invalid and any
//! membership failure, still stops the process exactly as before.
use layer2_core::agent_ownership::{Authority, OwnershipCoordinator, OwnershipError};
use layer2_core::AgentId;
use std::{
    collections::BTreeMap,
    future::Future,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

/// Bounded like the other receiver work in one housekeeping pass.
const RETRIES_PER_PASS: usize = 4;

#[derive(Default)]
pub(super) struct ContainedHandoffs {
    enabled: AtomicBool,
    cursor: AtomicUsize,
    pending: Mutex<BTreeMap<AgentId, Arc<Authority>>>,
}

impl ContainedHandoffs {
    pub(super) fn enable(&self) {
        self.enabled.store(true, Ordering::Release);
    }

    /// Keep a transiently failed receiver for retry, or return the error.
    pub(super) fn contain(
        &self,
        stage: &'static str,
        authority: &Arc<Authority>,
        error: OwnershipError,
    ) -> Result<(), OwnershipError> {
        if !self.enabled.load(Ordering::Acquire) || !matches!(error, OwnershipError::Unavailable(_))
        {
            return Err(error);
        }
        authority.log_receiver_handoff_failure(
            stage,
            &error,
            "Receiver handoff unavailable; receiver stays fenced and is retried",
        );
        self.pending
            .lock()
            .unwrap()
            .insert(authority.token.agent_id.clone(), authority.clone());
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn pending(&self) -> usize {
        self.pending.lock().unwrap().len()
    }

    /// Retry a bounded set of contained receivers through `handoff`.
    ///
    /// An entry whose exact authority the coordinator no longer owns is
    /// dropped: a later acquisition replaced it or its release completed.
    /// Before each retry, an entry whose token no longer holds a live durable
    /// lease is forgotten, so this process can acquire the Agent again just as
    /// a restart would allow. `Ok(true)` drops it, `Ok(false)` and
    /// `Unavailable` keep it, and any other error is returned to stop the
    /// process as before. Returned ids have no newer local authority, so their
    /// per-Agent housekeeping timers can be cleared.
    pub(super) async fn retry<F, Fut>(
        &self,
        coordinator: &OwnershipCoordinator,
        mut handoff: F,
    ) -> Result<Vec<AgentId>, OwnershipError>
    where
        F: FnMut(Arc<Authority>) -> Fut,
        Fut: Future<Output = Result<bool, OwnershipError>>,
    {
        let batch: Vec<_> = {
            let pending = self.pending.lock().unwrap();
            if pending.is_empty() {
                return Ok(Vec::new());
            }
            // Rotate so a few persistently failing receivers cannot starve others.
            let start = self.cursor.fetch_add(RETRIES_PER_PASS, Ordering::Relaxed) % pending.len();
            pending
                .values()
                .cycle()
                .skip(start)
                .take(RETRIES_PER_PASS.min(pending.len()))
                .cloned()
                .collect()
        };
        let owned = coordinator.owned().await;
        let mut completed = Vec::new();
        for authority in batch {
            let id = authority.token.agent_id.clone();
            let retained = owned.iter().any(|current| Arc::ptr_eq(current, &authority));
            let done = if !retained {
                true
            } else {
                // Lease custody first: it is checked on every pass, unpaced.
                match coordinator.forget_unconfirmed_retirement(&authority).await {
                    Ok(true) => true,
                    Ok(false) | Err(OwnershipError::Unavailable(_)) => {
                        match handoff(authority.clone()).await {
                            Ok(done) => done,
                            Err(OwnershipError::Unavailable(_)) => false,
                            Err(error) => return Err(error),
                        }
                    }
                    Err(error) => return Err(error),
                }
            };
            if done {
                let mut pending = self.pending.lock().unwrap();
                if pending
                    .get(&id)
                    .is_some_and(|current| Arc::ptr_eq(current, &authority))
                {
                    pending.remove(&id);
                }
                let newer = owned.iter().any(|current| {
                    current.token.agent_id == id && !Arc::ptr_eq(current, &authority)
                });
                if !newer {
                    completed.push(id);
                }
            }
        }
        Ok(completed)
    }
}

#[cfg(test)]
#[path = "containment_tests.rs"]
mod tests;
