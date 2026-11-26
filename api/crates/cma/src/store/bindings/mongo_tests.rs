use super::*;
use layer2_core::profiles::{content_digest, AgentProvider, ProfileCapacityBounds};
use layer2_core::sandbox::binding::{
    map_profile, BindingConversationState, BindingState, BindingWork,
};
use layer2_core::SubjectId;
use mongodb::Client;
use serde_json::json;

pub(super) const NOW: &str = "2026-09-22T00:00:00Z";

pub(super) async fn fixture() -> (Client, String, MongoSandboxStore) {
    let uri =
        std::env::var("CHRONO_TEST_MONGODB_URI").expect("local Mongo replica set is required");
    let client = Client::with_uri_str(uri).await.unwrap();
    let database = format!("cma_binding_test_{}", uuid::Uuid::new_v4().simple());
    crate::store::migrations::prepare_bindings_for_test(&client.database(&database)).await;
    let store = MongoSandboxStore::open(&client, &database).await.unwrap();
    (client, database, store)
}

pub(super) fn binding() -> ProviderBinding {
    let value = json!({
        "schema_version":1,
        "metadata":{"name":"stored-binding", "owner":{"kind":"human", "subject":"owner"}, "provider":"registered"},
        "model":{"access":"managed", "service":AgentProvider::default().connection_services().managed,
            "api_type":"supported", "model":"test", "thinking_effort":"medium"},
        "permission":{"action_mode":"guarded"}, "context":{},
        "environment":{"size":{"cpu_cores":1, "memory_gib":2}, "workspace":{"type":"directory", "name":"project"}}
    });
    let digest = format!("sha256:{}", content_digest(&value).unwrap());
    let plan = map_profile(
        value,
        &digest,
        AgentProvider::default(),
        "supported",
        ProfileCapacityBounds {
            min_cpu_millis: 250,
            max_cpu_millis: 8000,
            min_memory_mib: 512,
            max_memory_mib: 16384,
        },
    )
    .unwrap();
    ProviderBinding::new(
        "binding-one".into(),
        SubjectId::new("manager"),
        plan,
        "a".repeat(64),
        NOW.into(),
    )
    .unwrap()
}

pub(in crate::store) async fn insert_retained_binding_fixture(
    database: &Database,
) -> (ProviderBinding, BindingConversation) {
    let mut value = binding();
    value.state = BindingState::Ready;
    value.work = BindingWork::Idle;
    value.advance_revision(NOW.into()).unwrap();
    let conversation =
        BindingConversation::new(&value, "conversation_retained".into(), NOW.into()).unwrap();
    database
        .collection::<Document>(BINDINGS)
        .insert_one(binding_document(&value).unwrap())
        .await
        .unwrap();
    database
        .collection::<Document>(CONVERSATIONS)
        .insert_one(conversation_document(&conversation).unwrap())
        .await
        .unwrap();
    (value, conversation)
}

