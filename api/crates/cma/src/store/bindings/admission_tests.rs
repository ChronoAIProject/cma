use super::mongo_tests::{binding, fixture, NOW};
use super::*;
use layer2_core::profiles::{
    content_digest, AgentProfileSnapshot, SandboxProfileSnapshot, WorkspaceProfileSnapshot,
};
use layer2_core::sandbox::binding::{
    BindingAdmissionFence, BindingConversationState, BindingState, BindingWork,
};
use layer2_core::sandbox::store::*;
use layer2_core::sandbox::*;
use layer2_core::{
    AgentProfileId, RuntimeCapabilities, RuntimeCapability, SandboxProfileId, WorkspaceProfileId,
};

pub(super) fn sandbox(value: &ProviderBinding) -> Sandbox {
    let content = value.profile.environment.as_ref().unwrap().sandbox.clone();
    Sandbox {
        id: value.sandbox_id.clone(),
        owner: value.owner.clone(),
        name: "binding-admission".into(),
        origin: SandboxOrigin::Manual,
        profile: SandboxProfileSnapshot {
            id: SandboxProfileId::generate(),
            revision: 1,
            digest: content_digest(&content).unwrap(),
            always_on: None,
            content: content.clone(),
        },
        status: SandboxStatus::Provisioning,
        failure: None,
        runtime: None,
        credential: SandboxCredentialState {
            source: value.credential_source(),
            service_grants: vec![],
            service_scope: None,
            reaches: content.nyxid_services,
            key_generation: 0,
            delivered: false,
            current: None,
            pending: None,
            revoke_debt: vec![],
            sign_in: SandboxSignInState::NotRequired,
        },
        work: SandboxWorkState::new(SandboxWorkStep::ProvisionRuntime, NOW.into()),
        limits: SandboxLimits {
            max_workspaces: 1,
            max_agents_per_workspace: 16,
        },
        lifecycle: Default::default(),
        last_activity_at: NOW.into(),
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
        purge_after: None,
        revision: 1,
    }
}

pub(super) fn workspace(value: &ProviderBinding) -> Workspace {
    let content = value
        .profile
        .environment
        .as_ref()
        .unwrap()
        .workspace
        .clone();
    Workspace {
        id: value.workspace_id.clone(),
        sandbox_id: value.sandbox_id.clone(),
        owner: value.owner.clone(),
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: WorkspaceProfileId::generate(),
            revision: 1,
            digest: content_digest(&content).unwrap(),
            content,
        },
        name: "project".into(),
        path: "/home/cma/project".into(),
        source: WorkspaceSource::Empty,
        resolved_commit: None,
        status: WorkspaceStatus::Preparing,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::Prepare, NOW.into()),
        next_worktree_number: 1,
        created_at: NOW.into(),
        ready_at: None,
        failed: None,
        removed_at: None,
        revision: 1,
    }
}

async fn advance<S: BindingStore>(
    store: &S,
    value: &mut ProviderBinding,
    state: BindingState,
    work: BindingWork,
) {
    let expected = value.revision;
    value.state = state;
    value.work = work;
    value.advance_revision(NOW.into()).unwrap();
    store
        .save_binding(Some(expected), value.clone())
        .await
        .unwrap();
}

