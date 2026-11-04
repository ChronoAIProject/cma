//! Avoid local writers repeatedly aborting each other's Mongo transactions.
//!
//! This is scheduling only: every writer still opens a fresh transaction and
//! validates durable revisions, leases and ownership. Other replicas and
//! separately opened stores continue to coordinate through Mongo alone.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use layer2_core::AgentId;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

#[derive(Default)]
pub(super) struct AgentWrites {
    pending: Mutex<HashMap<AgentId, Weak<AsyncMutex<()>>>>,
}

impl AgentWrites {
    pub(super) async fn lock(&self, id: &AgentId) -> OwnedMutexGuard<()> {
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
        lock.lock_owned().await
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
}
