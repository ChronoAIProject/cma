//! Mutable Always-on policy must not invalidate durable admission identity.

use super::*;
use futures_util::FutureExt;
use layer2_core::sandbox::store::{PageRequest, SandboxMaintenanceStore};
use mongodb::bson::{serialize_to_bson, Document};
use std::{future::IntoFuture, panic::AssertUnwindSafe, time::Duration};

fn trigger() -> SandboxOrigin {
    SandboxOrigin::Trigger {
        launch_id: "always-on-trigger".into(),
    }
}

fn lifecycle() -> SandboxOrigin {
    SandboxOrigin::Lifecycle {
        operation_id: "always-on-lifecycle".into(),
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn trigger_initial_none_survives_always_on_changes() {
    policy_case(trigger(), None).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn trigger_initial_false_survives_always_on_changes() {
    policy_case(trigger(), Some(false)).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn trigger_initial_true_survives_always_on_changes() {
    policy_case(trigger(), Some(true)).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn lifecycle_initial_none_survives_always_on_changes() {
    policy_case(lifecycle(), None).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn lifecycle_initial_false_survives_always_on_changes() {
    policy_case(lifecycle(), Some(false)).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn lifecycle_initial_true_survives_always_on_changes() {
    policy_case(lifecycle(), Some(true)).await;
}

async fn policy_case(origin: SandboxOrigin, initial_policy: Option<bool>) {
    let (client, name) = database("always_on").await;
    let database = client.database(&name);
    assert!(
        database.list_collection_names().await.unwrap().is_empty(),
        "refuse preexisting data before taking cleanup ownership"
    );
    let result = tokio::time::timeout(
        Duration::from_secs(90),
        AssertUnwindSafe(exercise(&client, &name, origin, initial_policy)).catch_unwind(),
    )
    .await;
    // Preserve cleanup on the expected RED assertion as well as on GREEN.
    let cleanup =
        tokio::time::timeout(Duration::from_secs(10), database.drop().into_future()).await;
    assert!(
        matches!(cleanup, Ok(Ok(()))),
        "owned database cleanup failed"
    );
    eprintln!("always_on_database_cleanup=PASS");
    match result {
        Ok(Ok(())) => {}
        Ok(Err(panic)) => std::panic::resume_unwind(panic),
        Err(_) => panic!("Always-on Mongo regression timed out"),
    }
}

async fn exercise(
    client: &Client,
    database: &str,
    origin: SandboxOrigin,
    initial_policy: Option<bool>,
) {
    // Separate clients/stores exercise durable visibility across AM instances.
    let peer_client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let left = MongoSandboxStore::open(client, database).await.unwrap();
    let right = MongoSandboxStore::open(&peer_client, database)
        .await
        .unwrap();
    let mut original = sandbox("sbx_always_on", "unused");
    original.origin = origin;
    original.profile.always_on = initial_policy;
    let first = request(original.clone(), 1);
    let mut current = match left.admit_sandbox(first.clone()).await.unwrap() {
        SandboxAdmissionResult::Created(created) => created,
        SandboxAdmissionResult::Existing(_) => panic!("fresh admission replayed"),
    };
    let identity = stored_document(&left, &current.id).await;
    assert_visible(&right, &current, &first).await;

    // Change the value on the first write even when the original was Some(true).
    let opposite = !initial_policy.unwrap_or(false);
    for (index, policy) in [opposite, !opposite, opposite].into_iter().enumerate() {
        let (writer, reader) = if index % 2 == 0 {
            (&left, &right)
        } else {
            (&right, &left)
        };
        let previous_revision = current.revision;
        let changed = writer
            .set_sandbox_always_on(current.id.clone(), previous_revision, policy)
            .await
            .expect("policy write must succeed before the independent read");
        assert_eq!(changed.profile.always_on, Some(policy));
        assert_eq!(changed.revision, previous_revision + 1);
        assert_eq!(changed.origin, original.origin);
        assert_eq!(changed.profile.content, original.profile.content);
        assert_eq!(changed.profile.digest, original.profile.digest);

        // On the unchanged baseline the write is Ok, but this peer read is Corrupt.
        assert_visible(reader, &changed, &first).await;
        assert_identity_preserved(&identity, &stored_document(reader, &changed.id).await);
        assert_eq!(
            reader
                .set_sandbox_always_on(changed.id.clone(), previous_revision, !policy)
                .await
                .unwrap_err(),
            SandboxStoreError::Conflict,
            "a policy change cannot bypass revision fencing"
        );
        assert_visible(writer, &changed, &first).await;
        reject_changed_admission(reader, &first).await;
        current = changed;
    }

    assert_eq!(left.sandboxes.count_documents(doc! {}).await.unwrap(), 1);
    assert_eq!(
        left.sandbox_events(&current.id, 0, 10).await.unwrap().len(),
        1
    );
    reject_corrupt_immutable_profile(&left, &right, &current, &first).await;
}

fn request(sandbox: Sandbox, live_limit: u16) -> SandboxAdmission {
    SandboxAdmission::new(
        sandbox,
        SandboxAdmissionName::Allocate("trigger-research".into()),
        live_limit,
        NOW.into(),
    )
    .unwrap()
}

async fn assert_visible(store: &MongoSandboxStore, current: &Sandbox, original: &SandboxAdmission) {
    assert_eq!(
        store
            .get_sandbox(&current.id)
            .await
            .expect("independent store must read the committed policy without corruption"),
        Some(current.clone())
    );
    let inventory = store
        .list_sandboxes(&current.owner, &PageRequest::default())
        .await
        .unwrap();
    assert_eq!(inventory.items, vec![current.clone()]);
    assert!(inventory.next.is_none());
    let replay = store
        .sandbox_admission_replay(&current.owner, &current.origin)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replay.sandbox, *current);
    assert_eq!(replay.name, original.name);
    assert_eq!(replay.admission_fingerprint, original.admission_fingerprint);

    let mut retry = original.sandbox.clone();
    retry.id = SandboxId("sbx_always_on_retry".into());
    assert_eq!(
        store.admit_sandbox(request(retry, 0)).await.unwrap(),
        SandboxAdmissionResult::Existing(current.clone()),
        "original admission must replay before zero quota and retain current policy"
    );
}

async fn stored_document(store: &MongoSandboxStore, id: &SandboxId) -> Document {
    store
        .sandboxes
        .find_one(doc! { "_id": id.as_str() })
        .await
        .unwrap()
        .unwrap()
}

fn assert_identity_preserved(original: &Document, current: &Document) {
    for field in [
        "_id",
        "document_schema_version",
        "owner",
        "name",
        "origin_type",
        "origin_launch_id",
        "origin_operation_id",
        "admission_name",
        "admission_fingerprint",
    ] {
        assert!(original.contains_key(field), "fixture must contain {field}");
        assert_eq!(original.get(field), current.get(field), "changed {field}");
    }
}

async fn reject_changed_admission(store: &MongoSandboxStore, original: &SandboxAdmission) {
    for policy in [None, Some(false), Some(true)] {
        if policy == original.sandbox.profile.always_on {
            continue;
        }
        let mut changed = original.sandbox.clone();
        changed.id = SandboxId("sbx_changed_policy".into());
        changed.profile.always_on = policy;
        let changed = request(changed, 0);
        assert_ne!(
            changed.admission_fingerprint,
            original.admission_fingerprint
        );
        assert_eq!(
            store.admit_sandbox(changed).await.unwrap_err(),
            SandboxStoreError::SandboxOriginConflict,
            "changed original policy must conflict, even when it equals current policy"
        );
    }
    let mut changed = original.sandbox.clone();
    changed.id = SandboxId("sbx_changed_reach".into());
    changed
        .credential
        .reaches
        .push(ServiceId::new("expanded-security-reach"));
    assert_eq!(
        store.admit_sandbox(request(changed, 0)).await.unwrap_err(),
        SandboxStoreError::SandboxOriginConflict
    );
}

async fn reject_corrupt_immutable_profile(
    writer: &MongoSandboxStore,
    reader: &MongoSandboxStore,
    current: &Sandbox,
    original: &SandboxAdmission,
) {
    let before = stored_document(writer, &current.id).await;
    let mut profile = current.profile.clone();
    profile.content.cpu_millis += 1;
    // Keep the content digest valid: rejection must protect admission identity.
    profile.digest = content_digest(&profile.content).unwrap();
    let mut valid_aggregate = current.clone();
    valid_aggregate.profile = profile.clone();
    valid_aggregate.validate().unwrap();
    let update = writer
        .sandboxes
        .update_one(
            doc! { "_id": current.id.as_str() },
            doc! { "$set": { "record.profile": serialize_to_bson(&profile).unwrap() } },
        )
        .await
        .unwrap();
    assert_eq!(update.matched_count, 1);
    assert_eq!(update.modified_count, 1);
    assert_identity_preserved(&before, &stored_document(reader, &current.id).await);
    assert!(matches!(
        reader.get_sandbox(&current.id).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    assert!(matches!(
        reader
            .sandbox_admission_replay(&current.owner, &current.origin)
            .await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    assert!(matches!(
        reader.admit_sandbox(original.clone()).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
}