async fn contract<S>(store: &S, mongo: Option<&MongoSandboxStore>)
where
    S: BindingStore
        + SandboxStore
        + SandboxAdmissionStore
        + WorkspaceStore
        + WorkspaceAdmissionStore
        + AgentStore
        + AgentAdmissionStore
        + WorktreeStore,
{
    let mut value = binding();
    store.save_binding(None, value.clone()).await.unwrap();
    advance(
        store,
        &mut value,
        BindingState::Provisioning,
        BindingWork::ProvisionSandbox,
    )
    .await;
    let parent = sandbox(&value);
    let mut input = SandboxAdmission::new(
        parent.clone(),
        SandboxAdmissionName::Exact(parent.name.clone()),
        3,
        NOW.into(),
    )
    .unwrap();
    input.binding = Some(BindingAdmissionFence::environment(&value));
    let stale = input.clone();
    advance(
        store,
        &mut value,
        BindingState::Provisioning,
        BindingWork::ProvisionSandbox,
    )
    .await;
    assert_eq!(
        store.admit_sandbox(stale).await,
        Err(SandboxStoreError::Conflict)
    );
    assert!(store.get_sandbox(&parent.id).await.unwrap().is_none());
    input.binding = Some(BindingAdmissionFence::environment(&value));
    store.admit_sandbox(input).await.unwrap();
    let mut parent = store.get_sandbox(&parent.id).await.unwrap().unwrap();
    parent.status = SandboxStatus::Active;
    parent.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
    parent.revision += 1;
    store.save_sandbox(1, parent.clone()).await.unwrap();

    advance(
        store,
        &mut value,
        BindingState::Provisioning,
        BindingWork::ProvisionWorkspace,
    )
    .await;
    let child = workspace(&value);
    let mut input = WorkspaceAdmission {
        binding: Some(BindingAdmissionFence::environment(&value)),
        workspace: child.clone(),
        kind: WorkspaceAdmissionKind::Manual,
        event_at: NOW.into(),
    };
    let stale = input.clone();
    advance(
        store,
        &mut value,
        BindingState::Provisioning,
        BindingWork::ProvisionWorkspace,
    )
    .await;
    assert_eq!(
        store.admit_workspace(stale).await,
        Err(SandboxStoreError::Conflict)
    );
    assert!(store.get_workspace(&child.id).await.unwrap().is_none());
    input.binding = Some(BindingAdmissionFence::environment(&value));
    store.admit_workspace(input).await.unwrap();
    let mut child = store.get_workspace(&child.id).await.unwrap().unwrap();
    child.status = WorkspaceStatus::Ready;
    child.resolved_commit = Some("a".repeat(40));
    child.ready_at = Some(NOW.into());
    child.work = WorkspaceWorkState::new(WorkspaceWorkStep::None, NOW.into());
    child.revision += 1;
    store.save_workspace(1, child.clone()).await.unwrap();
    advance(store, &mut value, BindingState::Ready, BindingWork::Idle).await;

    let conversation =
        BindingConversation::new(&value, "conversation_one".into(), NOW.into()).unwrap();
    store
        .save_binding_conversation(value.revision, None, conversation.clone())
        .await
        .unwrap();
    let content = value.profile.agent.clone();
    let input = AgentAdmission {
        binding: Some(BindingAdmissionFence::conversation(&value, &conversation)),
        created_by: None,
        owner: value.owner.clone(),
        workspace_id: child.id.clone(),
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
    };
    let mut wrong = input.clone();
    wrong.agent_id = layer2_core::AgentId::generate();
    assert_eq!(
        store.admit_agent(wrong).await,
        Err(SandboxStoreError::Conflict)
    );
    let created = store.admit_agent(input.clone()).await.unwrap();
    assert_eq!(created.creation().agent.id, conversation.agent_id);
    let binding_filter = doc! {"_id": &value.binding_id};
    let hold_filter = doc! {"_id": parent.id.as_str()};
    let binding_before = match mongo {
        Some(mongo) => {
            use crate::store::work_claim_authority::LEDGER_MAINTENANCE_FIELD;
            let updated = mongo
                .sandboxes
                .update_one(
                    hold_filter.clone(),
                    doc! {"$set": {LEDGER_MAINTENANCE_FIELD: null}},
                )
                .await
                .unwrap();
            assert_eq!(updated.matched_count, 1);
            Some(
                mongo
                    .bindings
                    .records
                    .find_one(binding_filter.clone())
                    .await
                    .unwrap()
                    .unwrap(),
            )
        }
        None => None,
    };
    let replay = store.admit_agent(input.clone()).await.unwrap();
    assert!(matches!(replay, AgentAdmissionResult::Existing(_)));
    assert_eq!(replay.creation(), created.creation());
    if let (Some(mongo), Some(binding_before)) = (mongo, binding_before) {
        assert_eq!(
            mongo
                .bindings
                .records
                .find_one(binding_filter)
                .await
                .unwrap(),
            Some(binding_before)
        );
    }
    let mut retiring = conversation.clone();
    retiring.state = BindingConversationState::Retiring;
    retiring.revision += 1;
    store
        .save_binding_conversation(value.revision, Some(conversation.revision), retiring)
        .await
        .unwrap();
    assert_eq!(
        store.admit_agent(input.clone()).await,
        Err(SandboxStoreError::Conflict)
    );
    if let Some(mongo) = mongo {
        use crate::store::work_claim_authority::LEDGER_MAINTENANCE_FIELD;
        mongo
            .sandboxes
            .update_one(hold_filter, doc! {"$unset": {LEDGER_MAINTENANCE_FIELD: ""}})
            .await
            .unwrap();
    }

    let other = BindingConversation::new(&value, "conversation_two".into(), NOW.into()).unwrap();
    store
        .save_binding_conversation(value.revision, None, other.clone())
        .await
        .unwrap();
    let mut input = input;
    input.agent_id = other.agent_id.clone();
    input.worktree_id = other.worktree_id.clone();
    input.creation_key = other.creation_key().unwrap();
    input.binding = Some(BindingAdmissionFence::conversation(&value, &other));
    advance(
        store,
        &mut value,
        BindingState::Retired,
        BindingWork::RetireCurrentConversations,
    )
    .await;
    assert_eq!(
        store.admit_agent(input).await,
        Err(SandboxStoreError::Conflict)
    );
    assert!(store.get_agent(&other.agent_id).await.unwrap().is_none());
    assert!(store
        .get_worktree(&other.worktree_id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn memory_binding_fences_resource_admission_and_conversation_retirement() {
    contract(&InMemorySandboxStore::default(), None).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_binding_fences_resource_admission_and_conversation_retirement() {
    let (client, database, store) = fixture().await;
    contract(&store, Some(&store)).await;
    client.database(&database).drop().await.unwrap();
}
