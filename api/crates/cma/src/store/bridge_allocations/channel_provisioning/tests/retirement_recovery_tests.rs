use super::*;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_retirement_holds_a_cancelled_unbound_revival_reservation_before_effects() {
    use super::super::lifecycle_tests::lifecycle_fixture::LifecycleFixture;
    let lifecycle = LifecycleFixture::new().await;
    lifecycle.pause().await;
    lifecycle.revive().await;
    lifecycle.step().await; // Verify the retained checkpoint.
    lifecycle.step().await; // Reserve generation before provider IO.
    let reserved = lifecycle.sandbox().await;
    let generation = reserved
        .lifecycle
        .runtime_provision
        .as_ref()
        .unwrap()
        .generation;
    assert!(generation > 1);
    assert!(reserved.runtime.is_none());
    let f = RetirementFixture::from_renewal(lifecycle.f);
    f.start().await;
    let retired = f.sandbox().await;
    assert!(retired.lifecycle.operation.is_none());
    assert!(retired.lifecycle.runtime_provision.is_none());
    assert_eq!(retired.lifecycle.runtime_generation_high_water, generation);
    for _ in 0..2 {
        f.step().await;
        let held = f.sandbox().await;
        assert_eq!(held.work.step, SandboxWorkStep::StopAgents);
        assert_eq!(held.credential, reserved.credential);
        assert!(held.runtime.is_none());
        assert!(f.receipt().await.is_none());
        assert_eq!(f.stops.calls.load(Ordering::SeqCst), 0);
        assert_eq!(f.f.state.revoke_count.load(Ordering::SeqCst), 0);
        assert_eq!(f.adapter.teardowns.load(Ordering::SeqCst), 0);
        f.retry().await;
    }
    assert_eq!(
        f.f.original.custody.list(&retired.id).await.unwrap().len(),
        1
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_retirement_cannot_publish_stopped_without_the_original_durable_intent() {
    use crate::{
        bridge::provisioning::retirement::ChannelRetirementAuthority,
        sandbox::worker::LiveAgentStopper,
        store::bridge_allocations::channel_retirement::RetirementChange,
    };
    let f = RetirementFixture::new(0).await;
    // An Agent may already be terminal before retirement is admitted.
    f.stops
        .stop_live_agents(&f.sandbox().await, None, "fixture-prior-stop")
        .await
        .unwrap();
    f.start().await;
    let now = SystemTime::now();
    let sandbox =
        f.f.original
            .resources
            .claim_sandbox_work(
                &f.f.original.record.placement.target().sandbox_id,
                "retirement-missing-intent",
                layer2_core::rfc3339_at(now),
                layer2_core::rfc3339_at(now + Duration::from_secs(60)),
            )
            .await
            .unwrap()
            .unwrap();
    let permit = ChannelRetirementAuthority::fixture(
        f.f.original.record.clone(),
        f.f.original.authority.clone(),
        f.f.original.lease.clone(),
        sandbox.clone(),
    );
    assert!(f
        .f
        .original
        .allocations
        .change_channel_retirement(
            &permit,
            &f.f.original.custody,
            RetirementChange::Stopped,
            now
        )
        .await
        .is_err());
    assert!(f.receipt().await.is_none());
    assert_eq!(f.sandbox().await, sandbox);
    assert_eq!(f.f.state.revoke_count.load(Ordering::SeqCst), 0);
    assert_eq!(f.adapter.teardowns.load(Ordering::SeqCst), 0);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_retirement_lost_key_delete_keeps_custody_until_authorized_absence_receipt() {
    let f = RetirementFixture::new(0).await;
    f.start().await;
    f.step().await;
    f.step().await;
    let before = f.sandbox().await;
    f.f.state.revoke_unknown.store(true, Ordering::SeqCst);
    f.step().await;
    assert_eq!(f.f.state.revoke_count.load(Ordering::SeqCst), 1);
    assert_eq!(f.sandbox().await.credential, before.credential);
    assert_eq!(f.receipt().await.unwrap().phase, RetirementPhase::Revoke);
    assert_eq!(
        f.f.original.custody.list(&before.id).await.unwrap().len(),
        1
    );
    f.retry().await;
    f.step().await;
    assert_eq!(f.f.state.revoke_count.load(Ordering::SeqCst), 1);
    assert!(f
        .f
        .original
        .custody
        .list(&before.id)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        f.receipt().await.unwrap().phase,
        RetirementPhase::CredentialsCleared
    );
    assert_eq!(f.sandbox().await.runtime, before.runtime);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_retirement_late_key_audit_failure_rolls_back_custody_debt_and_progress() {
    let f = RetirementFixture::new(2).await;
    f.start().await;
    f.step().await;
    f.step().await;
    let before = f.sandbox().await;
    let receipt = f.receipt().await.unwrap();
    f.db().run_command(doc!{"collMod":"channel_retirement_audits","validator":{"action":{"$ne":"key_revoked"}}}).await.unwrap();
    f.step().await;
    assert_eq!(f.f.state.revoke_count.load(Ordering::SeqCst), 1);
    assert_eq!(f.sandbox().await.credential, before.credential);
    assert_eq!(f.receipt().await.unwrap(), receipt);
    assert_eq!(
        f.f.original.custody.list(&before.id).await.unwrap().len(),
        2
    );
    assert_eq!(f.adapter.teardowns.load(Ordering::SeqCst), 0);
    f.db()
        .run_command(doc! {"collMod":"channel_retirement_audits","validator":{}})
        .await
        .unwrap();
    f.retry().await;
    f.step().await;
    assert_eq!(f.f.state.revoke_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.f.original.custody.list(&before.id).await.unwrap().len(),
        1
    );
    assert!(f.sandbox().await.credential.revoke_debt.is_empty());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_retirement_lost_teardown_holds_until_complete_inventory_proves_absence() {
    let f = RetirementFixture::new(0).await;
    f.start().await;
    for _ in 0..4 {
        f.step().await;
    }
    let before = f.sandbox().await;
    assert_eq!(
        f.receipt().await.unwrap().phase,
        RetirementPhase::RuntimeIntent
    );
    f.adapter.lose_teardown.store(true, Ordering::SeqCst);
    f.adapter
        .inventory_unavailable
        .store(true, Ordering::SeqCst);
    f.step().await;
    assert_eq!(f.adapter.teardowns.load(Ordering::SeqCst), 1);
    assert_eq!(f.sandbox().await.runtime, before.runtime);
    assert_eq!(
        f.receipt().await.unwrap().phase,
        RetirementPhase::RuntimeIntent
    );
    f.retry().await;
    f.step().await;
    assert_eq!(f.adapter.teardowns.load(Ordering::SeqCst), 1);
    assert_eq!(f.sandbox().await.runtime, before.runtime);
    f.adapter
        .inventory_unavailable
        .store(false, Ordering::SeqCst);
    f.retry().await;
    f.step().await;
    assert_eq!(f.adapter.teardowns.load(Ordering::SeqCst), 1);
    assert!(f.sandbox().await.runtime.is_none());
    assert_eq!(
        f.receipt().await.unwrap().phase,
        RetirementPhase::RuntimeGone
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_retirement_late_runtime_audit_failure_retains_exact_teardown_intent() {
    let f = RetirementFixture::new(0).await;
    f.start().await;
    for _ in 0..4 {
        f.step().await;
    }
    let before = f.sandbox().await;
    f.db().run_command(doc!{"collMod":"channel_retirement_audits","validator":{"action":{"$ne":"runtime_gone"}}}).await.unwrap();
    f.step().await;
    assert_eq!(f.adapter.teardowns.load(Ordering::SeqCst), 1);
    assert_eq!(f.sandbox().await.runtime, before.runtime);
    assert_eq!(
        f.receipt().await.unwrap().phase,
        RetirementPhase::RuntimeIntent
    );
    f.db()
        .run_command(doc! {"collMod":"channel_retirement_audits","validator":{}})
        .await
        .unwrap();
    f.retry().await;
    f.step().await;
    assert_eq!(f.adapter.teardowns.load(Ordering::SeqCst), 1);
    assert!(f.sandbox().await.runtime.is_none());
    assert_eq!(
        f.receipt().await.unwrap().phase,
        RetirementPhase::RuntimeGone
    );
    f.close().await;
}
