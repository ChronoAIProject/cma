use super::*;
use l3::owner_protocol::{ReceiverRevocation, TransportAuthority};
use layer2_core::agent_ownership::{test_support::MemoryOwnershipStore, Authority};
use std::sync::atomic::AtomicUsize;

struct Handoff {
    runtime: Runtime,
    store: Arc<MemoryOwnershipStore>,
    old_store: Arc<MemoryOwnershipStore>,
    old: Arc<OwnershipCoordinator>,
    peer: Arc<OwnershipCoordinator>,
    authority: Arc<Authority>,
    agent: AgentId,
}

impl Handoff {
    async fn new(failure: &'static str, enabled: bool) -> Self {
        let runtime = Runtime::start_with_legacy_compatibility(failure, enabled).await;
        let store = Arc::new(MemoryOwnershipStore::at_fixed_time(1));
        let old_store = store.client();
        let old = OwnershipCoordinator::register(old_store.clone(), member("old"))
            .await
            .unwrap();
        let agent = AgentId("legacy-agent".into());
        let authority = old.acquire(&agent).await.unwrap().unwrap();
        let peer = OwnershipCoordinator::register(store.client(), member("peer"))
            .await
            .unwrap();
        assert!(peer.acquire(&agent).await.unwrap().is_none());
        Self {
            runtime,
            store,
            old_store,
            old,
            peer,
            authority,
            agent,
        }
    }

    async fn attempt_owned_control(&self) {
        let handle = l3::RuntimeHandle::new(HANDLE);
        self.runtime
            .transport
            .bind_native_runtime(&handle, "fixture-sandbox", &"a".repeat(64))
            .await
            .unwrap();
        assert!(self
            .authority
            .run(
                self.runtime
                    .transport
                    .exchange(&handle, l3::wire::RequestFrame::Resources,)
            )
            .await
            .is_err());
        assert!(self.authority.receiver_cancelled());
        self.old.begin_drain().await;
    }

    async fn settle(&self) -> Result<(), layer2_core::agent_ownership::OwnershipError> {
        self.old
            .handoff(
                &self.authority,
                self.authority.revoke_transports_after_local_drain(),
            )
            .await
    }

    async fn assert_original_lease(&self) {
        assert_eq!(self.store.membership().await.unwrap().now_ms, 1);
        assert_eq!(
            self.store.owner(&self.agent).await.unwrap().unwrap().token,
            self.authority.token,
        );
        assert!(self.peer.acquire(&self.agent).await.unwrap().is_none());
    }

    async fn assert_peer_took_over(&self, revocations: usize) {
        assert_eq!(self.store.membership().await.unwrap().now_ms, 1);
        let peer = self.peer.acquire(&self.agent).await.unwrap().unwrap();
        assert_eq!(peer.token.epoch, self.authority.token.epoch + 1);
        assert_eq!(self.runtime.revocations(), revocations);
        self.runtime.assert_counts(1);
    }
}

#[tokio::test]
async fn legacy_receiver_handoff_releases_before_expiry_after_local_drain() {
    let fixture = Handoff::new("receiverless", true).await;
    fixture.attempt_owned_control().await;
    fixture.assert_original_lease().await;
    fixture.settle().await.unwrap();
    fixture.assert_peer_took_over(1).await;
}

#[tokio::test]
async fn legacy_receiver_handoff_retains_unknown_disabled_changed_and_possible_confirmation() {
    for (failure, enabled, replace_after_revocation, confirmations) in [
        ("receiverless", false, false, 0),
        ("receiverless_unknown_image", true, false, 0),
        ("receiverless", true, true, 0),
        ("receiverless_confirm_ambiguous", true, false, 1),
    ] {
        let fixture = Handoff::new(failure, enabled).await;
        fixture.attempt_owned_control().await;
        fixture
            .runtime
            .counts
            .replace_after_revocation
            .store(replace_after_revocation, Ordering::SeqCst);
        assert!(fixture.settle().await.is_err(), "{failure}");
        fixture.assert_original_lease().await;
        assert_eq!(fixture.runtime.revocations(), 1);
        fixture
            .runtime
            .assert_counts_with_confirmation(1, confirmations);
    }
}

#[tokio::test]
async fn legacy_receiver_handoff_revalidates_if_exact_token_release_fails() {
    let fixture = Handoff::new("receiverless", true).await;
    fixture.attempt_owned_control().await;
    fixture.old_store.partition(true);
    assert!(fixture.settle().await.is_err());
    fixture.old_store.partition(false);
    fixture.assert_original_lease().await;
    assert_eq!(fixture.runtime.revocations(), 1);
    fixture
        .runtime
        .counts
        .replace_after_revocation
        .store(true, Ordering::SeqCst);
    assert!(fixture.settle().await.is_err());
    fixture.assert_original_lease().await;
    assert_eq!(fixture.runtime.revocations(), 2);
}

#[derive(Default)]
struct OneShotAcknowledgement {
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl ReceiverRevocation for OneShotAcknowledgement {
    fn identity(&self) -> String {
        "supported-sibling".into()
    }

    async fn revoke(&self) -> l3::Result<()> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok(())
        } else {
            Err(l3::AdapterError::Unavailable(
                "fixture runtime retired after its acknowledgement".into(),
            ))
        }
    }
}

#[tokio::test]
async fn legacy_receiver_handoff_retries_negative_proof_without_reopening_acknowledged_sibling() {
    let fixture = Handoff::new("receiverless", true).await;
    let acknowledged = Arc::new(OneShotAcknowledgement::default());
    fixture
        .authority
        .register_receiver(acknowledged.clone())
        .unwrap();
    fixture.attempt_owned_control().await;
    fixture.old_store.partition(true);
    assert!(fixture.settle().await.is_err());
    fixture.old_store.partition(false);
    fixture.assert_original_lease().await;
    assert_eq!(acknowledged.calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.runtime.revocations(), 1);

    fixture.settle().await.unwrap();
    assert_eq!(acknowledged.calls.load(Ordering::SeqCst), 1);
    fixture.assert_peer_took_over(2).await;
}

struct UnresolvedSibling;

#[async_trait::async_trait]
impl ReceiverRevocation for UnresolvedSibling {
    fn identity(&self) -> String {
        "zzz-sibling".into()
    }

    async fn revoke(&self) -> l3::Result<()> {
        Err(l3::AdapterError::Unavailable(
            "fixture acknowledgement lost".into(),
        ))
    }
}

#[tokio::test]
async fn legacy_receiver_handoff_cannot_settle_an_unresolved_sibling() {
    let fixture = Handoff::new("receiverless", true).await;
    fixture
        .authority
        .register_receiver(Arc::new(UnresolvedSibling))
        .unwrap();
    fixture.attempt_owned_control().await;
    assert!(fixture.settle().await.is_err());
    fixture.assert_original_lease().await;
    assert_eq!(fixture.runtime.revocations(), 1);
}
