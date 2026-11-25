use std::future::Future;
use std::sync::{atomic::AtomicBool, Arc};

use tokio::sync::watch;
use tokio::time::Instant;

use super::{transport::RegisteredReceiver, OwnershipError, OwnershipStore, OwnershipToken};
mod provenance;
mod renewal;
mod retirement;
mod validity;
pub(super) use retirement::CleanupState;
use validity::Validity;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReceiverCancellation {
    CancelledWhileLive,
    AlreadySafelyCancelled,
    NotLive,
}

/// Monotonic local cancellation complements the durable and receiving-runtime
/// fences. A revoked/expired authority can never be revived by a late renewal.
pub struct Authority {
    pub(super) retry_evidence:
        std::sync::OnceLock<Arc<layer3_adapter::owner_protocol::OwnershipEvidence>>,
    pub token: OwnershipToken,
    pub(super) policy: super::MaintenancePolicy,
    pub(super) settlement_watch: layer3_adapter::owner_protocol::ReceiverSettlementWatch,
    // Diagnostic lineage only. Ownership fences use the token and receiver settlement.
    pub(super) sandbox_sha256: std::sync::OnceLock<String>,
    pub(super) retiring: AtomicBool,
    pub(super) receiver_failure:
        std::sync::OnceLock<layer3_adapter::owner_protocol::OwnershipFailure>,
    pub(super) receiver_failure_recording: std::sync::OnceLock<watch::Receiver<bool>>,
    pub(super) cleanup: tokio::sync::Mutex<CleanupState>,
    pub(super) renewal: std::sync::Mutex<Option<Arc<super::lease_operation::LeaseWrite>>>,
    validity: watch::Sender<Validity>,
    active_work: watch::Sender<usize>,
    pub(super) store: Option<Arc<dyn OwnershipStore>>,
    pub(super) receivers: std::sync::Mutex<std::collections::BTreeMap<String, RegisteredReceiver>>,
    pub(super) confirmed_receivers: std::sync::Mutex<std::collections::BTreeSet<String>>,
}

impl Authority {
    pub fn new(token: OwnershipToken, deadline: Instant) -> Arc<Self> {
        let validity = watch::channel(Validity::Live {
            until: deadline,
            heartbeat_mark: None,
        })
        .0;
        Arc::new(Self {
            retry_evidence: Default::default(),
            token,
            policy: super::MaintenancePolicy::Legacy,
            settlement_watch: Default::default(),
            sandbox_sha256: Default::default(),
            retiring: AtomicBool::new(false),
            receiver_failure: Default::default(),
            receiver_failure_recording: Default::default(),
            cleanup: tokio::sync::Mutex::new(CleanupState::default()),
            renewal: std::sync::Mutex::new(None),
            validity,
            active_work: watch::channel(0).0,
            store: None,
            receivers: Default::default(),
            confirmed_receivers: Default::default(),
        })
    }

    #[cfg(test)]
    pub(super) fn with_store(
        token: OwnershipToken,
        deadline: Instant,
        store: Arc<dyn OwnershipStore>,
    ) -> Arc<Self> {
        Self::with_store_policy(token, deadline, store, super::MaintenancePolicy::Legacy)
    }

    pub(super) fn with_store_policy(
        token: OwnershipToken,
        deadline: Instant,
        store: Arc<dyn OwnershipStore>,
        policy: super::MaintenancePolicy,
    ) -> Arc<Self> {
        let validity = watch::channel(Validity::Live {
            until: deadline,
            heartbeat_mark: None,
        })
        .0;
        Arc::new(Self {
            retry_evidence: Default::default(),
            token,
            policy,
            settlement_watch: Default::default(),
            sandbox_sha256: Default::default(),
            retiring: AtomicBool::new(false),
            receiver_failure: Default::default(),
            receiver_failure_recording: Default::default(),
            cleanup: tokio::sync::Mutex::new(CleanupState::default()),
            renewal: std::sync::Mutex::new(None),
            validity,
            active_work: watch::channel(0).0,
            store: Some(store),
            receivers: Default::default(),
            confirmed_receivers: Default::default(),
        })
    }

    pub(super) fn unpublished(
        token: OwnershipToken,
        store: Arc<dyn OwnershipStore>,
        policy: super::MaintenancePolicy,
    ) -> Arc<Self> {
        let mut authority = Self::with_store_policy(token, Instant::now(), store, policy);
        let inner = Arc::get_mut(&mut authority).unwrap();
        inner.validity.send_replace(Validity::Unpublished);
        inner.cleanup.get_mut().local_quiescent = true;
        inner
            .retiring
            .store(true, std::sync::atomic::Ordering::Release);
        authority
    }

    pub(super) fn is_unpublished(&self) -> bool {
        self.observe_validity() == Validity::Unpublished
    }

    pub fn is_current(&self) -> bool {
        self.observe_validity().deadline().is_some()
    }

    fn observe_validity(&self) -> Validity {
        let mut observed = Validity::Revoked;
        self.validity.send_if_modified(|state| {
            let previous = *state;
            state.observe(Instant::now());
            observed = *state;
            *state != previous
        });
        observed
    }

    pub(super) fn established_deadline(&self) -> Option<Instant> {
        self.observe_validity().deadline()
    }

