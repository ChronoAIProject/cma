//! Real binding authorization writes must roll back on command-replay refusal.
use super::*;
use layer2_core::sandbox::agent_io::*;
use layer2_core::sandbox::binding::BindingConversationAuthority;
use layer2_core::sandbox::ledger_maintenance_admission::LedgerMaintenanceAdmissionPolicy;
use layer2_core::sandbox::ledger_maintenance_plan::LedgerMaintenancePlanV1;
use layer2_core::sandbox::store::ledger_maintenance::{
    LedgerMaintenanceAdmission, LedgerMaintenanceStore,
};
use layer3_adapter::runtime_ledger_maintenance::{
    LedgerBindingV1, LedgerSourceFormatV1, LEDGER_BINDING_FORMAT,
};

async fn command_fixture(
    store: &MongoSandboxStore,
) -> (Sandbox, CommandReservation, ProviderBinding) {
    let binding = ready(store).await;
    parent(store, &binding).await;
    ready_workspace(store, &binding).await;
    let input = agent_request(store, &binding, "command_replay_fixture").await;
    let created = store.admit_agent(input).await.unwrap();
    // Creation advances the persisted parent. AgentCreation carries only the
    // child result, so load the current parent for command admission's guard.
    let parent = layer2_core::sandbox::store::SandboxStore::get_sandbox(
        store,
        &created.creation().agent.sandbox_id,
    )
    .await
    .unwrap()
    .unwrap();
    let mut agent = created.creation().agent.clone();
    let revision = agent.revision;
    agent.status = AgentStatus::Ready;
    agent.work = AgentWorkState::new(AgentWorkStep::None, NOW.into());
    agent.ready_at = Some(NOW.into());
    agent.conversation.conversation_id = Some("bound-command-fixture".into());
    agent.conversation.runtime_generation = Some(1);
    agent.revision += 1;
    store.save_agent(revision, agent.clone()).await.unwrap();
    let mut conversation = store
        .binding_conversation(
            &binding.binding_id,
            binding.generation,
            "command_replay_fixture",
        )
        .await
        .unwrap()
        .unwrap();
    let revision = conversation.revision;
    conversation.state = BindingConversationState::Ready;
    conversation.revision += 1;
    store
        .save_binding_conversation(binding.revision, Some(revision), conversation.clone())
        .await
        .unwrap();
    let proof = BindingConversationAuthority {
        binding: binding.clone(),
        conversation,
    };
    let command = AgentCommand::message("Inspect retained bound fixture.");
    let request = CommandReservation {
        authority_deadline: None,
        admission_audit: None,
        envelope: AgentCommandEnvelope {
            continuation: None,
            schema_version: AGENT_IO_SCHEMA_VERSION,
            command_id: layer2_core::ids::AgentCommandId::generate(),
            agent_id: agent.id.clone(),
            owner: proof.binding.resource_owner(),
            actor: AgentActor::Api {
                subject: agent.owner.clone(),
                delegation_id: None,
            },
            source: AgentCommandSource {
                kind: AgentSourceKind::Api,
                reference: "fixture-source".into(),
            },
            correlation_id: "fixture-correlation".into(),
            request_fingerprint: command.fingerprint().unwrap(),
            authorization: proof.command_authority(),
            command,
        },
        binding: Some(proof),
        key_sha256: command_key_digest("bound-command-replay").unwrap(),
        response_id: Some(layer2_core::ids::AgentResponseId::generate()),
        admitted_at: NOW.into(),
        queue_deadline: "2026-09-22T00:05:00Z".into(),
        queue_capacity: 2,
        expected_agent_revision: agent.revision,
        parent: SandboxMutationGuard::capture(&parent),
        policy: layer2_core::resource_access::ResourcePolicyFence {
            sandbox_id: parent.id.clone(),
            sandbox_created_at: parent.created_at.clone(),
            policy_revision: 0,
        },
    };
    request.validate().unwrap();
    (parent, request, binding)
}

