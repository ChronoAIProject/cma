use super::mongo_tests::{binding, fixture, NOW};
use super::*;
use crate::sandbox::SandboxCredentialCustody;
use crate::store::{AuthEncryptionKey, MongoSandboxCredentialStore};
use base64::Engine;
use layer2_core::sandbox::binding::{
    BindingConversationState, BindingCredentialCustody, BindingState, BindingWork,
    ProvidedCredential,
};
use layer2_core::sandbox::store::{SandboxStore, WorkspaceStore};
use layer2_core::sandbox::{
    RuntimeBinding, SandboxStatus, SandboxWorkStep, WorkspaceStatus, WorkspaceWorkStep,
};
use ring::rand::{SecureRandom, SystemRandom};
use std::sync::Arc;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_failed_binding_retry_preserves_generation_and_fences_original_custody_and_parents() {
    let (client, database, store) = fixture().await;
    let mut bytes = [0; 32];
    SystemRandom::new().fill(&mut bytes).unwrap();
    let encryption =
        AuthEncryptionKey::from_base64(&base64::engine::general_purpose::STANDARD.encode(bytes))
            .unwrap();
    let custody = Arc::new(
        MongoSandboxCredentialStore::open(&client, &database, "binding-retry-test", &encryption)
            .await
            .unwrap(),
    );
    let store = Arc::new(store);
    let controller = MongoBindingCredentials::new(store.clone(), custody.clone());
    let key = ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap();
    let mut binding = binding();
    binding.key_sha256 = key.sha256();
    store.save_binding(None, binding.clone()).await.unwrap();
    controller.retain_binding(&binding, &key).await.unwrap();
    let material = custody.get(&binding.sandbox_id, 1).await.unwrap().unwrap();
    // Load a historical generation exactly as an upgraded deployment does.
    binding.generation = 3;
    binding.state = BindingState::Failed {
        reason: "sandbox provisioning failed".into(),
    };
    binding.work = BindingWork::Idle;
    binding.advance_revision(NOW.into()).unwrap();
    store
        .bindings
        .records
        .replace_one(
            doc! {"_id": &binding.binding_id},
            binding_document(&binding).unwrap(),
        )
        .await
        .unwrap();
    let mut sandbox = super::admission_tests::sandbox(&binding);
    sandbox.status = SandboxStatus::Active;
    sandbox.work.schedule(SandboxWorkStep::None, NOW.into());
    sandbox.runtime = Some(RuntimeBinding {
        boot_id: None,
        runtime_id: "original-runtime".into(),
        runtime_version: "runtime-v1".into(),
        provision_generation: Some(3),
        bound_at: NOW.into(),
    });
    sandbox.lifecycle.runtime_generation_high_water = 3;
    sandbox.credential.current = Some(material.authority.clone());
    sandbox.credential.key_generation = 1;
    sandbox.credential.delivered = true;
    store.create_sandbox(sandbox.clone()).await.unwrap();
    let mut workspace = super::admission_tests::workspace(&binding);
    workspace.status = WorkspaceStatus::Ready;
    workspace.resolved_commit = Some("a".repeat(40));
    workspace.ready_at = Some(NOW.into());
    workspace.work.schedule(WorkspaceWorkStep::None, NOW.into());
    store.create_workspace(workspace.clone()).await.unwrap();
    let mut old = binding.clone();
    old.generation = 2;
    old.state = BindingState::Ready;
    let mut retired =
        BindingConversation::new(&old, "retired_generation_two".into(), NOW.into()).unwrap();
    retired.state = BindingConversationState::Retired;
    store
        .bindings
        .conversations
        .insert_one(conversation_document(&retired).unwrap())
        .await
        .unwrap();
    let deleted: layer2_core::sandbox::Agent = serde_json::from_value(serde_json::json!({
        "id":retired.agent_id,"worktree_id":retired.worktree_id,"workspace_id":binding.workspace_id,
        "sandbox_id":binding.sandbox_id,"owner":binding.owner,
        "creation_key":"retired-generation-two","creation_fingerprint":"d".repeat(64),
        "profile":{"id":"agp_retired","revision":1,"digest":"b".repeat(64),"content":binding.profile.agent},
        "capabilities":[],"status":"deleted",
        "conversation":{"conversation_id":"original-retired-conversation","runtime_generation":2,
            "last_seq":47,"first_message":null,"active_turn":null,"operation":null,"queued_messages":[]},
        "work":layer2_core::sandbox::AgentWorkState::new(layer2_core::sandbox::AgentWorkStep::None,NOW.into()),
        "failure":null,"title":null,"created_at":NOW,"updated_at":NOW,"ready_at":NOW,
        "stopped_at":NOW,"requested_stop_reason":null,"stop_reason":"owner","deleted_at":NOW,
        "origin":{"type":"manual"},"revision":5
    })).unwrap();
    let deleted_document = crate::store::agents::agent_document(&deleted).unwrap();
    store
        .agents
        .insert_one(deleted_document.clone())
        .await
        .unwrap();
    assert_eq!(
        store.pending_bindings(None, 1).await.unwrap(),
        vec![binding.clone()]
    );
    let mut current = binding.clone();
    current.state = BindingState::Ready;
    let pending =
        BindingConversation::new(&current, "pending_conversation".into(), NOW.into()).unwrap();
    store
        .bindings
        .conversations
        .insert_one(conversation_document(&pending).unwrap())
        .await
        .unwrap();
    assert!(controller
        .retry_recovered_binding(&binding, &key)
        .await
        .is_err());
    store
        .bindings
        .conversations
        .delete_one(doc! {"_id": conversation_storage_key(&pending)})
        .await
        .unwrap();

    // Missing custody is a refusal, never an instruction to recreate the key.
    custody.delete(&sandbox.id, 1).await.unwrap();
    assert!(controller
        .retry_recovered_binding(&binding, &key)
        .await
        .is_err());
    assert!(custody.get(&sandbox.id, 1).await.unwrap().is_none());
    custody.insert(material).await.unwrap();
    let mut busy = sandbox.clone();
    busy.work.schedule(SandboxWorkStep::Activate, NOW.into());
    busy.revision += 1;
    store
        .save_sandbox(sandbox.revision, busy.clone())
        .await
        .unwrap();
    assert!(controller
        .retry_recovered_binding(&binding, &key)
        .await
        .is_err());
    sandbox.revision = busy.revision + 1;
    store
        .save_sandbox(busy.revision, sandbox.clone())
        .await
        .unwrap();
    store
        .workspaces
        .update_one(
            doc! {"_id": workspace.id.as_str()},
            doc! {"$set": {"owned_workspace_cleanup_admission_v1": {"pending": true}}},
        )
        .await
        .unwrap();
    assert!(controller
        .retry_recovered_binding(&binding, &key)
        .await
        .is_err());
    store
        .workspaces
        .update_one(
            doc! {"_id": workspace.id.as_str()},
            doc! {"$unset": {"owned_workspace_cleanup_admission_v1": ""}},
        )
        .await
        .unwrap();
    let wrong = ProvidedCredential::new("nyx_wrong-key".into()).unwrap();
    assert!(controller
        .retry_recovered_binding(&binding, &wrong)
        .await
        .is_err());
    assert_eq!(
        store.binding(&binding.binding_id).await.unwrap(),
        Some(binding.clone())
    );
    let mut deleting = deleted.clone();
    deleting.status = layer2_core::sandbox::AgentStatus::Stopped;
    deleting.deleted_at = None;
    deleting.work.schedule(
        layer2_core::sandbox::AgentWorkStep::CloseProcess,
        NOW.into(),
    );
    deleting.revision += 1;
    store
        .agents
        .replace_one(
            doc! {"_id": deleted.id.as_str()},
            crate::store::agents::agent_document(&deleting).unwrap(),
        )
        .await
        .unwrap();
    assert!(controller.recover_binding(&binding).await.is_err());
    store
        .agents
        .replace_one(doc! {"_id": deleted.id.as_str()}, deleted_document.clone())
        .await
        .unwrap();

    let next = controller.recover_binding(&binding).await.unwrap();
    assert_eq!(next.state, BindingState::Ready);
    let mut expected = binding.clone();
    expected.state = BindingState::Ready;
    expected.advance_revision(next.updated_at.clone()).unwrap();
    assert_eq!(next, expected);
    assert_eq!(next.generation, 3);
    assert_eq!(
        store.get_sandbox(&sandbox.id).await.unwrap(),
        Some(sandbox.clone())
    );
    assert_eq!(
        store.get_workspace(&workspace.id).await.unwrap(),
        Some(workspace)
    );
    assert_eq!(
        store
            .binding_conversation(&binding.binding_id, 2, &retired.conversation_id)
            .await
            .unwrap(),
        Some(retired)
    );
    assert!(store
        .binding_conversations(&binding.binding_id, 3, None, 100)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        store
            .agents
            .find_one(doc! {"_id": deleted.id.as_str()})
            .await
            .unwrap(),
        Some(deleted_document)
    );
    assert_eq!(
        custody
            .get(&sandbox.id, 1)
            .await
            .unwrap()
            .unwrap()
            .value
            .expose(),
        key.expose()
    );
    assert!(custody.get(&sandbox.id, 2).await.unwrap().is_none());
    // A stale retry loses to a changed key and then to retirement. Neither can
    // be overwritten by the prior Failed binding revision.
    let mut changed = next
        .rotate_credential(wrong.sha256(), layer2_core::now_rfc3339())
        .unwrap();
    store
        .save_binding(Some(next.revision), changed.clone())
        .await
        .unwrap();
    assert!(controller
        .retry_recovered_binding(&binding, &key)
        .await
        .is_err());
    let revision = changed.revision;
    changed.state = BindingState::Retired;
    changed.work = BindingWork::RetireCurrentConversations;
    changed
        .advance_revision(layer2_core::now_rfc3339())
        .unwrap();
    store
        .save_binding(Some(revision), changed.clone())
        .await
        .unwrap();
    assert!(controller
        .retry_recovered_binding(&binding, &key)
        .await
        .is_err());
    assert!(controller
        .retry_recovered_binding(&changed, &wrong)
        .await
        .is_err());
    assert_eq!(
        store.binding(&binding.binding_id).await.unwrap(),
        Some(changed)
    );
    client.database(&database).drop().await.unwrap();
}
