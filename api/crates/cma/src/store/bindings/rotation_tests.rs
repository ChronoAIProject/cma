use super::mongo_tests::{binding, fixture, NOW};
use super::*;
use crate::sandbox::{ManagedSandboxCredentials, SandboxCredentialCustody};
use crate::store::{AuthEncryptionKey, MongoSandboxCredentialStore};
use base64::Engine;
use layer2_core::sandbox::binding::{
    BindingCredentialCustody, BindingState, BindingWork, ProvidedCredential,
};
use layer2_core::sandbox::store::SandboxStore;
use layer2_core::sandbox::{SandboxStatus, SandboxWorkStep};
use ring::rand::{SecureRandom, SystemRandom};
use std::sync::Arc;

fn key() -> ProvidedCredential {
    ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_binding_rotation_atomically_fences_key_sandbox_and_conversations() {
    let (client, database, store) = fixture().await;
    let mut bytes = [0; 32];
    SystemRandom::new().fill(&mut bytes).unwrap();
    let encryption =
        AuthEncryptionKey::from_base64(&base64::engine::general_purpose::STANDARD.encode(bytes))
            .unwrap();
    let custody = Arc::new(
        MongoSandboxCredentialStore::open(&client, &database, "binding-rotation-test", &encryption)
            .await
            .unwrap(),
    );
    let store = Arc::new(store);
    let controller = MongoBindingCredentials::new(store.clone(), custody.clone());
    let original = key();
    let replacement = key();
    let mut binding = binding();
    binding.key_sha256 = original.sha256();
    store.save_binding(None, binding.clone()).await.unwrap();
    controller
        .retain_binding(&binding, &original)
        .await
        .unwrap();
    binding.state = BindingState::Ready;
    binding.work = BindingWork::Idle;
    binding.advance_revision(NOW.into()).unwrap();
    store.save_binding(Some(1), binding.clone()).await.unwrap();
    let mut sandbox = super::admission_tests::sandbox(&binding);
    sandbox.status = SandboxStatus::Active;
    sandbox.work.schedule(SandboxWorkStep::None, NOW.into());
    sandbox.credential.current = Some(
        ManagedSandboxCredentials::binding_material(&binding, &original)
            .unwrap()
            .authority,
    );
    sandbox.credential.key_generation = 1;
    sandbox.credential.delivered = true;
    store.create_sandbox(sandbox.clone()).await.unwrap();
    let conversation =
        BindingConversation::new(&binding, "conversation_rotation".into(), NOW.into()).unwrap();
    store
        .save_binding_conversation(binding.revision, None, conversation.clone())
        .await
        .unwrap();

    // A busy runtime refuses admission without retaining an orphan replacement.
    let mut busy = sandbox.clone();
    busy.work.schedule(SandboxWorkStep::Activate, NOW.into());
    busy.revision += 1;
    store
        .save_sandbox(sandbox.revision, busy.clone())
        .await
        .unwrap();
    assert!(controller
        .rotate_binding(&binding, &replacement)
        .await
        .is_err());
    assert!(custody.get(&binding.sandbox_id, 2).await.unwrap().is_none());
    assert_eq!(
        store.binding(&binding.binding_id).await.unwrap().unwrap(),
        binding
    );
    sandbox.revision = busy.revision + 1;
    store
        .save_sandbox(busy.revision, sandbox.clone())
        .await
        .unwrap();

    let rotated = controller
        .rotate_binding(&binding, &replacement)
        .await
        .unwrap();
    assert_eq!(rotated.generation, binding.generation);
    assert_eq!(rotated.sandbox_id, binding.sandbox_id);
    assert_eq!(rotated.workspace_id, binding.workspace_id);
    assert_eq!(rotated.work, BindingWork::DeliverCredential);
    // A former binding key must never regain ordinary user authority after rotation.
    assert!(store.contains_key_hash(&original.sha256()).await.unwrap());
    assert!(store
        .contains_key_hash(&replacement.sha256())
        .await
        .unwrap());
    let stored = store
        .get_sandbox(&binding.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.status, SandboxStatus::CredentialRotating);
    assert_eq!(stored.credential.current, sandbox.credential.current);
    assert_eq!(
        stored.credential.pending.as_ref().unwrap().provider_key_id,
        format!("provided-{}", replacement.sha256())
    );
    assert_eq!(
        custody
            .get(&binding.sandbox_id, 2)
            .await
            .unwrap()
            .unwrap()
            .value
            .expose(),
        replacement.expose()
    );
    assert_eq!(
        store
            .binding_conversation(&binding.binding_id, 1, &conversation.conversation_id)
            .await
            .unwrap(),
        Some(conversation)
    );
    assert!(controller.rotate_binding(&binding, &key()).await.is_err());
    assert!(custody.get(&binding.sandbox_id, 3).await.unwrap().is_none());

    let mut retired = rotated.clone();
    retired.state = BindingState::Retired;
    retired.work = BindingWork::RetireCurrentConversations;
    retired
        .advance_revision(layer2_core::now_rfc3339())
        .unwrap();
    store
        .save_binding(Some(rotated.revision), retired.clone())
        .await
        .unwrap();
    assert!(controller
        .rotate_binding(&binding, &original)
        .await
        .is_err());
    assert_eq!(
        store.binding(&binding.binding_id).await.unwrap().unwrap(),
        retired
    );
    let reopened = MongoSandboxStore::open(&client, &database).await.unwrap();
    assert!(reopened
        .contains_key_hash(&original.sha256())
        .await
        .unwrap());
    assert!(reopened
        .contains_key_hash(&replacement.sha256())
        .await
        .unwrap());
    assert!(reopened
        .find_by_key_hash(&original.sha256())
        .await
        .unwrap()
        .is_none());
    assert!(reopened
        .find_by_key_hash(&replacement.sha256())
        .await
        .unwrap()
        .is_none());
    client.database(&database).drop().await.unwrap();
}
