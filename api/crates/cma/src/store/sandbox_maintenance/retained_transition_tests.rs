use super::*;
use crate::store::runtime_canary::CanaryPolicy;
use crate::store::sandbox_maintenance::mongo_integration_tests::{active_sandbox, database};
use crate::store::sandbox_unpublished_recovery::mongo_integration_tests::home;
use layer2_core::sandbox::{SandboxWorkState, SandboxWorkStep};
use layer3_adapter::RetainedStartupPolicyPredecessor;
use mongodb::bson::Bson;

#[path = "retained_transition_tests/lease.rs"]
mod lease;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retained_runtime_grant_same_image_reserves_once_and_replays_after_policy_close() {
    let (client, name, store) = database().await;
    let db = client.database(&name);
    let (sandbox, grant) = retained_fixture(&store, "retained-transition").await;
    let request = grant.request.clone();
    let fresh = || RetainedRuntimeGrantPolicy {
        release_key: &grant.release_key,
        catalog_sha256: &grant.catalog_sha256,
        owner_scope_sha256: &grant.owner_scope_sha256,
        target_image: &request.target_image,
        target_config_digest: &request.target_config_digest,
    };
    CanaryPolicy::configure(
        &db,
        0,
        true,
        grant.catalog_sha256.clone(),
        grant.owner_scope_sha256.clone(),
    )
    .await
    .unwrap();
    assert!(store
        .issue_retained_runtime_grant(&db, &grant)
        .await
        .is_err());
    CanaryPolicy::configure_retained(
        &db,
        0,
        true,
        grant.catalog_sha256.clone(),
        grant.owner_scope_sha256.clone(),
    )
    .await
    .unwrap();
    store
        .issue_retained_runtime_grant(&db, &grant)
        .await
        .unwrap();
    let mut foreign = request.clone();
    foreign.owner = layer2_core::SubjectId("foreign".into());
    assert!(store
        .redeem_retained_runtime_grant(&db, foreign.clone(), Some(fresh()))
        .await
        .is_err());
    let before = store.get_sandbox(&sandbox.id).await.unwrap().unwrap();
    assert_eq!(before, sandbox);

    for hold in [Bson::Document(doc! {"phase": "requested"}), Bson::Null] {
        let before = held_rows(&store, &sandbox, hold).await;
        assert!(store
            .redeem_retained_runtime_grant(&db, request.clone(), Some(fresh()))
            .await
            .is_err());
        assert_eq!(rows(&store, &sandbox).await, before);
    }
    clear_hold(&store, &sandbox).await;

    let (left, right) = tokio::join!(
        store.redeem_retained_runtime_grant(&db, request.clone(), Some(fresh())),
        store.redeem_retained_runtime_grant(&db, request.clone(), Some(fresh())),
    );
    let receipt = left.unwrap();
    assert_eq!(right.unwrap(), receipt);
    assert_eq!(receipt.request, request);
    let admitted = store.get_sandbox(&sandbox.id).await.unwrap().unwrap();
    assert_eq!(admitted.revision, sandbox.revision + 1);
    assert_eq!(
        admitted.lifecycle.runtime_generation_high_water,
        receipt.successor_generation
    );
    assert_eq!(
        admitted.admitted_retained_runtime_transition(),
        Some(&receipt)
    );

    CanaryPolicy::configure_retained(
        &db,
        1,
        false,
        "e".repeat(64),
        grant.owner_scope_sha256.clone(),
    )
    .await
    .unwrap();
    assert_eq!(
        store
            .redeem_retained_runtime_grant(&db, request.clone(), None)
            .await
            .unwrap(),
        receipt
    );
    let mut drifted = request;
    drifted.predecessor.resource_version = "18".into();
    assert!(store
        .redeem_retained_runtime_grant(&db, drifted, None)
        .await
        .is_err());
    assert_eq!(
        store.get_sandbox(&sandbox.id).await.unwrap(),
        Some(admitted)
    );

    for hold in [Bson::Document(doc! {"phase": "requested"}), Bson::Null] {
        let before = held_rows(&store, &sandbox, hold).await;
        assert!(store
            .abort_retained_runtime_grant(&db, &sandbox.owner, &grant.request.grant_id)
            .await
            .is_err());
        assert_eq!(
            store
                .redeem_retained_runtime_grant(&db, grant.request.clone(), None)
                .await
                .unwrap(),
            receipt
        );
        assert_eq!(rows(&store, &sandbox).await, before);
    }
    clear_hold(&store, &sandbox).await;

