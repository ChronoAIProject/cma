use super::super::agent_admission_tests::{admission, created, ready_store};
use super::*;
use crate::ids::{AgentCommandId, AgentResponseId};
use crate::sandbox::store::{AgentAdmissionStore, SandboxMutationGuard};
use crate::sandbox::{AgentOperationDelivery, AgentStatus};
use crate::{RuntimeCapabilities, RuntimeCapability};

pub(super) const NOW: &str = "2026-09-05T00:00:00Z";

pub(in crate::sandbox::store::memory) async fn fixture() -> (
    InMemorySandboxStore,
    Agent,
    TargetedControlReservation,
    AdmittedAgentCommand,
) {
    let store = ready_store(1).await;
    let mut agent = created(store.admit_agent(admission("targeted")).await.unwrap()).agent;
    agent.status = AgentStatus::Working;
    agent.conversation.active_turn = Some(2);
    agent.capabilities = RuntimeCapabilities::new([
        RuntimeCapability::Conversation,
        RuntimeCapability::Interrupt,
        RuntimeCapability::Steer,
    ]);
    store.lock().agents.insert(agent.id.clone(), agent.clone());
    let parent = store
        .lock()
        .sandboxes
        .get(&agent.sandbox_id)
        .unwrap()
        .clone();
    let command = AgentCommand::Interrupt;
    let ordinary = CommandReservation {
        binding: None,
        nyxid_trace: None,
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
                kind: AgentSourceKind::Api,
                reference: "targeted-fixture".into(),
            },
            correlation_id: "targeted-fixture".into(),
            request_fingerprint: command.fingerprint().unwrap(),
            authorization: AgentAuthorizationRevision {
                authority_id: "owner-custody".into(),
                policy_revision: 1,
                grant_revision: None,
            },
            command,
        },
        key_sha256: command_key_digest("control").unwrap(),
        response_id: None,
        admitted_at: NOW.into(),
        queue_deadline: "2026-09-05T00:05:00Z".into(),
        queue_capacity: 8,
        expected_agent_revision: agent.revision,
        parent: SandboxMutationGuard::capture(&parent),
        policy: crate::resource_access::ResourcePolicyFence {
            sandbox_id: parent.id.clone(),
            sandbox_created_at: parent.created_at.clone(),
            policy_revision: 0,
        },
        authority_deadline: None,
        admission_audit: None,
    };
    let mut original = ordinary.clone();
    original.envelope.command_id = AgentCommandId::generate();
    original.envelope.command = AgentCommand::message("original partial output stays retained");
    original.envelope.request_fingerprint = original.envelope.command.fingerprint().unwrap();
    original.response_id = Some(AgentResponseId::generate());
    original.key_sha256 = command_key_digest("original-response").unwrap();
    let mut response = original.record(1, 0, None).unwrap();
    response.status = AgentResponseStatus::InProgress;
    response.work.dispatch = Some(CommandDispatch {
        claim: CommandClaim {
            agent_id: agent.id.clone(),
            command_id: original.envelope.command_id.clone(),
            holder: "original-worker".into(),
            fence: 1,
        },
        operation_sequence: 1,
        turn_sequence: 2,
        runtime_generation: 1,
        fenced_at: NOW.into(),
        delivery: AgentOperationDelivery::WriteConfirmed,
    });
    response.validate().unwrap();
    {
        let mut state = store.lock();
        state
            .agent_commands
            .insert(original.envelope.command_id.clone(), response.clone());
        state.agent_command_keys.insert(
            (original.scope(), original.key_sha256),
            original.envelope.command_id,
        );
        state
            .agent_command_orders
            .entry(agent.id.clone())
            .or_default()
            .last_order = 1;
    }
    let request = TargetedControlReservation {
        command: ordinary,
        target: TargetedResponse {
            response_id: response.receipt.response_id.clone().unwrap(),
            turn_sequence: 2,
        },
        observed_agent_revision: agent.revision,
        agent_created_at: agent.created_at.clone(),
        retry_deadline: None,
    };
    (store, agent, request, response)
}

pub(super) fn set_command(request: &mut TargetedControlReservation, command: AgentCommand) {
    request.command.envelope.request_fingerprint = command.fingerprint().unwrap();
    request.command.envelope.command = command;
}

pub(super) fn counts(store: &InMemorySandboxStore) -> (usize, usize, usize, usize, Vec<u64>) {
    let state = store.lock();
    (
        state.agent_commands.len(),
        state.targeted_controls.len(),
        state.agent_command_keys.len(),
        state.command_authorization_audits.len(),
        state
            .agent_command_orders
            .values()
            .map(|lane| lane.last_order)
            .collect(),
    )
}
