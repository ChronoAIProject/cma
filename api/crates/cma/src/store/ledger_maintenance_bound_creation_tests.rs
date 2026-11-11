//! Child of bindings::mongo_tests, sharing the real binding fixture builders.
use super::super::admission_tests::{sandbox, workspace};
use super::*;
use crate::store::ledger_maintenance_test_support::{all_rows, pre_exclusion, retain_home};
use layer2_core::profiles::{content_digest, AgentProfileSnapshot};
use layer2_core::sandbox::binding::BindingAdmissionFence;
use layer2_core::sandbox::store::ledger_maintenance_pre_exclusion::LedgerPreExclusionStore;
use layer2_core::sandbox::store::*;
use layer2_core::sandbox::*;
use layer2_core::{AgentProfileId, RuntimeCapabilities, RuntimeCapability};

#[path = "ledger_maintenance_bound_command_tests.rs"]
mod bound_commands;

async fn parent(store: &MongoSandboxStore, binding: &ProviderBinding) -> Sandbox {
    let mut value = sandbox(binding);
    value.status = SandboxStatus::Active;
    value.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
    value.runtime = Some(RuntimeBinding {
        runtime_id: "fixture-runtime".into(),
        runtime_version: "fixture-image".into(),
        provision_generation: Some(1),
        boot_id: Some("b".repeat(32)),
        bound_at: NOW.into(),
    });
    value.lifecycle.runtime_generation_high_water = 1;
    retain_home(&mut value);
    value.validate().unwrap();
    store
        .sandboxes
        .insert_one(crate::store::sandboxes::sandbox_document(&value, 0).unwrap())
        .await
        .unwrap();
    value
}

async fn agent_request(
    store: &MongoSandboxStore,
    binding: &ProviderBinding,
    key: &str,
) -> AgentAdmission {
    let conversation = BindingConversation::new(binding, key.into(), NOW.into()).unwrap();
    store
        .save_binding_conversation(binding.revision, None, conversation.clone())
        .await
        .unwrap();
    let content = binding.profile.agent.clone();
    AgentAdmission {
        binding: Some(BindingAdmissionFence::conversation(binding, &conversation)),
        created_by: None,
        owner: binding.owner.clone(),
        workspace_id: binding.workspace_id.clone(),
        agent_id: conversation.agent_id.clone(),
        worktree_id: conversation.worktree_id.clone(),
        profile: AgentProfileSnapshot {
            id: AgentProfileId::generate(),
            revision: 1,
            digest: content_digest(&content).unwrap(),
            content,
        },
        capabilities: RuntimeCapabilities::new([RuntimeCapability::Conversation]),
        title: None,
        first_message: None,
        origin: AgentOrigin::Manual,
        creation_key: conversation.creation_key().unwrap(),
        event_at: NOW.into(),
    }
}

async fn ready_workspace(store: &MongoSandboxStore, binding: &ProviderBinding) {
    let mut value = workspace(binding);
    value.status = WorkspaceStatus::Ready;
    value.work = WorkspaceWorkState::new(WorkspaceWorkStep::None, NOW.into());
    value.ready_at = Some(NOW.into());
    value.resolved_commit = Some("a".repeat(40));
    store
        .workspaces
        .insert_one(crate::store::workspaces::workspace_document(&value).unwrap())
        .await
        .unwrap();
}

async fn admission_fence(store: &MongoSandboxStore, binding: &ProviderBinding) -> i64 {
    store
        .bindings
        .records
        .find_one(doc! {"_id":&binding.binding_id})
        .await
        .unwrap()
        .unwrap()
        .get_i64("admission_fence")
        .unwrap()
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and creation hooks"]
async fn bound_agent_creation_and_mutating_replay_refuse_under_hold_without_partial_fences() {
    let (client, name, store) = fixture().await;
    let binding = ready(&store).await;
    let parent = parent(&store, &binding).await;
    ready_workspace(&store, &binding).await;
    let existing = agent_request(&store, &binding, "existing_conversation").await;
    let original = match store.admit_agent(existing.clone()).await.unwrap() {
        AgentAdmissionResult::Created(creation) => creation,
        _ => panic!("expected a new bound Agent"),
    };
    let fence = admission_fence(&store, &binding).await;
    assert_eq!(
        store.admit_agent(existing.clone()).await.unwrap(),
        AgentAdmissionResult::Existing(original)
    );
    assert_eq!(
        admission_fence(&store, &binding).await,
        fence + 1,
        "bound replay really writes its authorization fence"
    );
    let pending = agent_request(&store, &binding, "pending_conversation").await;
    let (policy, input) = pre_exclusion(&store, &parent, NOW).await;
    store.admit_pre_exclusion(policy, input).await.unwrap();
    let before = all_rows(&store).await;
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    for request in [existing, pending] {
        assert_eq!(
            reopened.admit_agent(request).await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(all_rows(&store).await, before);
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and creation hooks"]
async fn bound_workspace_has_a_real_positive_control_and_held_binding_fence_rollback() {
    for held in [false, true] {
        let (client, name, store) = fixture().await;
        let mut binding = binding();
        store.save_binding(None, binding.clone()).await.unwrap();
        binding.state = BindingState::Provisioning;
        binding.work = BindingWork::ProvisionWorkspace;
        binding.advance_revision(NOW.into()).unwrap();
        store.save_binding(Some(1), binding.clone()).await.unwrap();
        let parent = parent(&store, &binding).await;
        let request = WorkspaceAdmission {
            binding: Some(BindingAdmissionFence::environment(&binding)),
            workspace: workspace(&binding),
            kind: WorkspaceAdmissionKind::Manual,
            event_at: NOW.into(),
        };
        if held {
            let (policy, input) = pre_exclusion(&store, &parent, NOW).await;
            store.admit_pre_exclusion(policy, input).await.unwrap();
        }
        let before = all_rows(&store).await;
        let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
        let result = reopened.admit_workspace(request).await;
        if held {
            assert_eq!(result, Err(SandboxStoreError::Conflict));
            assert_eq!(all_rows(&store).await, before);
        } else {
            assert!(matches!(result, Ok(WorkspaceAdmissionResult::Created(_))));
            assert_ne!(all_rows(&store).await, before);
        }
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and creation hooks"]
async fn failed_bound_agent_insert_rolls_back_both_binding_and_maintenance_fences() {
    let (client, name, store) = fixture().await;
    let binding = ready(&store).await;
    parent(&store, &binding).await;
    ready_workspace(&store, &binding).await;
    let input = agent_request(&store, &binding, "injected_failure").await;
    client.database(&name).run_command(doc! {"collMod":store.agents.name(),"validator":{"reject_fixture":{"$exists":true}},"validationLevel":"strict","validationAction":"error"}).await.unwrap();
    let before = all_rows(&store).await;
    assert!(matches!(
        store.admit_agent(input.clone()).await,
        Err(SandboxStoreError::Unavailable(_))
    ));
    assert_eq!(all_rows(&store).await, before);
    client
        .database(&name)
        .run_command(doc! {"collMod":store.agents.name(),"validator":{}})
        .await
        .unwrap();
    assert!(matches!(
        store.admit_agent(input).await,
        Ok(AgentAdmissionResult::Created(_))
    ));
    client.database(&name).drop().await.unwrap();
}
