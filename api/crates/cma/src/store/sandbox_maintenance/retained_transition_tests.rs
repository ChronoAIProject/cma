use super::*;
use crate::store::runtime_canary::CanaryPolicy;
use crate::store::sandbox_maintenance::mongo_integration_tests::{active_sandbox, database};
use crate::store::sandbox_unpublished_recovery::mongo_integration_tests::home;
use layer2_core::sandbox::{SandboxWorkState, SandboxWorkStep};
use layer3_adapter::RetainedStartupPolicyPredecessor;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retained_runtime_grant_same_image_reserves_once_and_replays_after_policy_close() {
    let (client, name, store) = database().await;
    let db = client.database(&name);
    let mut sandbox = active_sandbox("retained-transition");
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
        request: request.clone(),
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
    let fresh = || RetainedRuntimeGrantPolicy {
        release_key: &grant.release_key,
        catalog_sha256: &grant.catalog_sha256,
        owner_scope_sha256: &grant.owner_scope_sha256,
        target_image: &image,
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
        .redeem_retained_runtime_grant(&db, foreign, Some(fresh()))
        .await
        .is_err());
    let before = store.get_sandbox(&sandbox.id).await.unwrap().unwrap();
    assert_eq!(before, sandbox);

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
    db.drop().await.unwrap();
}
