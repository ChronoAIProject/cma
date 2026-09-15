//! Disposable tests build a real Ready runtime and retain its original Agent
//! and session while the separate Channel lane replaces only its scoped key.
#[path = "renewal/authority_tests.rs"]
mod authority_tests;
#[path = "renewal/discovery_tests.rs"]
mod discovery_tests;
#[path = "renewal/fixture.rs"]
mod fixture;
#[path = "renewal/guard_tests.rs"]
mod guard_tests;
use crate::sandbox::{channel_renewal::model::RenewalPhase, SandboxCredentialCustody};
use fixture::RenewalFixture;
use layer2_core::sandbox::{store::SandboxWorkerStore, SandboxStatus, SandboxWorkStep};
use layer3_adapter::wire::WireOperation;
use mongodb::bson::{doc, Document};
use std::{
    sync::atomic::Ordering,
    time::{Duration, SystemTime},
};

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn real_channel_renewal_keeps_original_allocation_runtime_agent_and_session() {
    let f = RenewalFixture::new().await;
    let before = f.snapshot().await;
    let agents = f
        .original
        .client
        .database(&f.original.database)
        .collection::<Document>("agents");
    let agent = agents
        .find_one(doc! {"_id":before.allocation.placement.target().agent_id.as_str()})
        .await
        .unwrap()
        .unwrap();
    f.step().await.unwrap();
    let minted = f.snapshot().await;
    assert_eq!(
        minted.renewal.as_ref().unwrap().phase,
        RenewalPhase::Deliver
    );
    assert_eq!(
        minted.sandbox.credential.current,
        before.sandbox.credential.current
    );
    assert_eq!(
        minted
            .sandbox
            .credential
            .pending
            .as_ref()
            .unwrap()
            .generation,
        2
    );
    assert!(minted.sandbox.pause_eligibility().is_err());
    assert_eq!(minted.sandbox.work.step, SandboxWorkStep::None);
    assert!(f
        .original
        .resources
        .due_sandbox_work("2999-01-01T00:00:00Z", 10)
        .await
        .unwrap()
        .is_empty());
    f.step().await.unwrap();
    let delivered = f.snapshot().await;
    assert_eq!(
        delivered.renewal.as_ref().unwrap().phase,
        RenewalPhase::Revoke
    );
    assert_eq!(
        delivered
            .sandbox
            .credential
            .current
            .as_ref()
            .unwrap()
            .generation,
        2
    );
    assert_eq!(delivered.sandbox.credential.revoke_debt.len(), 1);
    f.step().await.unwrap();
    let after = f.snapshot().await;
    assert_eq!(
        after.renewal.as_ref().unwrap().phase,
        RenewalPhase::Complete
    );
    assert!(after.allocation == before.allocation);
    assert_eq!(after.sandbox.runtime, before.sandbox.runtime);
    assert_eq!(after.sandbox.lifecycle, before.sandbox.lifecycle);
    assert_eq!(after.sandbox.status, SandboxStatus::Active);
    assert!(after.sandbox.work.lease.is_none());
    assert!(after.sandbox.pause_eligibility().is_ok());
    assert_eq!(
        agents
            .find_one(doc! {"_id":after.allocation.placement.target().agent_id.as_str()})
            .await
            .unwrap()
            .unwrap(),
        agent
    );
    assert!(f
        .original
        .custody
        .get(&after.sandbox.id, 1)
        .await
        .unwrap()
        .is_none());
    assert!(f
        .original
        .custody
        .get(&after.sandbox.id, 2)
        .await
        .unwrap()
        .is_some());
    assert_eq!(f.state.mint_count.load(Ordering::SeqCst), 1);
    assert_eq!(f.state.revoke_count.load(Ordering::SeqCst), 1);
    discovery_tests::require_renewal_audits(&f).await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_renewal_preflight_failure_has_no_intent_and_concurrent_mint_is_single() {
    let f = RenewalFixture::new().await;
    f.state.scopes_unavailable.store(true, Ordering::SeqCst);
    assert!(f.step().await.is_err());
    assert!(f.snapshot().await.renewal.is_none());
    assert_eq!(f.state.mint_count.load(Ordering::SeqCst), 0);
    f.state.scopes_unavailable.store(false, Ordering::SeqCst);
    let (a, b) = tokio::join!(f.step(), f.step());
    assert!(a.is_ok() || b.is_ok());
    assert_eq!(f.state.mint_count.load(Ordering::SeqCst), 1);
    assert!(matches!(
        f.snapshot().await.renewal.as_ref().unwrap().phase,
        RenewalPhase::Deliver | RenewalPhase::Revoke
    ));
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn unknown_channel_renewal_mint_holds_same_key_and_never_blind_retries() {
    let f = RenewalFixture::new().await;
    let before = f.snapshot().await;
    f.state.mint_unknown.store(true, Ordering::SeqCst);
    assert!(f.step().await.is_err());
    f.state.mint_unknown.store(false, Ordering::SeqCst);
    assert!(f.step().await.is_err());
    assert!(f.step().await.is_err());
    let held = f.snapshot().await;
    assert_eq!(
        held.renewal.as_ref().unwrap().phase,
        RenewalPhase::MintStarted
    );
    assert!(held.allocation == before.allocation);
    assert_eq!(held.sandbox.credential, before.sandbox.credential);
    assert_eq!(held.sandbox.runtime, before.sandbox.runtime);
    assert_eq!(f.state.mint_count.load(Ordering::SeqCst), 1);
    assert_eq!(f.state.revoke_count.load(Ordering::SeqCst), 0);
    assert!(f
        .original
        .allocations
        .due_channel_renewal_sandboxes(None, f.due)
        .await
        .unwrap()
        .is_empty());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn lost_channel_renewal_delivery_replays_same_generation_and_runtime_operation() {
    let f = RenewalFixture::new().await;
    f.step().await.unwrap();
    f.original
        .backend
        .lose_next_response(WireOperation::Credential);
    assert!(f.step().await.is_err());
    let pending = f.snapshot().await;
    assert_eq!(
        pending.renewal.as_ref().unwrap().phase,
        RenewalPhase::Deliver
    );
    expire_claim(&f).await;
    f.step().await.unwrap();
    let frames = f
        .original
        .backend
        .requests()
        .await
        .into_iter()
        .filter(|(_, frame)| frame.operation() == WireOperation::Credential)
        .collect::<Vec<_>>();
    assert_eq!(frames.len(), 3); // original key, lost replacement reply, replay
    assert_eq!(frames[1], frames[2]);
    assert_eq!(f.state.mint_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.snapshot()
            .await
            .sandbox
            .credential
            .current
            .as_ref()
            .unwrap()
            .generation,
        2
    );
    f.step().await.unwrap();
    f.close().await;
}

async fn expire_claim(f: &RenewalFixture) {
    let expired = layer2_core::rfc3339_at(SystemTime::now() - Duration::from_secs(2));
    let milliseconds = mongodb::bson::DateTime::parse_rfc3339_str(&expired)
        .unwrap()
        .timestamp_millis();
    f.original
        .client
        .database(&f.original.database)
        .collection::<Document>("sandboxes")
        .update_one(
            doc! {"_id":f.original.record.placement.target().sandbox_id.as_str()},
            doc! {"$set":{"channel_renewal.claim.expires_at_ms":milliseconds,
            "record.work.lease.expires_at":&expired,"work_lease_expires_at":expired}},
        )
        .await
        .unwrap();
}
