use super::containment_test_support::{manager, Store};
use super::*;
use layer3_adapter::owner_protocol::{
    ReceiverCleanup, ReceiverConfirmationCustody, ReceiverSettlementFailure as Failure,
    ReceiverSettlementWatch,
};
use layer3_adapter::owner_protocol::{ReceiverRevocation, TransportAuthority};
use std::sync::{atomic::Ordering, Arc};

struct ClassifiedReceiver {
    failure: Failure,
    hold: bool,
    entered: tokio::sync::Notify,
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl ReceiverRevocation for ClassifiedReceiver {
    fn identity(&self) -> String {
        "classified".into()
    }
    async fn revoke(&self) -> layer3_adapter::Result<()> {
        panic!("enabled retirement must use typed settlement")
    }
    async fn settle_typed(
        &self,
        _: ReceiverConfirmationCustody,
        outcome: &ReceiverSettlementWatch,
    ) -> Result<ReceiverCleanup, Failure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        outcome.record(self.failure);
        self.entered.notify_one();
        if self.hold {
            std::future::pending::<()>().await;
        }
        Err(self.failure)
    }
}

fn classified(failure: Failure, hold: bool) -> Arc<ClassifiedReceiver> {
    Arc::new(ClassifiedReceiver {
        failure,
        hold,
        entered: tokio::sync::Notify::new(),
        calls: Default::default(),
    })
}

struct UnclassifiedReceiver;
#[async_trait::async_trait]
impl ReceiverRevocation for UnclassifiedReceiver {
    fn identity(&self) -> String {
        "unclassified".into()
    }
    async fn revoke(&self) -> layer3_adapter::Result<()> {
        Err(layer3_adapter::AdapterError::Rejected(
            "synthetic protocol refusal".into(),
        ))
    }
}

#[tokio::test(start_paused = true)]
async fn unclassified_receiver_refusal_is_not_flattened_into_transient_pending() {
    let store = Arc::new(Store::default());
    let manager = manager(store.clone(), false, 2).await;
    let old = manager
        .acquire(&AgentId("old".into()))
        .await
        .unwrap()
        .unwrap();
    old.register_receiver(Arc::new(UnclassifiedReceiver))
        .unwrap();
    let result = manager.retirement_background(&old).await;
    assert!(
        matches!(result, Err(OwnershipError::Invalid(_))),
        "an unclassified receiver refusal must remain fatal, not become retryable Pending"
    );
    assert_eq!(store.release_calls.load(Ordering::SeqCst), 0);
    assert_eq!(old.receivers.lock().unwrap().len(), 1);
    assert_eq!(manager.owned().await.len(), 1);
    assert!(!old.is_current());
}

#[tokio::test(start_paused = true)]
async fn every_receiver_category_has_an_explicit_policy_and_retains_unsettled_custody() {
    for failure in [
        Failure::Deadline,
        Failure::Unavailable,
        Failure::IncompatibleProtocol,
        Failure::Invalid,
        Failure::AuthorityLost,
        Failure::Unclassified,
    ] {
        let store = Arc::new(Store::default());
        let manager = manager(store.clone(), false, 2).await;
        let old = manager
            .acquire(&AgentId("old".into()))
            .await
            .unwrap()
            .unwrap();
        let healthy = manager
            .acquire(&AgentId("healthy".into()))
            .await
            .unwrap()
            .unwrap();
        old.register_receiver(classified(failure, false)).unwrap();
        let result = manager.retirement_background(&old).await;
        match failure {
            Failure::Deadline | Failure::Unavailable => {
                assert_eq!(result.unwrap(), None);
                manager.maintain().await.unwrap();
                assert!(healthy.is_current());
                assert!(manager.is_ready());
            }
            Failure::IncompatibleProtocol => {
                assert_eq!(result, Err(OwnershipError::IncompatibleProtocol))
            }
            Failure::AuthorityLost => assert_eq!(result, Err(OwnershipError::Stale)),
            Failure::Invalid | Failure::Unclassified => {
                assert!(matches!(result, Err(OwnershipError::Invalid(_))))
            }
        }
        if !failure.is_transient() {
            assert!(manager.maintain().await.is_err());
            assert!(!manager.is_ready());
        }
        assert_eq!(store.release_calls.load(Ordering::SeqCst), 0);
        assert_eq!(old.receivers.lock().unwrap().len(), 1);
        assert_eq!(manager.owned().await.len(), 2);
    }
}

#[tokio::test(start_paused = true)]
async fn known_terminal_result_wins_over_diagnostic_timeout_or_cancelled_attempt() {
    for cancel in [false, true] {
        let store = Arc::new(Store::default());
        let manager = manager(store.clone(), false, 1).await;
        let old = manager
            .acquire(&AgentId("old".into()))
            .await
            .unwrap()
            .unwrap();
        let receiver = classified(Failure::IncompatibleProtocol, true);
        old.register_receiver(receiver.clone()).unwrap();
        let task = tokio::spawn({
            let manager = manager.clone();
            let old = old.clone();
            async move { manager.retirement_background(&old).await }
        });
        receiver.entered.notified().await;
        if cancel {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            assert_eq!(
                task.await.unwrap(),
                Err(OwnershipError::IncompatibleProtocol)
            );
        }
        assert_eq!(
            manager.retirement_background(&old).await,
            Err(OwnershipError::IncompatibleProtocol)
        );
        assert_eq!(receiver.calls.load(Ordering::SeqCst), 1);
        assert_eq!(store.release_calls.load(Ordering::SeqCst), 0);
        assert_eq!(old.receivers.lock().unwrap().len(), 1);
        assert_eq!(manager.owned().await.len(), 1);
        assert_eq!(
            manager.maintain().await.err(),
            Some(OwnershipError::IncompatibleProtocol)
        );
    }
}

#[tokio::test(start_paused = true)]
async fn transient_exchange_timeout_keeps_partial_ack_without_forging_completion() {
    let store = Arc::new(Store::default());
    let manager = manager(store.clone(), false, 1).await;
    let old = manager
        .acquire(&AgentId("old".into()))
        .await
        .unwrap()
        .unwrap();
    let acked = super::containment_test_support::Receiver::install(&old, "a", false);
    old.register_receiver(classified(Failure::Unavailable, true))
        .unwrap();
    assert_eq!(manager.retirement_background(&old).await.unwrap(), None);
    assert_eq!(acked.calls.load(Ordering::SeqCst), 1);
    assert_eq!(old.receivers.lock().unwrap().len(), 1);
    assert_eq!(store.release_calls.load(Ordering::SeqCst), 0);
    assert!(old.settlement_error().is_none());
    assert!(manager.maintain().await.is_ok());
}
