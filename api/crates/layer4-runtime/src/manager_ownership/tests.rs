use super::*;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("cma-owner-test-{}", uuid::Uuid::new_v4())))
    }
    fn registry(&self) -> ManagerOwnership {
        ManagerOwnership::new(self.0.clone())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if self.0.exists() {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
}

fn owner(epoch: u64) -> TransportOwner {
    TransportOwner {
        protocol: OWNER_PROTOCOL_VERSION,
        agent_id: "agent".into(),
        instance_id: format!("instance-{epoch}"),
        epoch,
    }
}

async fn confirm(registry: &ManagerOwnership, owner: TransportOwner) {
    let challenge = registry.challenge(owner.clone()).await.unwrap();
    // Production renews Mongo here. Its later lease cannot expire before the
    // receiver deadline measured when this challenge was issued.
    let response = registry
        .confirm(OwnerConfirmation {
            owner: owner.clone(),
            nonce: challenge.nonce,
        })
        .await
        .unwrap();
    assert_eq!(response.owner, owner);
}

#[tokio::test]
async fn incompatible_protocol_is_rejected_and_legacy_sessions_do_not_exhaust_cache() {
    let fixture = Fixture::new();
    let registry = fixture.registry();
    let mut incompatible = owner(1);
    incompatible.protocol = OWNER_PROTOCOL_VERSION + 1;
    assert!(registry.challenge(incompatible).await.is_err());
    for index in 0..=MAX_GATES {
        let session = registry
            .session(&format!("legacy-{index}"), None)
            .await
            .unwrap();
        assert!(session.permit().await.is_ok());
    }
    assert!(
        !fixture.0.exists(),
        "legacy traffic never persists ownership metadata"
    );
    confirm(&registry, owner(1)).await;
    assert!(registry.session("agent", None).await.is_err());
}

#[tokio::test(start_paused = true)]
async fn challenge_deadline_rejects_delayed_confirmation_without_clock_synchronization() {
    let fixture = Fixture::new();
    let registry = fixture.registry();
    let challenge = registry.challenge(owner(1)).await.unwrap();
    tokio::time::advance(Duration::from_millis(RECEIVER_LEASE_MS)).await;
    assert!(registry
        .confirm(OwnerConfirmation {
            owner: owner(1),
            nonce: challenge.nonce
        })
        .await
        .is_err());
    assert!(registry.session("agent", Some(owner(1))).await.is_err());
    assert!(
        !fixture.0.exists(),
        "an unconfirmed lease writes no authority record"
    );
}

#[tokio::test(start_paused = true)]
async fn two_and_three_owners_fence_old_sessions_and_stale_output_acknowledgements() {
    for count in [2, 3] {
        let fixture = Fixture::new();
        let registry = fixture.registry();
        let output = crate::bridge::OutputRetention::default();
        output.retain(b"terminal-frame").await;
        let mut previous = Vec::new();
        for epoch in 1..=count {
            confirm(&registry, owner(epoch)).await;
            let current = registry.session("agent", Some(owner(epoch))).await.unwrap();
            for old in &previous {
                assert!(!acknowledge(old, &output).await);
            }
            assert_eq!(output.snapshot(0).await.unwrap().frames.len(), 1);
            previous.push(current);
        }
        assert!(acknowledge(previous.last().unwrap(), &output).await);
        assert!(output.snapshot(0).await.unwrap().frames.is_empty());
    }
}

async fn acknowledge(session: &OwnerSession, output: &crate::bridge::OutputRetention) -> bool {
    let Ok(_permit) = session.permit().await else {
        return false;
    };
    output.acknowledge(output.stream_id(), 1).await
}

#[tokio::test(start_paused = true)]
async fn takeover_waits_for_effect_in_progress_and_rechecks_queued_input() {
    use std::{
        future::{poll_fn, Future},
        task::Poll,
    };
    let fixture = Fixture::new();
    let registry = Arc::new(fixture.registry());
    confirm(&registry, owner(1)).await;
    let old = registry.session("agent", Some(owner(1))).await.unwrap();
    let challenge = registry.challenge(owner(2)).await.unwrap();
    let permit = old.permit().await.unwrap();
    let mut takeover = Box::pin(registry.confirm(OwnerConfirmation {
        owner: owner(2),
        nonce: challenge.nonce,
    }));
    poll_fn(|cx| {
        assert!(takeover.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    let mut queued_old_input = Box::pin(old.permit());
    poll_fn(|cx| {
        assert!(queued_old_input.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    drop(permit);
    takeover.await.unwrap();
    assert!(queued_old_input.await.is_err());
    assert!(old.permit().await.is_err());
    assert!(registry.session("agent", Some(owner(2))).await.is_ok());
}

#[tokio::test(start_paused = true)]
async fn same_epoch_renewal_is_not_blocked_by_a_long_running_control_effect() {
    let fixture = Fixture::new();
    let registry = fixture.registry();
    confirm(&registry, owner(1)).await;
    let session = registry.session("agent", Some(owner(1))).await.unwrap();
    let permit = session.permit().await.unwrap();
    let half_lease = Duration::from_millis(RECEIVER_LEASE_MS / 2);
    tokio::time::advance(half_lease).await;
    confirm(&registry, owner(1)).await;
    // Cross the original deadline while remaining inside the renewed lease.
    tokio::time::advance(half_lease + Duration::from_millis(1)).await;
    drop(permit);
    assert!(session.permit().await.is_ok());
    tokio::time::advance(half_lease - Duration::from_millis(1)).await;
    assert!(session.permit().await.is_err());
}

#[tokio::test(start_paused = true)]
async fn revocation_is_permanent_for_epoch_and_restart_preserves_watermark_without_lease() {
    let fixture = Fixture::new();
    let registry = fixture.registry();
    confirm(&registry, owner(3)).await;
    let old = registry.session("agent", Some(owner(3))).await.unwrap();
    registry.revoke(owner(3)).await.unwrap();
    assert!(old.permit().await.is_err());
    assert!(registry.challenge(owner(3)).await.is_err());
    let restarted = fixture.registry();
    assert!(restarted.session("agent", Some(owner(3))).await.is_err());
    assert!(restarted.challenge(owner(2)).await.is_err());
    assert!(restarted.challenge(owner(3)).await.is_err());
    confirm(&restarted, owner(4)).await;
    assert!(restarted.session("agent", Some(owner(4))).await.is_ok());
}

#[tokio::test(start_paused = true)]
async fn legacy_default_is_unchanged_and_explicit_rollback_cannot_revive_previous_sessions() {
    let fixture = Fixture::new();
    let registry = fixture.registry();
    let original = registry.session("agent", None).await.unwrap();
    assert!(original.permit().await.is_ok());
    assert!(!fixture.0.exists());
    confirm(&registry, owner(1)).await;
    assert!(original.permit().await.is_err());
    assert!(registry.session("agent", None).await.is_err());
    assert!(registry.reset_legacy(owner(1)).await.is_err());
    tokio::time::advance(Duration::from_millis(RECEIVER_LEASE_MS)).await;
    registry.reset_legacy(owner(1)).await.unwrap();
    assert!(registry
        .session("agent", None)
        .await
        .unwrap()
        .permit()
        .await
        .is_ok());
    assert!(original.permit().await.is_err());
    assert!(registry.challenge(owner(1)).await.is_err());
    confirm(&registry, owner(2)).await;
}
