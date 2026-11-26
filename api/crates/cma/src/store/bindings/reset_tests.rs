use super::*;
include!("gateway_reset_tests.rs");
use layer2_core::{
    profiles::content_digest,
    resource_access::{ResourceAccessStore, ResourceTarget},
    sandbox::{
        binding::*,
        store::{AgentLogStore, AgentStore, SandboxStore, WorkspaceStore},
        AgentStatus, AgentStopReason, AgentWorkState, AgentWorkStep, SandboxWorkState,
        SandboxWorkStep, WorkspaceWorkState, WorkspaceWorkStep,
    },
};

pub(super) async fn fixture() -> (
    mongodb::Client,
    String,
    MongoSandboxStore,
    ProviderBinding,
    BindingConversation,
) {
    let (client, database, _) = super::mongo_tests::fixture().await;
    let (store, target) =
        crate::store::sandbox_mongo_integration_tests::resource_access_tests::fixture(
            &client, &database,
        )
        .await;
    let snap = store.resource_access_snapshot(&target).await.unwrap();
    let mut agent = snap.lineage.agent.unwrap();
    let mut binding = super::mongo_tests::binding();
    binding.owner = agent.owner.clone();
    binding.profile.owner = agent.owner.clone();
    binding.sandbox_id = agent.sandbox_id.clone();
    binding.workspace_id = agent.workspace_id.clone();
    store.save_binding(None, binding.clone()).await.unwrap();
    binding.state = BindingState::Ready;
    binding.work = BindingWork::Idle;
    binding
        .advance_revision(super::mongo_tests::NOW.into())
        .unwrap();
    store.save_binding(Some(1), binding.clone()).await.unwrap();
    let mut mapping = BindingConversation::new(
        &binding,
        "oracle-partition".into(),
        super::mongo_tests::NOW.into(),
    )
    .unwrap();
    mapping.agent_id = agent.id.clone();
    mapping.worktree_id = agent.worktree_id.clone();
    store
        .save_binding_conversation(binding.revision, None, mapping.clone())
        .await
        .unwrap();
    mapping.state = BindingConversationState::Ready;
    mapping.revision += 1;
    store
        .save_binding_conversation(binding.revision, Some(1), mapping.clone())
        .await
        .unwrap();
    agent
        .transition(
            AgentStatus::Stopped,
            layer2_core::now_rfc3339(),
            Some(AgentStopReason::Owner),
        )
        .unwrap();
    agent
        .transition(AgentStatus::Deleted, layer2_core::now_rfc3339(), None)
        .unwrap();
    agent.work = AgentWorkState::new(AgentWorkStep::None, layer2_core::now_rfc3339());
    store
        .agents
        .replace_one(
            doc! {"_id":agent.id.as_str()},
            crate::store::agents::agent_document(&agent).unwrap(),
        )
        .await
        .unwrap();
    let mut sandbox = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    let revision = sandbox.revision;
    sandbox.work = SandboxWorkState::new(SandboxWorkStep::None, layer2_core::now_rfc3339());
    sandbox.revision += 1;
    store.save_sandbox(revision, sandbox).await.unwrap();
    let mut workspace = store
        .get_workspace(&agent.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let revision = workspace.revision;
    workspace.work = WorkspaceWorkState::new(WorkspaceWorkStep::None, layer2_core::now_rfc3339());
    workspace.revision += 1;
    store.save_workspace(revision, workspace).await.unwrap();
    (client, database, store, binding, mapping)
}
pub(super) fn request(
    binding: &ProviderBinding,
    mapping: &BindingConversation,
    key: &str,
) -> ConversationResetRequest {
    ConversationResetRequest {
        binding_id: binding.binding_id.clone(),
        idempotency_binding_id: binding.binding_id.clone(),
        conversation_id: "oracle-partition".into(),
        expected_agent_id: mapping.agent_id.clone(),
        owner: binding.resource_owner(),
        manager: binding.manager.clone(),
        operation_id: content_digest(&key).unwrap(),
        request_digest: content_digest(&(key, "request")).unwrap(),
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_partition_reset_replays_after_reopen_and_fences_racing_writers() {
    let (client, database, store, binding, old) = fixture().await;
    let reset = request(&binding, &old, "same-operation");
    assert!(store
        .conversation_reset_state(&binding.binding_id)
        .await
        .unwrap()
        .heads
        .is_empty());
    assert!(matches!(
        store.reset_binding_conversation(reset.clone(), false).await,
        Err(BindingError::ResetRefused("partition_reset_disabled"))
    ));
    let logs = store.agent_log(&old.agent_id, 0, 100).await.unwrap();
    let before_indexes: Vec<_> = store
        .bindings
        .records
        .list_indexes()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let two = MongoSandboxStore::open(&client, &database).await.unwrap();
    let (a, b) = tokio::join!(
        store.reset_binding_conversation(reset.clone(), true),
        two.reset_binding_conversation(reset.clone(), true)
    );
    let receipt = a.unwrap();
    assert_eq!(receipt, b.unwrap());
    assert_eq!(receipt.head.incarnation, 1);
    assert!(store
        .get_agent(&receipt.head.successor.agent_id)
        .await
        .unwrap()
        .is_none());
    assert!(store
        .pending_binding_conversations(None, 100)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(store.agent_log(&old.agent_id, 0, 100).await.unwrap(), logs);
    assert_eq!(
        store
            .binding_conversation(&binding.binding_id, 1, &old.conversation_id)
            .await
            .unwrap(),
        Some(old.clone())
    );
    let reopened = MongoSandboxStore::open(&client, &database).await.unwrap();
    assert_eq!(
        reopened
            .reset_binding_conversation(reset.clone(), false)
            .await
            .unwrap(),
        receipt
    );
    let mut changed = reset.clone();
    changed.request_digest = "f".repeat(64);
    assert!(matches!(
        reopened.reset_binding_conversation(changed, true).await,
        Err(BindingError::ResetRefused("partition_reset_key_conflict"))
    ));
    // A stale ensure/provision snapshot cannot write after the reset's parent CAS.
    assert!(store
        .save_binding_conversation(binding.revision, None, receipt.head.successor.clone())
        .await
        .is_err());
    let current = store.binding(&binding.binding_id).await.unwrap().unwrap();
    let (a, b) = tokio::join!(
        store.save_binding_conversation(current.revision, None, receipt.head.successor.clone()),
        two.save_binding_conversation(current.revision, None, receipt.head.successor.clone())
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let stale = request(&binding, &old, "another-operation");
    assert!(matches!(
        store.reset_binding_conversation(stale, true).await,
        Err(BindingError::ResetRefused(
            "partition_reset_mapping_changed"
        ))
    ));
    let row = store
        .bindings
        .records
        .find_one(doc! {"_id":&binding.binding_id})
        .await
        .unwrap()
        .unwrap();
    assert!(row.contains_key("conversation_resets_v1"));
    assert_eq!(current.reset_format, Some(ConversationResetFormat::V1));
    assert_eq!(
        row.get_document("record")
            .unwrap()
            .get_str("reset_format")
            .unwrap(),
        "v1"
    );
    let after_indexes: Vec<_> = store
        .bindings
        .records
        .list_indexes()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert_eq!(format!("{before_indexes:?}"), format!("{after_indexes:?}"));
    // Metadata-only updates must preserve reset metadata outside the strict record.
    let mut next = current.clone();
    next.advance_revision(layer2_core::now_rfc3339()).unwrap();
    store
        .save_binding(Some(current.revision), next)
        .await
        .unwrap();
    assert_eq!(
        store
            .reset_binding_conversation(reset, false)
            .await
            .unwrap(),
        receipt
    );
    let target = ResourceTarget::Agent {
        agent_id: old.agent_id,
    };
    assert_eq!(
        store
            .resource_access_snapshot(&target)
            .await
            .unwrap()
            .lineage
            .agent
            .unwrap()
            .status,
        AgentStatus::Deleted
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_partition_reset_rejects_old_agent_with_fresh_binding_authority() {
    use layer2_core::sandbox::agent_io::*;
    let (client, database, store, binding, old) = fixture().await;
    store
        .reset_binding_conversation(request(&binding, &old, "reset"), true)
        .await
        .unwrap();
    let current = store.binding(&binding.binding_id).await.unwrap().unwrap();
    let agent = store.get_agent(&old.agent_id).await.unwrap().unwrap();
    let sandbox = store
        .get_sandbox(&current.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let proof = BindingConversationAuthority {
        binding: current,
        conversation: old.clone(),
    };
    let mut input = command_request(&agent, &sandbox, "stale-resolution");
    input.envelope.authorization = proof.command_authority();
    input.envelope.owner = proof.binding.resource_owner();
    input.envelope.actor = AgentActor::Api {
        subject: proof.binding.owner.clone(),
        delegation_id: None,
    };
    input.envelope.source.kind = AgentSourceKind::Api;
    let AgentCommand::Message { input: content, .. } = &mut input.envelope.command else {
        unreachable!()
    };
    content.content.push(AgentContent::ExternalEvent {
        context: ExternalEventContext {
            provider_conversation_id: None,
            source_id: "channel".into(),
            conversation_id: "oracle-partition".into(),
            event_id: "queued-event".into(),
            reference: "original-reference".into(),
            payload: serde_json::json!({"event":"queued"}),
        },
    });
    input.envelope.request_fingerprint = input.envelope.command.fingerprint().unwrap();
    input.binding = Some(proof);
    assert_eq!(
        store.reserve_command(input).await,
        Err(CommandLedgerError::Conflict)
    );
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        0
    );
    assert!(store
        .binding_conversation(
            &binding.binding_id,
            binding.generation,
            &old.conversation_id
        )
        .await
        .unwrap()
        .is_some());
    client.database(&database).drop().await.unwrap();
}

fn command_request(
    agent: &layer2_core::sandbox::Agent,
    parent: &layer2_core::sandbox::Sandbox,
    key: &str,
) -> layer2_core::sandbox::agent_io::CommandReservation {
    use layer2_core::ids::{AgentCommandId, AgentResponseId};
    use layer2_core::sandbox::{agent_io::*, store::SandboxMutationGuard};
    let command = AgentCommand::message("Inspect this.");
    CommandReservation {
        binding: None,
        authority_deadline: None,
        admission_audit: None,
        envelope: AgentCommandEnvelope {
            continuation: None,
            schema_version: AGENT_IO_SCHEMA_VERSION,
            command_id: AgentCommandId::generate(),
            agent_id: agent.id.clone(),
            owner: AgentOwner::Person {
                subject: agent.owner.clone(),
            },
            actor: AgentActor::Human {
                subject: agent.owner.clone(),
            },
            source: AgentCommandSource {
                kind: AgentSourceKind::UserInterface,
                reference: "interaction-one".into(),
            },
            correlation_id: "correlation-one".into(),
            request_fingerprint: command.fingerprint().unwrap(),
            authorization: AgentAuthorizationRevision {
                replay_principal: None,
                authority_id: "personal-owner".into(),
                policy_revision: 1,
                grant_revision: None,
            },
            command,
        },
        key_sha256: command_key_digest(key).unwrap(),
        response_id: Some(AgentResponseId::generate()),
        admitted_at: super::mongo_tests::NOW.into(),
        queue_deadline: "2026-09-22T00:02:00Z".into(),
        queue_capacity: 2,
        expected_agent_revision: agent.revision,
        parent: SandboxMutationGuard::capture(parent),
        policy: layer2_core::resource_access::ResourcePolicyFence {
            sandbox_id: parent.id.clone(),
            sandbox_created_at: parent.created_at.clone(),
            policy_revision: 0,
        },
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_profile_transfer_pins_one_successor_and_retains_exact_receipt() {
    let (client, database, store, binding, previous) = fixture().await;
    let mut destination = binding.clone();
    destination.binding_id = "new-profile-fixture".into();
    destination.key_sha256 = "b".repeat(64);
    destination.revision = 1;
    destination.state = BindingState::Provisioning;
    destination.work = BindingWork::AwaitingCredential;
    store.save_binding(None, destination.clone()).await.unwrap();
    destination.state = BindingState::Ready;
    destination.work = BindingWork::Idle;
    destination
        .advance_revision(layer2_core::now_rfc3339())
        .unwrap();
    store
        .save_binding(Some(1), destination.clone())
        .await
        .unwrap();

    let request = ProfileTransferRequest {
        source_binding_id: binding.binding_id.clone(),
        destination_binding_id: destination.binding_id.clone(),
        conversation_id: previous.conversation_id.clone(),
        expected_agent_id: previous.agent_id.clone(),
        expected_generation: binding.generation,
        expected_destination_generation: destination.generation,
        expected_incarnation: 0,
        owner: binding.resource_owner(),
        manager: binding.manager.clone(),
        operation_id: content_digest(&"transfer-operation").unwrap(),
        request_digest: content_digest(&"transfer-request").unwrap(),
    };
    assert!(matches!(
        store
            .transfer_profile_conversation(request.clone(), false)
            .await,
        Err(BindingError::ResetRefused("partition_reset_disabled"))
    ));
    let peer = MongoSandboxStore::open(&client, &database).await.unwrap();
    let (one, two) = tokio::join!(
        store.transfer_profile_conversation(request.clone(), true),
        peer.transfer_profile_conversation(request.clone(), true)
    );
    let receipt = one.unwrap();
    assert_eq!(receipt, two.unwrap());
    let source = store
        .binding_conversation(
            &binding.binding_id,
            binding.generation,
            &previous.conversation_id,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(source.state, BindingConversationState::Retired);
    assert_eq!(source.profile_transfer.as_deref(), Some(&receipt));
    let successor = peer
        .binding_conversation(
            &destination.binding_id,
            destination.generation,
            &previous.conversation_id,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(successor.agent_id, receipt.destination_agent_id);
    assert_eq!(successor.worktree_id, receipt.destination_worktree_id);
    assert_eq!(successor.state, BindingConversationState::Provisioning);
    assert_ne!(successor.agent_id, previous.agent_id);
    assert_eq!(
        peer.transfer_profile_conversation(request.clone(), false)
            .await
            .unwrap(),
        receipt
    );
    let mut changed = request;
    changed.request_digest = content_digest(&"different-request").unwrap();
    assert!(matches!(
        peer.transfer_profile_conversation(changed, true).await,
        Err(BindingError::ResetRefused("partition_reset_key_conflict"))
    ));
    assert_eq!(
        store
            .binding_conversations(&destination.binding_id, destination.generation, None, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    client.database(&database).drop().await.unwrap();
}