    assert!(store
        .abort_retained_runtime_grant(&db, &foreign.owner, &grant.request.grant_id)
        .await
        .is_err());
    for _ in 0..2 {
        store
            .abort_retained_runtime_grant(&db, &sandbox.owner, &grant.request.grant_id)
            .await
            .unwrap();
    }
    let aborted = store.get_sandbox(&sandbox.id).await.unwrap().unwrap();
    assert_eq!(aborted.status, SandboxStatus::Paused);
    assert!(aborted.admitted_retained_runtime_transition().is_none());
    assert_eq!(aborted.lifecycle.home, sandbox.lifecycle.home);
    assert_eq!(
        aborted.lifecycle.suspended_runtime,
        sandbox.lifecycle.suspended_runtime
    );
    assert_eq!(
        aborted.lifecycle.runtime_generation_high_water,
        receipt.successor_generation
    );
    let stored = store
        .read_retained_runtime_grant(&sandbox.owner, &grant.request.grant_id)
        .await
        .unwrap();
    assert!(stored.aborted_at.is_some());
    assert_eq!(stored.receipt, Some(receipt.clone()));
    assert_eq!(
        store
            .redeem_retained_runtime_grant(&db, grant.request, None)
            .await
            .unwrap(),
        receipt
    );
    assert_eq!(store.get_sandbox(&sandbox.id).await.unwrap(), Some(aborted));
    db.drop().await.unwrap();
}

pub(crate) async fn retained_fixture(
    store: &MongoSandboxStore,
    id: &str,
) -> (Sandbox, RetainedRuntimeGrant) {
    let mut sandbox = active_sandbox(id);
    let image = format!("registry.example/runtime@sha256:{}", "a".repeat(64));
    let mut binding = sandbox.runtime.take().unwrap();
    binding.runtime_id = "agent-sandbox:member:sandbox-uid".into();
    binding.runtime_version = image.clone();
    sandbox.status = SandboxStatus::Paused;
    sandbox.lifecycle.suspended_runtime = Some(Box::new(binding));
    sandbox.lifecycle.home = Some(Box::new(home(&sandbox.id)));
    sandbox.work = SandboxWorkState::new(SandboxWorkStep::None, now_rfc3339());

    let attachment = sandbox
        .lifecycle
        .home
        .as_ref()
        .unwrap()
        .attachment()
        .unwrap();
    sandbox.lifecycle.home_recovery = Some(Box::new(layer2_core::sandbox::SandboxHomeRecovery {
        home_identity: attachment.identity().unwrap(),
        source_generation: 1,
        resume_pause: None,
    }));
    store.create_sandbox(sandbox.clone()).await.unwrap();
    let request = RetainedRuntimeTransitionRequest {
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        expected_revision: sandbox.revision,
        grant_id: uuid::Uuid::new_v4().simple().to_string(),
        policy_revision: 1,
        predecessor: RetainedStartupPolicyPredecessor {
            uid: attachment
                .receipt
                .native_authority
                .as_ref()
                .unwrap()
                .sandbox_uid
                .clone(),
            resource_version: "17".into(),
            request_sha256: "b".repeat(64),
            home_identity: attachment.identity().unwrap(),
            image: image.clone(),
            generation: 1,
            journal_recovery: false,
        },
        target_image: image.clone(),
        target_config_digest: format!("sha256:{}", "c".repeat(64)),
        journal_recovery: true,
    };
    let grant = RetainedRuntimeGrant {
        request,
        release_key: "retained-test".into(),
        catalog_sha256: "c".repeat(64),
        owner_scope_sha256: "d".repeat(64),
        issued_by: layer2_core::SubjectId("operator".into()),
        consent_sha256: "e".repeat(64),
        custody_sha256: "f".repeat(64),
        expires_at: DateTime::from_millis(DateTime::now().timestamp_millis() + 120_000),
        receipt: None,
        aborted_at: None,
    };
    (sandbox, grant)
}

async fn rows(store: &MongoSandboxStore, sandbox: &Sandbox) -> (bson::Document, bson::Document) {
    let aggregate = store.sandboxes.find_one(doc! {"_id": sandbox.id.as_str()});
    let grants = store
        .sandbox_admissions
        .find_one(doc! {"_id": sandbox.owner.as_str()});
    let (aggregate, grants) = tokio::join!(async { aggregate.await }, async { grants.await });
    (aggregate.unwrap().unwrap(), grants.unwrap().unwrap())
}

async fn held_rows(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
    hold: Bson,
) -> (bson::Document, bson::Document) {
    store
        .sandboxes
        .update_one(
            doc! {"_id": sandbox.id.as_str()},
            doc! {"$set": {"ledger_maintenance": hold}},
        )
        .await
        .unwrap();
    rows(store, sandbox).await
}

async fn clear_hold(store: &MongoSandboxStore, sandbox: &Sandbox) {
    store
        .sandboxes
        .update_one(
            doc! {"_id": sandbox.id.as_str()},
            doc! {"$unset": {"ledger_maintenance": 1}},
        )
        .await
        .unwrap();
}