    pub fn renew_until(&self, next: Instant) -> Result<(), OwnershipError> {
        let mut renewed = false;
        self.validity.send_modify(|state| {
            let now = Instant::now();
            state.observe(now);
            if state.deadline().is_some() && now < next {
                *state = Validity::Live {
                    until: next,
                    heartbeat_mark: None,
                };
                renewed = true;
            } else {
                state.revoke(now);
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
        matches!(*self.validity.borrow(), Validity::ReceiverCancelled { .. })
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

    pub(super) fn cancel_receiver(&self) -> ReceiverCancellation {
        // Publish the cause before revoking so concurrent maintenance can
        // distinguish receiver cancellation from genuine ownership loss.
        let mut outcome = ReceiverCancellation::NotLive;
        self.validity.send_modify(|state| {
            // A receiver callback arriving after generic revocation/expiry
            // must not relabel genuine ownership loss as receiver cancellation.
            state.observe(Instant::now());
            match *state {
                Validity::ReceiverCancelled { .. } => {
                    outcome = ReceiverCancellation::AlreadySafelyCancelled;
                }
                Validity::Live { until, .. } => {
                    *state = Validity::ReceiverCancelled {
                        previous_until: until,
                    };
                    outcome = ReceiverCancellation::CancelledWhileLive;
                }
                _ => (),
            }
        });
        outcome
    }

    pub fn revoke(&self) {
        self.validity
            .send_modify(|state| state.revoke(Instant::now()));
    }

    pub async fn revoked(&self) {
        let mut validity = self.validity.subscribe();
        loop {
            let Some(until) = validity.borrow_and_update().deadline() else {
                return;
            };
            if until <= Instant::now() {
                if self.observe_validity().deadline().is_none() {
                    return;
                }
                continue;
            }
            tokio::select! {
                changed = validity.changed() => if changed.is_err() { return },
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
        let _work = self.admit_work()?;
        tokio::select! {
            biased;
            _ = self.revoked() => Err(OwnershipError::Stale),
            result = EXECUTION_AUTHORITY.scope(self.clone(), layer3_adapter::owner_protocol::scope(self.clone(), operation)) => {
                if self.is_current() { Ok(result) } else { Err(OwnershipError::Stale) }
            }
        }
    }

    pub(super) fn admit_work(&self) -> Result<ActiveWork<'_>, OwnershipError> {
        let admitted = self.active_work.send_if_modified(|count| {
            if self.is_current() {
                *count += 1;
                true
            } else {
                false
            }
        });
        admitted
            .then(|| ActiveWork(&self.active_work))
            .ok_or(OwnershipError::Stale)
    }

    pub(super) async fn join_local_work(&self) -> Result<(), OwnershipError> {
        // Revocation closes admission before examining this count. Use the
        // admission lock to include any racing work that passed is_current.
        self.active_work
            .subscribe()
            .wait_for(|count| *count == 0)
            .await
            .map(|_| ())
            .map_err(|_| OwnershipError::Unavailable("local ownership join ended".into()))
    }
}

pub(super) struct ActiveWork<'a>(&'a watch::Sender<usize>);

impl Drop for ActiveWork<'_> {
    fn drop(&mut self) {
        self.0.send_modify(|count| *count -= 1);
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
    async fn expiry_observers_and_late_renewal_preserve_the_original_deadline() {
        for observer in 0..3 {
            let authority = authority();
            let until = authority.observe_validity().deadline().unwrap();
            tokio::time::advance(Duration::from_secs(30)).await;
            match observer {
                0 => assert!(!authority.is_current()),
                1 => authority.revoked().await,
                _ => assert_eq!(
                    authority.renew_until(Instant::now() + Duration::from_secs(30)),
                    Err(OwnershipError::Stale)
                ),
            }
            assert_eq!(
                *authority.validity.borrow(),
                Validity::Expired {
                    previous_until: until,
                    mark: None
                }
            );
            authority.revoke();
            assert_eq!(authority.cancel_receiver(), ReceiverCancellation::NotLive);
            assert_eq!(
                *authority.validity.borrow(),
                Validity::Expired {
                    previous_until: until,
                    mark: None
                }
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn generic_revocation_never_acquires_expiry_or_receiver_cancellation_cause() {
        let authority = authority();
        authority.revoke();
        tokio::time::advance(Duration::from_secs(31)).await;
        authority.revoked().await;
        assert_eq!(authority.cancel_receiver(), ReceiverCancellation::NotLive);
        assert_eq!(
            authority.renew_until(Instant::now() + Duration::from_secs(30)),
            Err(OwnershipError::Stale)
        );
        assert_eq!(*authority.validity.borrow(), Validity::Revoked);
    }

    #[tokio::test(start_paused = true)]
    async fn repeated_revocation_preserves_live_receiver_cancellation() {
        let authority = authority();
        let previous_until = authority.observe_validity().deadline().unwrap();
        assert_eq!(
            authority.cancel_receiver(),
            ReceiverCancellation::CancelledWhileLive
        );
        authority.revoke();
        tokio::time::advance(Duration::from_secs(31)).await;
        authority.revoke();
        assert_eq!(
            authority.cancel_receiver(),
            ReceiverCancellation::AlreadySafelyCancelled
        );
        assert!(authority.receiver_cancelled());
        assert!(!authority.is_current());
        assert_eq!(
            *authority.validity.borrow(),
            Validity::ReceiverCancelled { previous_until }
        );
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
