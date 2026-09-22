use super::mongo_tests::{binding, fixture, NOW};
use super::*;
use layer2_core::sandbox::binding::{BindingState, BindingWork};
use layer2_core::sandbox::InMemorySandboxStore;

async fn contract<S: BindingStore>(store: &S) {
    let first = binding();
    let second = ProviderBinding::new(
        "binding-second".into(),
        first.manager.clone(),
        first.profile.clone(),
        first.key_sha256.clone(),
        NOW.into(),
    )
    .unwrap();
    let (a, b) = tokio::join!(
        store.save_binding(None, first.clone()),
        store.save_binding(None, second.clone())
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let (winner, mut other) = if a.is_ok() {
        assert_eq!(b, Err(SandboxStoreError::Conflict));
        (first, second)
    } else {
        assert_eq!(a, Err(SandboxStoreError::Conflict));
        (second, first)
    };
    assert_eq!(
        store.find_by_key_hash(&winner.key_sha256).await.unwrap(),
        Some(winner.clone())
    );
    other.key_sha256 = "b".repeat(64);
    store.save_binding(None, other.clone()).await.unwrap();
    assert_eq!(
        store.find_by_key_hash(&other.key_sha256).await.unwrap(),
        Some(other.clone())
    );
    let mut ready = winner.clone();
    ready.state = BindingState::Ready;
    ready.work = BindingWork::Idle;
    ready.advance_revision(NOW.into()).unwrap();
    store
        .save_binding(Some(winner.revision), ready.clone())
        .await
        .unwrap();
    let conversation =
        BindingConversation::new(&ready, "conversation_index".into(), NOW.into()).unwrap();
    store
        .save_binding_conversation(ready.revision, None, conversation.clone())
        .await
        .unwrap();
    assert_eq!(
        store
            .binding_conversation_for_agent(
                &ready.binding_id,
                ready.generation,
                &conversation.agent_id
            )
            .await
            .unwrap(),
        Some(conversation.clone())
    );
    assert!(store
        .binding_conversation_for_agent(&other.binding_id, other.generation, &conversation.agent_id)
        .await
        .unwrap()
        .is_none());
    assert!(store
        .binding_conversation_for_agent(
            &ready.binding_id,
            ready.generation + 1,
            &conversation.agent_id
        )
        .await
        .unwrap()
        .is_none());
    let mut duplicate = conversation.clone();
    duplicate.conversation_id = "conversation_other".into();
    assert_eq!(
        store
            .save_binding_conversation(ready.revision, None, duplicate)
            .await,
        Err(SandboxStoreError::Conflict)
    );
    let mut retired = ready.clone();
    retired.state = BindingState::Retired;
    retired.work = BindingWork::RetireCurrentConversations;
    retired.advance_revision(NOW.into()).unwrap();
    store
        .save_binding(Some(ready.revision), retired.clone())
        .await
        .unwrap();
    assert!(store
        .find_by_key_hash(&retired.key_sha256)
        .await
        .unwrap()
        .is_none());
    // A retired key cannot silently become authority for another binding.
    let duplicate = ProviderBinding::new(
        "binding-third".into(),
        other.manager,
        other.profile,
        retired.key_sha256,
        NOW.into(),
    )
    .unwrap();
    assert_eq!(
        store.save_binding(None, duplicate).await,
        Err(SandboxStoreError::Conflict)
    );
}

#[tokio::test]
async fn memory_binding_hash_uniqueness_and_reverse_conversation_index() {
    contract(&InMemorySandboxStore::default()).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_binding_hash_uniqueness_and_reverse_conversation_index() {
    let (client, database, store) = fixture().await;
    contract(&store).await;
    client.database(&database).drop().await.unwrap();
}
