use std::future::Future;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use tokio::sync::watch;
use tokio::time::Instant;

use super::{OwnershipError, OwnershipStore, OwnershipToken};

tokio::task_local! {
    static EXECUTION_AUTHORITY: Arc<Authority>;
}

/// The token captured at the start of this execution, never a lookup of whatever
/// epoch happens to be current when a late provider result finally returns.
pub fn execution_owner() -> Option<OwnershipToken> {
    execution_authority().map(|authority| authority.token.clone())
}

pub fn execution_authority() -> Option<Arc<Authority>> {
    EXECUTION_AUTHORITY.try_with(Arc::clone).ok()
}

#[cfg(test)]
#[path = "authority_retry_evidence_tests.rs"]
mod retry_evidence_tests;

/// Monotonic local cancellation complements the durable and receiving-runtime
/// fences. A revoked/expired authority can never be revived by a late renewal.
pub struct Authority {
    pub(super) retry_evidence:
        std::sync::OnceLock<Arc<layer3_adapter::owner_protocol::OwnershipEvidence>>,
    pub token: OwnershipToken,
    pub(super) retiring: AtomicBool,
    receiver_cancelled: AtomicBool,
    pub(super) receiver_failure:
        std::sync::OnceLock<layer3_adapter::owner_protocol::OwnershipFailure>,
    pub(super) receiver_failure_recording: std::sync::OnceLock<watch::Receiver<bool>>,
    pub(super) handoff_complete: tokio::sync::Mutex<bool>,
    deadline: watch::Sender<Option<Instant>>,
    pub(super) store: Option<Arc<dyn OwnershipStore>>,
    pub(super) receivers: std::sync::Mutex<
        std::collections::BTreeMap<
            String,
            Arc<dyn layer3_adapter::owner_protocol::ReceiverRevocation>,
        >,
    >,
}

impl Authority {
    pub fn new(token: OwnershipToken, deadline: Instant) -> Arc<Self> {
        let (deadline, _) = watch::channel(Some(deadline));
        Arc::new(Self {
            retry_evidence: Default::default(),
            token,
            retiring: AtomicBool::new(false),
            receiver_cancelled: AtomicBool::new(false),
            receiver_failure: Default::default(),
            receiver_failure_recording: Default::default(),
            handoff_complete: tokio::sync::Mutex::new(false),
            deadline,
            store: None,
            receivers: Default::default(),
        })
    }

    pub(super) fn with_store(
        token: OwnershipToken,
        deadline: Instant,
        store: Arc<dyn OwnershipStore>,
    ) -> Arc<Self> {
        let (deadline, _) = watch::channel(Some(deadline));
        Arc::new(Self {
            retry_evidence: Default::default(),
            token,
            retiring: AtomicBool::new(false),
            receiver_cancelled: AtomicBool::new(false),
            receiver_failure: Default::default(),
            receiver_failure_recording: Default::default(),
            handoff_complete: tokio::sync::Mutex::new(false),
            deadline,
            store: Some(store),
            receivers: Default::default(),
        })
    }

    pub fn is_current(&self) -> bool {
        if self.receiver_cancelled() {
            return false;
        }
        self.deadline
            .borrow()
            .is_some_and(|deadline| Instant::now() < deadline)
    }

    pub fn renew_until(&self, next: Instant) -> Result<(), OwnershipError> {
        let mut renewed = false;
        self.deadline.send_modify(|deadline| {
            if !self.receiver_cancelled()
                && deadline.is_some_and(|current| Instant::now() < current)
                && Instant::now() < next
            {
                *deadline = Some(next);
                renewed = true;
            } else {
                *deadline = None;
            }
        });
        if renewed {
            Ok(())
        } else {
            Err(OwnershipError::Stale)
        }
    }

    /// Receiver failure cancels one Agent, independently of process membership.
    pub fn receiver_cancelled(&self) -> bool {
        self.receiver_cancelled.load(Ordering::Acquire)
    }

    pub fn receiver_failure(&self) -> Option<layer3_adapter::owner_protocol::OwnershipFailure> {
        self.receiver_failure.get().copied()
    }

