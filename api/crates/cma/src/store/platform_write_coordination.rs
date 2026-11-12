//! Queue local writers of the platform period row so they do not abort each
//! other's transactions (concurrency rule 5).
//!
//! Scheduling only, like `AgentWrites`: every writer still opens a fresh
//! transaction, and other replicas or separately opened stores coordinate
//! through Mongo alone. Taken after the Agent write lock, before the session.
use layer2_core::sandbox::{store::AgentConversationMutation, AgentLogKind};
use tokio::sync::{Mutex, MutexGuard};

use super::sandbox_mongo::MongoSandboxStore;

#[derive(Default)]
pub(super) struct PlatformWrites(Mutex<()>);

/// Process-local platform spend coordination shared by every clone of a
/// store: the period-row writer queue and the stop-read cache.
#[derive(Default)]
pub(super) struct PlatformState {
    pub(super) writes: PlatformWrites,
    pub(super) stop_cache: std::sync::Mutex<super::budget::PlatformStopCache>,
}

impl PlatformWrites {
    pub(super) async fn lock(&self) -> MutexGuard<'_, ()> {
        self.0.lock().await
    }
}

impl MongoSandboxStore {
    /// Held across a transaction that may write the period row, unless the
    /// last settings read showed no enabled platform policy.
    pub(super) async fn platform_write_guard(&self) -> Option<MutexGuard<'_, ()>> {
        if !self.platform_enabled_hint() {
            return None;
        }
        Some(self.platform.writes.lock().await)
    }

    /// Turn entry and terminal turn mutations reserve or settle liability; a
    /// retained-usage write without the lock is still correct, only unscheduled.
    pub(super) async fn conversation_platform_guard(
        &self,
        mutation: &AgentConversationMutation,
    ) -> Option<MutexGuard<'_, ()>> {
        let writes_period = mutation.agent_logs.iter().any(|log| {
            matches!(
                log,
                AgentLogKind::TurnStarted { .. }
                    | AgentLogKind::TurnCompleted { .. }
                    | AgentLogKind::TurnFailed { .. }
                    | AgentLogKind::TurnInterrupted { .. }
            )
        });
        if !writes_period {
            return None;
        }
        self.platform_write_guard().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn one_writer_at_a_time_and_a_cancelled_waiter_releases_its_turn() {
        let writes = PlatformWrites::default();
        let held = writes.lock().await;
        assert!(
            tokio::time::timeout(Duration::from_millis(10), writes.lock())
                .await
                .is_err()
        );
        drop(held);
        drop(
            tokio::time::timeout(Duration::from_secs(1), writes.lock())
                .await
                .expect("a released lock must admit the next writer"),
        );
    }
}
