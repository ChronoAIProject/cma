//! Avoid local writers repeatedly aborting each other's Mongo transactions.
//!
//! This is scheduling only: every writer still opens a fresh transaction and
//! validates durable revisions, leases and ownership. Other replicas and
//! separately opened stores continue to coordinate through Mongo alone.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use layer2_core::AgentId;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

tokio::task_local! {
    /// Bound a cleanup preflight requeue and any reserve publication to the
    /// same original work lease, independently of the Agent owner lease.
    pub(crate) static CLEANUP_RETRY_DEADLINE: tokio::time::Instant;
}

#[derive(Default)]
pub(super) struct AgentWrites {
    pending: Mutex<HashMap<AgentId, Weak<AsyncMutex<()>>>>,
}

impl AgentWrites {
    fn lock_for(&self, id: &AgentId) -> Arc<AsyncMutex<()>> {
        let lock = {
            let mut pending = self
                .pending
                .lock()
                .expect("Agent write scheduling poisoned");
            // Keep at most 64 inactive entries; active owners/waiters hold the
            // strong references. Never evict a live lock, including a waiter.
            if pending.len() >= 64 {
                pending.retain(|_, lock| lock.strong_count() > 0);
            }
            match pending.get(id).and_then(Weak::upgrade) {
                Some(lock) => lock,
                None => {
                    let lock = Arc::new(AsyncMutex::new(()));
                    pending.insert(id.clone(), Arc::downgrade(&lock));
                    lock
                }
            }
        };
        lock
    }

    pub(super) async fn lock(&self, id: &AgentId) -> OwnedMutexGuard<()> {
        self.lock_for(id).lock_owned().await
    }

    /// Wait for a local Agent writer only until the caller's existing budget.
    /// Cancelling this wait is safe because `lock_owned` does not retain a
    /// waiter after its future is dropped.
    pub(super) async fn lock_until(
        &self,
        id: &AgentId,
        deadline: tokio::time::Instant,
    ) -> Option<OwnedMutexGuard<()>> {
        tokio::time::timeout_at(deadline, self.lock_for(id).lock_owned())
            .await
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn cancelled_waiter_does_not_block_the_agent_or_other_agents() {
        let writes = AgentWrites::default();
        let id = AgentId("agent-a".into());
        let held = writes.lock(&id).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(10), writes.lock(&id))
                .await
                .is_err()
        );
        let other = tokio::time::timeout(
            Duration::from_secs(1),
            writes.lock(&AgentId("agent-b".into())),
        )
        .await
        .expect("unrelated Agents must remain independent");
        drop(other);
        drop(held);
        drop(
            tokio::time::timeout(Duration::from_secs(1), writes.lock(&id))
                .await
                .expect("cancellation must release the waiter"),
        );
    }

    #[tokio::test]
    async fn pruning_keeps_live_locks_and_bounds_retired_agent_entries() {
        let writes = AgentWrites::default();
        let id = AgentId("live-agent".into());
        let held = writes.lock(&id).await;
        for index in 0..256 {
            drop(writes.lock(&AgentId(format!("retired-{index}"))).await);
        }
        assert!(writes.pending.lock().unwrap().len() <= 64);
        assert!(
            tokio::time::timeout(Duration::from_millis(10), writes.lock(&id))
                .await
                .is_err(),
            "pruning must not create a second lock for a live Agent"
        );
        drop(held);
        drop(
            tokio::time::timeout(Duration::from_secs(1), writes.lock(&id))
                .await
                .unwrap(),
        );
    }

    #[tokio::test]
    async fn deadline_bounds_a_waiting_control_writer() {
        let writes = AgentWrites::default();
        let id = AgentId("agent-a".into());
        let held = writes.lock(&id).await;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(10);
        assert!(writes.lock_until(&id, deadline).await.is_none());
        drop(held);
        assert!(writes
            .lock_until(&id, tokio::time::Instant::now() + Duration::from_secs(1))
            .await
            .is_some());
    }
}
