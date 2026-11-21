use super::*;
use crate::agent_ownership::{
    test_support::MemoryOwnershipStore, Member, OwnershipStore, PROTOCOL_VERSION,
};
use layer3_adapter::owner_protocol::{ReceiverRevocation, TransportAuthority};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[path = "observation_tests.rs"]
mod observations;

struct Receiver {
    id: &'static str,
    ack: AtomicBool,
    calls: AtomicUsize,
    blocked: AtomicBool,
}
impl Receiver {
    fn new(id: &'static str) -> Arc<Self> {
        Arc::new(Self {
            id,
            ack: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
            blocked: AtomicBool::new(false),
        })
    }
}
#[async_trait::async_trait]
impl ReceiverRevocation for Receiver {
    fn identity(&self) -> String {
        self.id.into()
    }
    async fn revoke(&self) -> layer3_adapter::Result<()> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if self.blocked.load(Ordering::Relaxed) {
            std::future::pending::<()>().await;
        }
        if self.ack.load(Ordering::Relaxed) {
            Ok(())
        } else {
            Err(layer3_adapter::AdapterError::Unavailable(
                "synthetic receiver refusal".into(),
            ))
        }
    }
}

async fn fixture(
    enabled: bool,
) -> (
    Arc<MemoryOwnershipStore>,
    Arc<OwnershipCoordinator>,
    Arc<Authority>,
    Arc<Receiver>,
) {
    let store = Arc::new(MemoryOwnershipStore::default());
    let coordinator = OwnershipCoordinator::register_with_cleanup_pacing(
        store.clone(),
        Member {
            instance_id: "pacing-owner".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://owner.invalid".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 8,
        },
        enabled,
    )
    .await
    .unwrap();
    let authority = coordinator
        .acquire(&AgentId("agent-a".into()))
        .await
        .unwrap()
        .unwrap();
    let receiver = Receiver::new("binding-a");
    authority.register_receiver(receiver.clone()).unwrap();
    authority.receiver_confirmed("binding-a");
    (store, coordinator, authority, receiver)
}

#[tokio::test(start_paused = true)]
async fn no_ack_keeps_lease_and_cooldown_while_exact_ack_releases_once() {
    let (store, coordinator, authority, receiver) = fixture(true).await;
    assert!(coordinator.handoff_background(&authority).await.is_err());
    assert_eq!(
        store
            .owner(&authority.token.agent_id)
            .await
            .unwrap()
            .unwrap()
            .token,
        authority.token
    );
    assert!(
        coordinator
            .background_cleanup_deferred(&authority.token.agent_id)
            .await
    );
    assert!(!coordinator.handoff_background(&authority).await.unwrap());
    assert_eq!(receiver.calls.load(Ordering::Relaxed), 1);
    tokio::time::advance(Duration::from_millis(3300)).await;
    receiver.ack.store(true, Ordering::Relaxed);
    assert!(coordinator.handoff_background(&authority).await.unwrap());
    assert!(store
        .owner(&authority.token.agent_id)
        .await
        .unwrap()
        .is_none());
    assert!(
        !coordinator
            .background_cleanup_deferred(&authority.token.agent_id)
            .await
    );
    assert!(coordinator.handoff_background(&authority).await.unwrap());
    assert_eq!(receiver.calls.load(Ordering::Relaxed), 2);
}

#[tokio::test(start_paused = true)]
async fn disabled_and_explicit_handoff_preserve_immediate_attempts() {
    for enabled in [false, true] {
        let (store, coordinator, authority, receiver) = fixture(enabled).await;
        assert!(coordinator.handoff_background(&authority).await.is_err());
        if !enabled {
            assert!(coordinator.handoff_background(&authority).await.is_err());
            assert!(
                !coordinator
                    .background_cleanup_deferred(&authority.token.agent_id)
                    .await
            );
        }
        receiver.ack.store(true, Ordering::Relaxed);
        coordinator
            .handoff(&authority, authority.revoke_transports())
            .await
            .unwrap();
        assert!(
            !coordinator
                .background_cleanup_deferred(&authority.token.agent_id)
                .await
        );
        assert!(store
            .owner(&authority.token.agent_id)
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            receiver.calls.load(Ordering::Relaxed),
            if enabled { 2 } else { 3 }
        );
    }
}

