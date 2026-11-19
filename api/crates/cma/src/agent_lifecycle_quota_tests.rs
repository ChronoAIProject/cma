use super::*;
use layer2_core::sandbox::store::{InMemorySandboxStore, SandboxStore};
use layer2_core::sandbox::FixedSandboxCredentialPrerequisite;
use mongodb::bson::{doc, Document};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn quota_preflight_preserves_replay_and_existing_targets_without_preparation() {
    let client = mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let db = client.database(&format!("cma_quota_{}", uuid::Uuid::new_v4().simple()));
    let operations = Arc::new(MongoAgentLifecycleStore::open(&db).await.unwrap());
    let profiles = Arc::new(
        crate::store::MongoProfileStore::open(&client, db.name())
            .await
            .unwrap(),
    );
    let store = Arc::new(InMemorySandboxStore::default());
    let service = fixture::facade_with_limit(
        store.clone(),
        profiles.clone(),
        operations.clone(),
        1,
        FixedSandboxCredentialPrerequisite::Ready,
    );
    let _slots = service
        .worker_slots
        .clone()
        .acquire_many_owned(crate::store::DISCOVERY_BATCH as u32)
        .await
        .unwrap();
    let mut owner = fixture::context("owner");
    owner.caller.authorization =
        Authorization::new([Capability::UseOwnSandboxes, Capability::CreateSandbox]);
    let original = request(json!({"name":"Original"}));
    let receipt = service
        .create(
            &owner.caller,
            Some(&owner),
            None,
            original.clone(),
            "original",
        )
        .await
        .unwrap();
    let workspace = fixture::seed(&store).await;
    let refused = request(json!({"name":"No slot", "image":{"id":"must-not-resolve"}}));
    assert!(matches!(
        service
            .create(&owner.caller, Some(&owner), None, refused, "refused")
            .await,
        Err(AgentLifecycleProvisioningError::SandboxLimitReached { limit: 1, live: 1 })
    ));
    assert_eq!(
        db.collection::<Document>("agent_lifecycle_operations")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    for collection in ["agent_profiles", "workspace_profiles", "sandbox_profiles"] {
        assert_eq!(
            db.collection::<Document>(collection)
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
    }
    let reopened = fixture::facade_with_limit(
        store.clone(),
        profiles.clone(),
        operations.clone(),
        1,
        FixedSandboxCredentialPrerequisite::Ready,
    );
    let _reopened_slots = reopened
        .worker_slots
        .clone()
        .acquire_many_owned(crate::store::DISCOVERY_BATCH as u32)
        .await
        .unwrap();
    assert_eq!(
        reopened
            .create(&owner.caller, Some(&owner), None, original, "original")
            .await
            .unwrap()
            .id,
        receipt.id
    );
    for target in [
        json!({"name":"Existing workspace","workspace_id":workspace.id.as_str()}),
        json!({"name":"Existing Sandbox","sandbox_id":workspace.sandbox_id.as_str()}),
    ] {
        service
            .create(
                &owner.caller,
                Some(&owner),
                None,
                request(target.clone()),
                target["name"].as_str().unwrap(),
            )
            .await
            .unwrap();
    }
    assert_eq!(
        store
            .owner_capacity(&owner.caller.subject)
            .await
            .unwrap()
            .counts
            .iter()
            .map(|(_, count)| count)
            .sum::<u64>(),
        1
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn worker_quota_failure_metadata_is_gated_and_survives_restart() {
    for enabled in [false, true] {
        let client =
            mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
                .await
                .unwrap();
        let db = client.database(&format!(
            "cma_quota_failure_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let operations = Arc::new(MongoAgentLifecycleStore::open(&db).await.unwrap());
        let profiles = Arc::new(
            crate::store::MongoProfileStore::open(&client, db.name())
                .await
                .unwrap(),
        );
        let store = Arc::new(InMemorySandboxStore::default());
        let service = fixture::facade_with_limit(
            store.clone(),
            profiles,
            operations.clone(),
            1,
            FixedSandboxCredentialPrerequisite::Ready,
        )
        .with_failure_metadata(enabled);
        let slots = service
            .worker_slots
            .clone()
            .acquire_many_owned(crate::store::DISCOVERY_BATCH as u32)
            .await
            .unwrap();
        let mut owner = fixture::context("owner");
        owner.caller.authorization =
            Authorization::new([Capability::UseOwnSandboxes, Capability::CreateSandbox]);
        let input = request(json!({"name":"Loses last slot"}));
        let receipt = service
            .create(&owner.caller, Some(&owner), None, input.clone(), "race")
            .await
            .unwrap();
        fixture::seed(&store).await;
        drop(slots);
        service.spawn_worker(receipt.id.clone());
        let record = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let record = operations
                    .read_for_owner(&owner.caller.subject, &receipt.id)
                    .await
                    .unwrap()
                    .unwrap();
                if record.status == "failed" {
                    break record;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(record.sandbox_id.is_none());
        assert!(record
            .error
            .as_ref()
            .unwrap()
            .contains("sandbox limit reached"));
        let expected = enabled.then_some(
            layer1_public::rest::AgentLifecycleFailure::SandboxLimitReached { limit: 1, live: 1 },
        );
        assert_eq!(record.failure, expected);
        let raw = db
            .collection::<Document>("agent_lifecycle_operations")
            .find_one(doc! {"_id":&receipt.id})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(raw.contains_key("failure"), enabled);
        let restarted = MongoAgentLifecycleStore::open(&db).await.unwrap();
        let replay = restarted
            .replay(&owner.caller.subject, "race", &input)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(replay.view().failure, expected);
        assert_eq!(replay.id, receipt.id);
        assert_eq!(replay.status, "failed");
        db.drop().await.unwrap();
    }
}