async fn ready(store: &MongoSandboxStore) -> ProviderBinding {
    let mut value = binding();
    store.save_binding(None, value.clone()).await.unwrap();
    value.state = BindingState::Ready;
    value.work = BindingWork::Idle;
    value.advance_revision(NOW.into()).unwrap();
    store.save_binding(Some(1), value.clone()).await.unwrap();
    value
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_binding_queue_policy_survives_reopen_and_fences_stale_updates() {
    let (client, database, store) = fixture().await;
    let original = ready(&store).await;
    assert_eq!(original.profile.effective_queue_lifetime_seconds(), 300);
    let mut profile = original.profile.clone();
    profile.queue_lifetime_seconds = Some(86400);
    profile.configuration_digest = Some(original.profile_digest.clone());
    profile.profile_digest = format!("sha256:{}", "b".repeat(64));
    let updated = original
        .reconfigure(profile, original.key_sha256.clone(), NOW.into())
        .unwrap();
    store
        .save_binding(Some(original.revision), updated.clone())
        .await
        .unwrap();
    let reopened = MongoSandboxStore::open(&client, &database).await.unwrap();
    let retained = reopened
        .binding(&original.binding_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained, updated);
    assert_eq!(retained.profile.effective_queue_lifetime_seconds(), 86400);
    let mut stale_profile = original.profile.clone();
    stale_profile.queue_lifetime_seconds = Some(3600);
    let stale = original
        .reconfigure(stale_profile, original.key_sha256.clone(), NOW.into())
        .unwrap();
    assert_eq!(
        reopened.save_binding(Some(original.revision), stale).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store.binding(&original.binding_id).await.unwrap(),
        Some(updated)
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_bindings_cas_reopen_reconfigure_and_conversation_races() {
    let (client, database, store) = fixture().await;
    let value = ready(&store).await;
    assert_eq!(
        store.save_binding(None, binding()).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store.save_binding(Some(1), value.clone()).await,
        Err(SandboxStoreError::Conflict)
    );
    let first = BindingConversation::new(&value, "conversation_one".into(), NOW.into()).unwrap();
    let second = BindingConversation::new(&value, "conversation_one".into(), NOW.into()).unwrap();
    let (a, b) = tokio::join!(
        store.save_binding_conversation(value.revision, None, first.clone()),
        store.save_binding_conversation(value.revision, None, second.clone())
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let winner = if a.is_ok() { &first } else { &second };
    assert_eq!(
        if a.is_ok() { b } else { a },
        Err(SandboxStoreError::Conflict)
    );
    let reopened = MongoSandboxStore::open(&client, &database).await.unwrap();
    assert_eq!(
        reopened.binding(&value.binding_id).await.unwrap(),
        Some(value.clone())
    );
    assert_eq!(
        reopened
            .binding_conversation(&value.binding_id, value.generation, "conversation_one")
            .await
            .unwrap()
            .as_ref(),
        Some(winner)
    );
    assert_eq!(
        store
            .pending_binding_conversations(None, 1)
            .await
            .unwrap()
            .as_slice(),
        std::slice::from_ref(winner)
    );
    let cursor = conversation_storage_key(winner);
    assert!(store
        .pending_binding_conversations(Some(&cursor), 1)
        .await
        .unwrap()
        .is_empty());

    let mut plan = value.profile.clone();
    plan.profile_digest = format!("sha256:{}", "b".repeat(64));
    plan.name = "Renamed binding".into();
    let changed = value
        .reconfigure(plan, value.key_sha256.clone(), NOW.into())
        .unwrap();
    store
        .save_binding(Some(value.revision), changed.clone())
        .await
        .unwrap();
    assert_eq!(changed.sandbox_id, value.sandbox_id);
    assert_eq!(changed.workspace_id, value.workspace_id);
    let stale = BindingConversation::new(&value, "conversation_two".into(), NOW.into()).unwrap();
    assert_eq!(
        store
            .save_binding_conversation(value.revision, None, stale)
            .await,
        Err(SandboxStoreError::Conflict)
    );
    let mut cleanup = winner.clone();
    cleanup.state = BindingConversationState::Retiring;
    cleanup.revision += 1;
    store
        .save_binding_conversation(changed.revision, Some(winner.revision), cleanup.clone())
        .await
        .unwrap();
    let previous_revision = cleanup.revision;
    cleanup.state = BindingConversationState::Retired;
    cleanup.revision += 1;
    store
        .save_binding_conversation(changed.revision, Some(previous_revision), cleanup.clone())
        .await
        .unwrap();
    assert!(store
        .pending_binding_conversations(None, 10)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        store
            .binding_conversations(&value.binding_id, value.generation, None, 10)
            .await
            .unwrap(),
        [cleanup]
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_binding_retirement_and_rejected_lineage_leave_no_partial_reservation() {
    let (client, database, store) = fixture().await;
    let value = ready(&store).await;
    let before = store
        .bindings
        .records
        .find_one(doc! {"_id": &value.binding_id})
        .await
        .unwrap()
        .unwrap();
    let mut wrong =
        BindingConversation::new(&value, "conversation_one".into(), NOW.into()).unwrap();
    wrong.owner = SubjectId::new("another-owner");
    assert_eq!(
        store
            .save_binding_conversation(value.revision, None, wrong)
            .await,
        Err(SandboxStoreError::Conflict)
    );
    let after = store
        .bindings
        .records
        .find_one(doc! {"_id": &value.binding_id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        before, after,
        "aborted admission must also undo its parent fence"
    );
    assert_eq!(
        store
            .bindings
            .conversations
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    let mut retired = value.clone();
    retired.state = BindingState::Retired;
    retired.work = BindingWork::RetireCurrentConversations;
    retired.advance_revision(NOW.into()).unwrap();
    store
        .save_binding(Some(value.revision), retired.clone())
        .await
        .unwrap();
    let late = BindingConversation::new(&value, "conversation_one".into(), NOW.into()).unwrap();
    assert_eq!(
        store
            .save_binding_conversation(retired.revision, None, late)
            .await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(store.pending_bindings(None, 1).await.unwrap(), [retired]);
    assert!(store
        .pending_bindings(Some(&value.binding_id), 1)
        .await
        .unwrap()
        .is_empty());
    store
        .bindings
        .records
        .update_one(
            doc! {"_id": &value.binding_id},
            doc! {"$set":{"record.owner":"wrong-owner"}},
        )
        .await
        .unwrap();
    assert!(matches!(
        store.binding(&value.binding_id).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_worker_classification_sees_other_clients_committed_key_history() {
    let (writer, database, store) = fixture().await;
    let reader = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let lookup = MongoSandboxStore::open(&reader, &database).await.unwrap();
    assert!(!lookup
        .classify_worker_key_hash(&"a".repeat(64))
        .await
        .unwrap());
    let original = ready(&store).await;
    assert!(lookup
        .classify_worker_key_hash(&original.key_sha256)
        .await
        .unwrap());
    let rotated = original
        .rotate_credential("b".repeat(64), NOW.into())
        .unwrap();
    store
        .save_binding(Some(original.revision), rotated.clone())
        .await
        .unwrap();
    let mut retired = rotated.clone();
    retired.state = BindingState::Retired;
    retired.work = BindingWork::RetireCurrentConversations;
    retired.advance_revision(NOW.into()).unwrap();
    store
        .save_binding(Some(rotated.revision), retired)
        .await
        .unwrap();
    assert!(lookup
        .classify_worker_key_hash(&original.key_sha256)
        .await
        .unwrap());
    assert!(lookup
        .classify_worker_key_hash(&rotated.key_sha256)
        .await
        .unwrap());
    assert!(!lookup
        .classify_worker_key_hash(&"c".repeat(64))
        .await
        .unwrap());
    writer.database(&database).drop().await.unwrap();
}

#[path = "admission_contention_tests.rs"]
mod admission_contention;