    /// A reused authority may already have sent confirmations. Never invent an
    /// initial-handshake history by installing the observer after that point.
    pub fn observe_retry_evidence(
        &self,
    ) -> Option<Arc<layer3_adapter::owner_protocol::OwnershipEvidence>> {
        let receivers = self.receivers.lock().unwrap();
        if let Some(evidence) = self.retry_evidence.get() {
            return Some(evidence.clone());
        }
        if !receivers.is_empty() {
            return None;
        }
        Some(self.retry_evidence.get_or_init(Default::default).clone())
    }

    pub(super) fn cancel_receiver(&self) {
        // Publish the cause before revoking so concurrent maintenance can
        // distinguish receiver cancellation from genuine ownership loss.
        self.deadline.send_modify(|deadline| {
            // A receiver callback arriving after generic revocation/expiry
            // must not relabel genuine ownership loss as receiver cancellation.
            if deadline.is_some_and(|until| Instant::now() < until) {
                self.receiver_cancelled.store(true, Ordering::Release);
            }
            *deadline = None;
        });
    }

    pub fn revoke(&self) {
        self.deadline.send_replace(None);
    }

    pub async fn revoked(&self) {
        let mut deadline = self.deadline.subscribe();
        loop {
            let Some(until) = *deadline.borrow_and_update() else {
                return;
            };
            if until <= Instant::now() {
                self.revoke();
                return;
            }
            tokio::select! {
                changed = deadline.changed() => if changed.is_err() { return },
                _ = tokio::time::sleep_until(until) => (),
            }
        }
    }

    /// Spawned tasks must explicitly capture this authority and enter their own
    /// scope; Tokio task locals intentionally do not transfer implicitly.
    pub async fn run<F: Future>(
        self: &Arc<Self>,
        operation: F,
    ) -> Result<F::Output, OwnershipError> {
        tokio::select! {
            biased;
            _ = self.revoked() => Err(OwnershipError::Stale),
            result = EXECUTION_AUTHORITY.scope(self.clone(), layer3_adapter::owner_protocol::scope(self.clone(), operation)) => {
                if self.is_current() { Ok(result) } else { Err(OwnershipError::Stale) }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentId;
    use std::time::Duration;

    fn authority() -> Arc<Authority> {
        Authority::new(
            OwnershipToken {
                agent_id: AgentId("agent".into()),
                instance_id: "owner".into(),
                epoch: 1,
                protocol: super::super::PROTOCOL_VERSION,
            },
            Instant::now() + Duration::from_secs(30),
        )
    }

    #[tokio::test(start_paused = true)]
    async fn expiry_cancels_pending_provider_result_and_late_renewal_cannot_revive_it() {
        let authority = authority();
        let (result, receive) = tokio::sync::oneshot::channel::<()>();
        let pending = tokio::spawn({
            let authority = authority.clone();
            async move { authority.run(receive).await }
        });
        tokio::time::advance(Duration::from_secs(30)).await;
        assert!(matches!(pending.await.unwrap(), Err(OwnershipError::Stale)));
        assert!(result.send(()).is_err());
        assert_eq!(
            authority.renew_until(Instant::now() + Duration::from_secs(30)),
            Err(OwnershipError::Stale)
        );
        assert!(!authority.is_current());
    }

    #[tokio::test(start_paused = true)]
    async fn scoped_token_is_exact_and_revocation_cancels_all_waiters() {
        let authority = authority();
        assert!(execution_owner().is_none());
        let token = authority
            .run(async { execution_owner().unwrap() })
            .await
            .unwrap();
        assert_eq!(token, authority.token);
        assert!(execution_owner().is_none());
        let mut waiters = tokio::task::JoinSet::new();
        for _ in 0..3 {
            let authority = authority.clone();
            waiters.spawn(async move { authority.run(std::future::pending::<()>()).await });
        }
        authority.revoke();
        while let Some(result) = waiters.join_next().await {
            assert_eq!(result.unwrap(), Err(OwnershipError::Stale));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn successful_renewal_keeps_waiters_until_new_deadline() {
        let authority = authority();
        let pending = tokio::spawn({
            let authority = authority.clone();
            async move {
                authority.revoked().await;
            }
        });
        tokio::time::advance(Duration::from_secs(20)).await;
        authority
            .renew_until(Instant::now() + Duration::from_secs(30))
            .unwrap();
        tokio::time::advance(Duration::from_secs(10)).await;
        assert!(authority.is_current());
        assert!(!pending.is_finished());
        tokio::time::advance(Duration::from_secs(20)).await;
        pending.await.unwrap();
        assert!(!authority.is_current());
    }
}
