use super::*;
use crate::store::sandbox_maintenance::{
    mongo_integration_tests::database, retained_transition::tests::retained_fixture,
};
use layer3_adapter::codex::{CodexAdapter, CodexSandboxSpec};
use mongodb::bson::doc;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retained_runtime_grant_owner_reconciles_when_the_other_redemption_wins_after_read() {
    let (client, name, store) = database().await;
    let database = client.database(&name);
    let (sandbox, mut grant) = retained_fixture(&store, "retained-owner-race").await;
    let config =
        crate::config::retained_runtime_config(&grant.request.target_image, sandbox.owner.as_str());
    let (adapter, _) = CodexAdapter::in_memory(CodexSandboxSpec::default());
    let service = RetainedRuntimeService::new(
        &config,
        "fixture".into(),
        Arc::new(store),
        database.clone(),
        Arc::new(adapter),
    )
    .unwrap();
    let release = service.release(&sandbox.owner, "approved-agent").unwrap();
    grant.release_key = release.release_key.clone();
    grant.request.target_config_digest = release.config_digest.clone();
    grant.catalog_sha256 = service.catalog_sha256.clone();
    grant.owner_scope_sha256 = service.scope_sha256.clone();
    CanaryPolicy::configure_retained(
        &database,
        0,
        true,
        grant.catalog_sha256.clone(),
        grant.owner_scope_sha256.clone(),
    )
    .await
    .unwrap();
    service
        .store
        .issue_retained_runtime_grant(&database, &grant)
        .await
        .unwrap();

    // This is the snapshot held by the owner route before a competing
    // redemption advances the Sandbox revision.
    let loaded = service
        .store
        .read_retained_runtime_grant(&sandbox.owner, &grant.request.grant_id)
        .await
        .unwrap();
    assert!(loaded.receipt.is_none());
    assert!(matches!(
        service
            .finish_redemption(loaded.request.clone(), Err(CanaryRefusal::Closed))
            .await,
        Err(CanaryRefusal::Closed),
    ));
    assert!(service
        .store
        .read_retained_runtime_grant(&sandbox.owner, &grant.request.grant_id)
        .await
        .unwrap()
        .receipt
        .is_none());
    let winner = service
        .store
        .redeem_retained_runtime_grant(
            &database,
            grant.request.clone(),
            Some(RetainedRuntimeGrantPolicy {
                release_key: &grant.release_key,
                catalog_sha256: &grant.catalog_sha256,
                owner_scope_sha256: &grant.owner_scope_sha256,
                target_image: &grant.request.target_image,
                target_config_digest: &grant.request.target_config_digest,
            }),
        )
        .await
        .unwrap();
    let preflight = service
        .fresh_redemption_policy(&sandbox.owner, &loaded)
        .await
        .map(Some);
    assert!(matches!(&preflight, Err(CanaryRefusal::Conflict)));
    let admin = client.database("admin");
    admin
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": {"times": 1},
            "data": {"failCommands": ["find"], "errorCode": 2_i32},
        })
        .await
        .unwrap();
    let failed_lookup = service
        .finish_redemption(loaded.request.clone(), preflight)
        .await;
    admin
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();
    assert!(matches!(failed_lookup, Err(CanaryRefusal::Unavailable)));
    let replay = service
        .redeem_loaded_grant(&sandbox.owner, loaded)
        .await
        .unwrap();
    assert_eq!(replay.operation_id, winner.operation_id);
    assert_eq!(replay.successor_generation, winner.successor_generation);
    let persisted = service
        .store
        .get_sandbox(&sandbox.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(persisted.revision, sandbox.revision + 1);
    assert_eq!(
        persisted.lifecycle.runtime_generation_high_water,
        winner.successor_generation
    );
    database.drop().await.unwrap();
}
