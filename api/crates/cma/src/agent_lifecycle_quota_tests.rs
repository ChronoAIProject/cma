use super::*;
use layer2_core::sandbox::store::{InMemorySandboxStore, SandboxStore};
use layer2_core::sandbox::{FixedSandboxCredentialPrerequisite, SandboxStatus};
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
        let _slots = service
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
        let collection = db.collection::<Document>("agent_lifecycle_operations");
        for (refusals, delay) in [(1, 5_000), (2, 10_000)] {
            service.run_worker(&receipt.id).await.unwrap();
            let retry = operations
                .read_for_owner(&owner.caller.subject, &receipt.id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(retry.status, "queued");
            assert_eq!(retry.sandbox_quota_refusals, Some(refusals));
            assert!(retry.lease_token.is_none());
            assert!(retry.lease_expires_at.is_none());
            assert!(retry.agent_id.is_none());
            assert!(retry.sandbox_id.is_none());
            assert!(retry.error.is_none());
            assert!(retry.failure.is_none());
            assert_eq!(
                retry.retry_at.unwrap().timestamp_millis() - retry.updated_at.timestamp_millis(),
                delay
            );
            assert!(operations.claim(&receipt.id).await.unwrap().is_none());
            make_retry_due(&collection, &receipt.id).await;
        }
        service.run_worker(&receipt.id).await.unwrap();
        let record = operations
            .read_for_owner(&owner.caller.subject, &receipt.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.status, "failed");
        assert_eq!(record.attempts, 3);
        assert_eq!(record.sandbox_quota_refusals, Some(2));
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

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn worker_quota_retry_resumes_original_agent_when_a_slot_returns() {
    let client = mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let db = client.database(&format!(
        "cma_quota_retry_{}",
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
        profiles.clone(),
        operations.clone(),
        1,
        FixedSandboxCredentialPrerequisite::Ready,
    )
    .with_task_experience(fixture::task_policy());
    let _slots = service
        .worker_slots
        .clone()
        .acquire_many_owned(crate::store::DISCOVERY_BATCH as u32)
        .await
        .unwrap();
    let mut owner = fixture::context("owner");
    owner.caller.authorization =
        Authorization::new([Capability::UseOwnSandboxes, Capability::CreateSandbox]);
    let input = request(json!({"name":"Wait for a slot", "initial_message":"Continue the task"}));
    let receipt = service
        .create(&owner.caller, Some(&owner), None, input.clone(), "retry")
        .await
        .unwrap();
    let collection = db.collection::<Document>("agent_lifecycle_operations");
    // Transient dependency retries take claims before any Sandbox quota check.
    for attempts in [1, 2] {
        let record = operations.claim(&receipt.id).await.unwrap().unwrap();
        assert_eq!(record.attempts, attempts);
        assert_eq!(record.sandbox_quota_refusals, None);
        operations
            .retry(&record.lease().unwrap(), attempts)
            .await
            .unwrap();
        make_retry_due(&collection, &receipt.id).await;
    }
    let occupied = fixture::seed(&store).await;
    service.run_worker(&receipt.id).await.unwrap();
    let retry = operations
        .read_for_owner(&owner.caller.subject, &receipt.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retry.attempts, 3);
    assert_eq!(retry.status, "queued");
    assert_eq!(retry.sandbox_quota_refusals, Some(1));
    assert_eq!(
        retry.retry_at.unwrap().timestamp_millis() - retry.updated_at.timestamp_millis(),
        5_000
    );
    let mut sandbox = store
        .get_sandbox(&occupied.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let revision = sandbox.revision;
    sandbox.status = SandboxStatus::Retired;
    sandbox.retired_at = Some("2026-10-13T00:00:00Z".into());
    sandbox.revision += 1;
    store.save_sandbox(revision, sandbox).await.unwrap();
    make_retry_due(&collection, &receipt.id).await;
    let restarted = Arc::new(MongoAgentLifecycleStore::open(&db).await.unwrap());
    assert_eq!(
        restarted.pending().await.unwrap().as_slice(),
        std::slice::from_ref(&receipt.id)
    );
    let next = fixture::facade_with_limit(
        store.clone(),
        profiles,
        restarted.clone(),
        1,
        FixedSandboxCredentialPrerequisite::Ready,
    )
    .with_task_experience(fixture::task_policy());
    let _next_slots = next
        .worker_slots
        .clone()
        .acquire_many_owned(crate::store::DISCOVERY_BATCH as u32)
        .await
        .unwrap();
    assert_eq!(
        next.create(&owner.caller, Some(&owner), None, input, "retry")
            .await
            .unwrap()
            .id,
        receipt.id
    );
    {
        // The fixture has no runtime worker. Stop once the real lifecycle
        // worker has durably bound its Sandbox and Starting Agent.
        let worker = next.run_worker(&receipt.id);
        let admission = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let record = restarted
                    .read_for_owner(&owner.caller.subject, &receipt.id)
                    .await
                    .unwrap()
                    .unwrap();
                if record.agent_id.is_some() {
                    break record;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        let record = tokio::select! {
            result = worker => panic!("worker exited before binding the Agent: {result:?}"),
            record = admission => record.unwrap(),
        };
        assert_eq!(record.status, "running");
        assert_eq!(record.attempts, 4);
        assert_eq!(record.sandbox_quota_refusals, Some(1));
        let sandbox = store
            .get_sandbox(&SandboxId(record.sandbox_id.unwrap()))
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(sandbox.origin, layer2_core::sandbox::SandboxOrigin::Lifecycle {
            operation_id
        } if operation_id == receipt.id)
        );
        assert_eq!(collection.count_documents(doc! {}).await.unwrap(), 1);
        assert_eq!(next.sandbox.capacity(&owner.caller).await.unwrap().live, 1);
    }
    db.drop().await.unwrap();
}

async fn make_retry_due(collection: &mongodb::Collection<Document>, id: &str) {
    let update = collection
        .update_one(
            doc! {"_id": id, "status": "queued"},
            doc! {"$set": {"retry_at": mongodb::bson::DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    assert_eq!(update.matched_count, 1);
}