async fn hold(store: &MongoSandboxStore, parent: &Sandbox, v1: bool) {
    let (policy, input) = pre_exclusion(store, parent, NOW).await;
    if !v1 {
        store.admit_pre_exclusion(policy, input).await.unwrap();
        return;
    }
    // Synthetic disposable persistence binding only; no physical authority.
    let p = input.accepted.plan();
    let plan = LedgerMaintenancePlanV1 {
        format: "ledger-maintenance-plan-v1".into(),
        binding: LedgerBindingV1 {
            format: LEDGER_BINDING_FORMAT.into(),
            sandbox_id: parent.id.to_string(),
            home_identity: p.home_identity.clone(),
            generation: p.generation,
            boot_id: p.boot_id.clone(),
            roots_sha256: p.roots_sha256.clone(),
            source_format: LedgerSourceFormatV1::V1,
            archive_id: uuid::Uuid::from_u128(1).to_string(),
            maintenance_id: p.operation_id.clone(),
            maintenance_epoch: 1,
            sandbox_revision: parent.revision,
            candidate_sha256: "c".repeat(64),
            recovery_sha256: "d".repeat(64),
            exclusion_receipt_sha256: "e".repeat(64),
        },
        source: p.source.clone(),
        consent_receipt_sha256: p.consent_receipt_sha256.clone(),
        requested_at: NOW.into(),
    };
    store
        .admit_ledger_maintenance(
            LedgerMaintenanceAdmissionPolicy { enabled: true },
            LedgerMaintenanceAdmission {
                caller: input.caller,
                accepted: crate::store::ledger_maintenance_test_support::artifact(&plan),
                lease: input.lease,
                now: NOW.into(),
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and bound replay hook"]
async fn bound_replay_keeps_authorization_serialization_and_refuses_both_holds_after_reopen() {
    for v1 in [false, true] {
        let (client, name, store) = fixture().await;
        let (parent, request, binding) = command_fixture(&store).await;
        let original = match store.reserve_command(request.clone()).await.unwrap() {
            CommandReservationResult::Created(record) => record,
            _ => panic!("expected new bound input"),
        };
        let fence = admission_fence(&store, &binding).await;
        assert_eq!(
            store.reserve_command(request.clone()).await.unwrap(),
            CommandReservationResult::Existing(original.clone())
        );
        assert_eq!(admission_fence(&store, &binding).await, fence + 1);
        hold(&store, &parent, v1).await;
        let before = all_rows(&store).await;
        let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
        assert_eq!(
            reopened.reserve_command(request.clone()).await,
            Err(CommandLedgerError::NotAdmissible)
        );
        assert_eq!(all_rows(&store).await, before);
        assert_eq!(
            reopened
                .get_command(&original.receipt.envelope.command_id)
                .await
                .unwrap(),
            Some(*original)
        );
        let mut changed = request;
        changed.envelope.command = AgentCommand::message("Changed fixture intent.");
        changed.envelope.request_fingerprint = changed.envelope.command.fingerprint().unwrap();
        assert_eq!(
            reopened.reserve_command(changed).await,
            Err(CommandLedgerError::IdempotencyConflict)
        );
        assert_eq!(all_rows(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and bound replay hook"]
async fn missing_or_inconsistent_bound_agent_cannot_commit_a_replay_authorization_fence() {
    for missing in [false, true] {
        let (client, name, store) = fixture().await;
        let (_, request, _) = command_fixture(&store).await;
        store.reserve_command(request.clone()).await.unwrap();
        let expected = if missing {
            store.agents.delete_many(doc! {}).await.unwrap();
            CommandLedgerError::NotFound
        } else {
            let mut agent = store
                .get_agent(&request.envelope.agent_id)
                .await
                .unwrap()
                .unwrap();
            agent.sandbox_id = layer2_core::SandboxId::generate();
            store
                .agents
                .replace_one(
                    doc! {"_id":agent.id.as_str()},
                    crate::store::agents::agent_document(&agent).unwrap(),
                )
                .await
                .unwrap();
            CommandLedgerError::Corrupt
        };
        let before = all_rows(&store).await;
        assert_eq!(store.reserve_command(request).await, Err(expected));
        assert_eq!(all_rows(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}
