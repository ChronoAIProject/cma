use super::*;
#[path = "retirement_async_tests.rs"]
mod asynchronous;
#[path = "finalization_tests.rs"]
mod finalization;
#[path = "retirement_fixture.rs"]
mod fixture;
#[path = "retirement_recovery_tests.rs"]
mod recovery;
#[path = "retention_tests.rs"]
mod retention;
use crate::sandbox::channel_retirement::model::RetirementPhase;
use fixture::RetirementFixture;
use layer2_core::sandbox::store::{AgentLogStore, AgentStore};

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_retirement_records_scoped_cleanup_preserves_history_and_holds_retention() {
    let f = RetirementFixture::new(0).await;
    let before = f.sandbox().await;
    let target = f.f.original.record.placement.target().clone();
    let history =
        f.f.original
            .resources
            .agent_log(&target.agent_id, 0, 100)
            .await
            .unwrap();
    f.start().await;
    for _ in 0..5 {
        f.step().await;
    }
    let after = f.sandbox().await;
    assert_eq!(after.status, SandboxStatus::Retiring);
    assert_eq!(after.work.step, SandboxWorkStep::MarkRetired);
    assert!(after.runtime.is_none());
    assert!(after.credential.current.is_none());
    assert!(after.credential.pending.is_none());
    assert!(after.credential.revoke_debt.is_empty());
    assert_eq!(f.stops.calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.f.state.revoke_count.load(Ordering::SeqCst), 1);
    assert_eq!(f.adapter.teardowns.load(Ordering::SeqCst), 1);
    assert!(f
        .f
        .original
        .custody
        .list(&after.id)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        f.receipt().await.unwrap().phase,
        RetirementPhase::RuntimeGone
    );
    assert_eq!(f.receipt().await.unwrap().runtime, before.runtime);
    assert!(f
        .f
        .original
        .resources
        .get_agent(&target.agent_id)
        .await
        .unwrap()
        .is_some());
    assert_eq!(
        f.f.original
            .resources
            .agent_log(&target.agent_id, 0, 100)
            .await
            .unwrap(),
        history
    );
    assert!(
        f.f.original
            .allocations
            .channel_allocation(&f.f.original.record.id)
            .await
            .unwrap()
            .unwrap()
            == f.f.original.record
    );
    f.step().await;
    let held = f.sandbox().await;
    assert_eq!(held.work.step, SandboxWorkStep::MarkRetired);
    assert!(held.purge_after.is_none());
    assert!(held.retired_at.is_none());
    assert_eq!(f.adapter.teardowns.load(Ordering::SeqCst), 1);
    let audits = f.db().collection::<Document>("channel_retirement_audits");
    assert_eq!(audits.count_documents(doc! {}).await.unwrap(), 6);
    assert!(f
        .db()
        .collection::<Document>("sandboxes")
        .find_one(doc! {"_id":after.id.as_str()})
        .await
        .unwrap()
        .unwrap()
        .contains_key("channel_storage_retention"));
    let audit = audits
        .find_one(doc! {"action":"key_revoked"})
        .await
        .unwrap()
        .unwrap();
    let actor = crate::store::sandbox_mongo::record_bson(&f.f.original.lease.actor_context().actor)
        .unwrap();
    assert_eq!(audit.get("human_actor"), Some(&actor));
    assert!(!format!("{audit:?}").contains(crate::auth::keys::bridge::test_fixture::HUMAN_SECRET));
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_retirement_refuses_any_additional_or_misindexed_child_before_stop_agents() {
    let f = RetirementFixture::new(0).await;
    let target = f.f.original.record.placement.target().clone();
    f.start().await;
    for (name, id) in [
        ("workspaces", target.workspace_id.as_str()),
        ("worktrees", target.worktree_id.as_str()),
        ("agents", target.agent_id.as_str()),
        ("agent_creation_provenance", target.agent_id.as_str()),
    ] {
        let rows = f.db().collection::<Document>(name);
        let mut extra = rows.find_one(doc! {"_id":id}).await.unwrap().unwrap();
        let extra_id = format!("extra-{id}");
        extra.insert("_id", &extra_id);
        // Nested parent references remain visible even if a query mirror was damaged.
        if name != "agent_creation_provenance" {
            extra.insert("sandbox_id", "another-root");
        }
        extra.insert("name", "extra-manual-child");
        if name == "agents" {
            extra.insert("creation_key", "extra-manual-child");
            extra.insert("status", "stopped");
        }
        rows.insert_one(extra).await.unwrap();
        let before = f.sandbox().await;
        f.step().await;
        let held = f.sandbox().await;
        assert_eq!(held.work.step, SandboxWorkStep::StopAgents);
        assert_eq!(held.runtime, before.runtime);
        assert_eq!(held.credential, before.credential);
        assert!(f.receipt().await.is_none());
        assert_eq!(f.stops.calls.load(Ordering::SeqCst), 0);
        assert_eq!(f.f.state.revoke_count.load(Ordering::SeqCst), 0);
        assert_eq!(f.adapter.teardowns.load(Ordering::SeqCst), 0);
        rows.delete_one(doc! {"_id":extra_id}).await.unwrap();
        f.retry().await;
    }
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_retirement_settles_known_pending_and_prior_debt_keys_atomically() {
    for steps in [1, 2] {
        let f = RetirementFixture::new(steps).await;
        let before = f.sandbox().await;
        if steps == 1 {
            assert!(before.credential.pending.is_some());
        } else {
            assert_eq!(before.credential.revoke_debt.len(), 1);
        }
        f.start().await;
        for _ in 0..6 {
            f.step().await;
        }
        let after = f.sandbox().await;
        assert_eq!(after.work.step, SandboxWorkStep::MarkRetired);
        assert!(after.credential.current.is_none());
        assert!(after.credential.pending.is_none());
        assert!(after.credential.revoke_debt.is_empty());
        assert!(f
            .f
            .original
            .custody
            .list(&after.id)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(f.f.state.revoke_count.load(Ordering::SeqCst), 2);
        assert_eq!(f.adapter.teardowns.load(Ordering::SeqCst), 1);
        let receipt = f.receipt().await.unwrap();
        assert_eq!(receipt.keys.len(), 2);
        assert!(receipt.keys.iter().all(|key| key.revoked));
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_retirement_refuses_replaced_origin_and_missing_original_human_grant() {
    let f = RetirementFixture::new(0).await;
    let target = f.f.original.record.placement.target().clone();
    f.start().await;
    let rows = f.db().collection::<Document>("agent_creation_provenance");
    let before = rows
        .find_one(doc! {"_id":target.agent_id.as_str()})
        .await
        .unwrap()
        .unwrap();
    rows.update_one(
        doc! {"_id":target.agent_id.as_str()},
        doc! {"$set":{"record.creation_origin":{"kind":"manual"}}},
    )
    .await
    .unwrap();
    f.step().await;
    assert_eq!(f.stops.calls.load(Ordering::SeqCst), 0);
    assert!(f.receipt().await.is_none());
    rows.replace_one(doc! {"_id":target.agent_id.as_str()}, before)
        .await
        .unwrap();
    f.retry().await;
    f.db()
        .collection::<Document>("bridge_credentials")
        .update_many(doc! {}, doc! {"$unset":{"control_plane":""}})
        .await
        .unwrap();
    f.step().await;
    assert_eq!(f.stops.calls.load(Ordering::SeqCst), 0);
    assert!(f.receipt().await.is_none());
    assert_eq!(f.f.state.revoke_count.load(Ordering::SeqCst), 0);
    f.close().await;
}