#[tokio::test(start_paused = true)]
async fn concurrent_and_cancelled_attempts_cannot_duplicate_io_or_leak_inflight_state() {
    let (store, coordinator, authority, receiver) = fixture(true).await;
    receiver.blocked.store(true, Ordering::Relaxed);
    let running = tokio::spawn({
        let coordinator = coordinator.clone();
        let authority = authority.clone();
        async move { coordinator.handoff_background(&authority).await }
    });
    tokio::task::yield_now().await;
    assert!(!coordinator.handoff_background(&authority).await.unwrap());
    assert_eq!(receiver.calls.load(Ordering::Relaxed), 1);
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    assert!(coordinator
        .cleanup_pacing
        .state
        .lock()
        .unwrap()
        .active
        .is_empty());
    assert!(!coordinator.handoff_background(&authority).await.unwrap());
    assert!(store
        .owner(&authority.token.agent_id)
        .await
        .unwrap()
        .is_some());
    tokio::time::advance(Duration::from_millis(3300)).await;
    receiver.blocked.store(false, Ordering::Relaxed);
    receiver.ack.store(true, Ordering::Relaxed);
    assert!(coordinator.handoff_background(&authority).await.unwrap());
}

fn observation(epoch: u64, binding: &'static str, verified: bool) -> Arc<Authority> {
    let authority = Authority::new(
        OwnershipToken {
            agent_id: AgentId("agent-a".into()),
            instance_id: "owner".into(),
            epoch,
            protocol: PROTOCOL_VERSION,
        },
        tokio::time::Instant::now() + Duration::from_secs(100),
    );
    authority.register_receiver(Receiver::new(binding)).unwrap();
    if verified {
        authority.receiver_confirmed(binding);
    }
    authority
}

#[tokio::test(start_paused = true)]
async fn epoch_churn_and_unverified_binding_never_reset_an_episode() {
    let pacing = CleanupPacing::new(true, 1);
    drop(pacing.begin(&observation(1, "binding-a", true)).unwrap());
    assert!(pacing.begin(&observation(2, "binding-a", true)).is_none());
    assert!(pacing.begin(&observation(3, "binding-b", false)).is_none());
    let newer = observation(3, "binding-b", true);
    let mut attempt = pacing.begin(&newer).unwrap();
    attempt.confirmed();
    drop(attempt);
    assert!(!pacing.deferred(&newer.token.agent_id));
    assert!(pacing.state.lock().unwrap().bindings.is_empty());
}

#[tokio::test(start_paused = true)]
async fn healthy_peers_and_membership_continue_during_failed_receiver_cooldown() {
    let (_, coordinator, authority, _) = fixture(true).await;
    assert!(coordinator.handoff_background(&authority).await.is_err());
    let healthy = coordinator
        .acquire(&AgentId("agent-b".into()))
        .await
        .unwrap()
        .unwrap();
    coordinator.maintain().await.unwrap();
    assert!(coordinator.is_ready() && healthy.is_current());
    assert!(coordinator.handoff_background(&healthy).await.unwrap());
    assert!(!coordinator.handoff_background(&authority).await.unwrap());
}

#[tokio::test(start_paused = true)]
async fn late_old_ack_cannot_release_or_forget_newer_owner() {
    let (store, coordinator, old, receiver) = fixture(true).await;
    TransportAuthority::revoke(old.as_ref());
    assert!(coordinator.handoff_background(&old).await.is_err());
    for _ in 0..6 {
        tokio::time::advance(Duration::from_secs(3)).await;
        coordinator.maintain().await.unwrap();
    }
    let successor = coordinator
        .acquire(&old.token.agent_id)
        .await
        .unwrap()
        .unwrap();
    assert!(successor.token.epoch > old.token.epoch);
    receiver.ack.store(true, Ordering::Relaxed);
    // Release is idempotent for an obsolete exact token; it must leave the
    // replacement untouched even when the old receiver finally acknowledges.
    assert!(coordinator.handoff_background(&old).await.unwrap());
    assert_eq!(
        store
            .owner(&old.token.agent_id)
            .await
            .unwrap()
            .unwrap()
            .token,
        successor.token
    );
    assert!(successor.is_current());
}
