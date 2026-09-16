use super::*;
use layer2_core::bridge::allocation::AllocationError;
use crate::sandbox::channel_provisioning::{ChannelProvisioningDependencies, ChannelProvisioningDriver};
use crate::store::bridge_definitions::mongo_integration_tests::allocations::channel_provisioning::fixture::{Fixture, VerifiedHuman};
use layer2_core::sandbox::store::{SandboxEventStore, SandboxStore, SandboxWorkerStore};
use mongodb::bson::Document;
use std::sync::Arc;

async fn fresh() -> RetirementFixture {
    fresh_with_connection(true).await
}
async fn fresh_with_connection(published: bool) -> RetirementFixture {
    let mut original = Fixture::new().await;
    original.adapter =
        Arc::new(fixture::RetirementAdapter::new(original.adapter.clone()).with_finality());
    original.driver = ChannelProvisioningDriver::new(
        ChannelProvisioningDependencies {
            allocations: original.allocations.clone(),
            credentials: Arc::new(VerifiedHuman),
            issuer: original.server.issuer.clone(),
            custody: original.custody.clone(),
            adapter: original.adapter.clone(),
        },
        original.settings.clone(),
        "finalization-fresh-provisioning".into(),
    )
    .unwrap();
    let renewal = RenewalFixture::from_original(original).await;
    let registry = if published {
        crate::sandbox::channel_startup::test_support::publish_connection(
            renewal.original.resources.clone(),
            renewal.original.adapter.clone(),
            &renewal.original.record.placement.target().agent_id,
        )
        .await
    } else {
        Arc::new(crate::sandbox::LiveAgentRegistry::default())
    };
    let f = RetirementFixture::with_agents(renewal, registry, None);
    f.start().await;
    for _ in 0..6 {
        f.step().await;
    }
    assert_eq!(f.sandbox().await.work.step, SandboxWorkStep::MarkRetired);
    assert_eq!(
        f.db()
            .collection::<Document>("channel_provider_finality_receipts")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    f
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_finalization_unfinished_agent_work_remains_held() {
    let f = fresh_with_connection(false).await;
    f.step().await;
    assert_unsealed(&f).await;
    f.close().await;
}
async fn raw(f: &RetirementFixture) -> Document {
    f.db()
        .collection::<Document>("sandboxes")
        .find_one(doc! {"_id": f.f.original.record.placement.target().sandbox_id.as_str()})
        .await
        .unwrap()
        .unwrap()
}
async fn assert_unsealed(f: &RetirementFixture) {
    assert_eq!(f.sandbox().await.status, SandboxStatus::Retiring);
    assert!(!raw(f).await.contains_key("channel_retirement_finalization"));
    for (collection, id) in [
        ("bridge_allocations", f.f.original.record.id.as_str()),
        (
            "agents",
            f.f.original.record.placement.target().agent_id.as_str(),
        ),
    ] {
        let row = f
            .db()
            .collection::<Document>(collection)
            .find_one(doc! {"_id": id})
            .await
            .unwrap()
            .unwrap();
        assert!(!row.contains_key("channel_history_seal"), "{collection}");
    }
    assert_eq!(
        f.db()
            .collection::<Document>("channel_retirement_finalizations")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_finalization_fresh_no_home_retires_and_retains_readable_history() {
    let f = fresh().await;
    let id = f.f.original.record.placement.target().agent_id.clone();
    let before = f.f.original.resources.agent_log(&id, 0, 100).await.unwrap();
    assert!(!before.is_empty());
    f.step().await;
    let sandbox = f.sandbox().await;
    assert_eq!(
        sandbox.status,
        SandboxStatus::Retired,
        "events: {:?}; agent: {:?}",
        f.f.original
            .resources
            .sandbox_events(&sandbox.id, 0, 100)
            .await
            .unwrap(),
        f.f.original.resources.get_agent(&id).await.unwrap()
    );
    assert_eq!(sandbox.work.step, SandboxWorkStep::None);
    assert!(sandbox.retired_at.is_some() && sandbox.purge_after.is_none());
    let receipt = raw(&f).await;
    assert!(receipt.contains_key("channel_retirement_finalization"));
    assert_eq!(
        f.f.original.resources.agent_log(&id, 0, 100).await.unwrap(),
        before
    );
    assert!(f
        .f
        .original
        .resources
        .append_agent_log(&id, layer2_core::now_rfc3339(), before[0].kind.clone())
        .await
        .is_err());
    assert!(f
        .f
        .original
        .resources
        .purge_sandbox(&sandbox.id, sandbox.revision, "2999-01-01T00:00:00Z")
        .await
        .is_err());
    assert!(!f
        .worker
        .run_one(&sandbox.id, SystemTime::now())
        .await
        .unwrap());
    assert_eq!(
        raw(&f).await,
        receipt,
        "repeated worker run cannot issue finalization twice"
    );
    let mut changed = sandbox.clone();
    changed.revision += 1;
    assert!(f
        .f
        .original
        .resources
        .save_sandbox(sandbox.revision, changed)
        .await
        .is_err());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_finalization_transaction_abort_rolls_back_all_seals_then_retries_once() {
    let f = fresh().await;
    f.db().run_command(doc! {"collMod": "channel_retirement_finalizations", "validator": {"_id": {"$eq": "impossible-fixture-id"}}}).await.unwrap();
    f.step().await;
    assert_unsealed(&f).await;
    f.db()
        .run_command(doc! {"collMod": "channel_retirement_finalizations", "validator": {}})
        .await
        .unwrap();
    f.retry().await;
    f.step().await;
    assert_eq!(f.sandbox().await.status, SandboxStatus::Retired);
    assert_eq!(
        f.db()
            .collection::<Document>("channel_retirement_finalizations")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_finalization_uncertain_commit_replays_without_partial_or_duplicate_receipts() {
    use crate::bridge::provisioning::retirement::ChannelRetirementAuthority;
    use crate::store::bridge_allocations::channel_retirement::RetirementChange;
    let f = fresh().await;
    let now = SystemTime::now();
    let claimed =
        f.f.original
            .resources
            .claim_sandbox_work(
                &f.sandbox().await.id,
                "uncertain-finalization",
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
        claimed,
    );
    let admin = f.f.original.client.database("admin");
    admin.run_command(doc! {"configureFailPoint": "failCommand", "mode": {"times": 5_i32},
        "data": {"failCommands": ["commitTransaction"], "writeConcernError": {
            "code": 50_i32, "codeName": "MaxTimeMSExpired", "errmsg": "fixture uncertain finalization commit"
        }}}).await.unwrap();
    let result =
        f.f.original
            .allocations
            .change_channel_retirement(
                &permit,
                &f.f.original.custody,
                RetirementChange::Finalize,
                now,
            )
            .await;
    admin
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();
    assert!(matches!(result, Err(AllocationError::Unavailable)));
    // The driver cannot know whether the injected commit outcome took effect.
    // Recovery accepts the exact receipt or retries an entirely unsealed root.
    let mut session = f.f.original.client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    let row = f
        .db()
        .collection::<Document>("sandboxes")
        .find_one(doc! {"_id": f.f.original.record.placement.target().sandbox_id.as_str()})
        .session(&mut session)
        .await
        .unwrap()
        .unwrap();
    let observed = crate::store::sandboxes::read_sandbox(&row).unwrap();
    let committed = observed.status == SandboxStatus::Retired;
    assert_eq!(
        row.contains_key("channel_retirement_finalization"),
        committed
    );
    for (collection, id) in [
        ("bridge_allocations", f.f.original.record.id.as_str()),
        (
            "agents",
            f.f.original.record.placement.target().agent_id.as_str(),
        ),
    ] {
        let row = f
            .db()
            .collection::<Document>(collection)
            .find_one(doc! {"_id":id})
            .session(&mut session)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.contains_key("channel_history_seal"), committed);
    }
    let receipt = f
        .db()
        .collection::<Document>("channel_retirement_finalizations")
        .find_one(doc! {"_id": &f.f.original.record.id})
        .session(&mut session)
        .await
        .unwrap();
    assert_eq!(receipt.is_some(), committed);
    session.abort_transaction().await.unwrap();
    if !committed {
        // An unknown commit may become majority visible after this snapshot.
        // The revision check prevents a retry from overwriting its receipt.
        for _ in 0..20 {
            if f.sandbox().await.status == SandboxStatus::Retired {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        if f.sandbox().await.status == SandboxStatus::Retiring {
            f.retry().await;
            f.step().await;
        }
    }
    let sandbox = f.sandbox().await;
    assert_eq!(sandbox.status, SandboxStatus::Retired);
    let before = raw(&f).await;
    assert!(!f
        .worker
        .run_one(&sandbox.id, SystemTime::now())
        .await
        .unwrap());
    assert_eq!(raw(&f).await, before);
    assert_eq!(
        f.db()
            .collection::<Document>("channel_retirement_finalizations")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_finalization_missing_or_corrupt_provider_proof_never_publishes_retired() {
    for missing in [true, false] {
        let f = fresh().await;
        let receipts = f
            .db()
            .collection::<Document>("channel_provider_finality_receipts");
        if missing {
            receipts.delete_many(doc! {}).await.unwrap();
        } else {
            receipts
                .update_many(doc! {}, doc! {"$set": {"sha256": "0".repeat(64)}})
                .await
                .unwrap();
        }
        f.step().await;
        assert_unsealed(&f).await;
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_finalization_missing_log_or_reference_is_held_without_partial_seal() {
    for collection in ["agent_log", "channel_history_references"] {
        let f = fresh().await;
        assert_eq!(
            f.db()
                .collection::<Document>(collection)
                .delete_one(doc! {})
                .await
                .unwrap()
                .deleted_count,
            1
        );
        f.step().await;
        assert_unsealed(&f).await;
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_finalization_reader_rejects_changed_receipt_and_keeps_purge_guard() {
    let f = fresh().await;
    f.step().await;
    let sandbox = f.sandbox().await;
    assert_eq!(sandbox.status, SandboxStatus::Retired);
    f.db().collection::<Document>("sandboxes").update_one(doc! {"_id": sandbox.id.as_str()},
        doc! {"$set": {"channel_retirement_finalization.provider_finality_sha256": "0".repeat(64)}}).await.unwrap();
    assert!(f
        .f
        .original
        .resources
        .get_sandbox(&sandbox.id)
        .await
        .is_err());
    assert!(f
        .f
        .original
        .resources
        .checkpoint_objects
        .purge_sandbox(sandbox.id.as_str())
        .await
        .is_err());
    f.close().await;
}
