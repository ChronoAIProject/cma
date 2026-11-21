use super::targeted_controls_tests::fixtures;
use crate::sandbox::agent_io::*;
use crate::sandbox::store::{AgentStore, InMemorySandboxStore};
use crate::sandbox::{Agent, AgentStatus};

#[path = "queued_cancellations_failure_tests.rs"]
mod failures;

pub(super) async fn queued() -> (
    InMemorySandboxStore,
    Agent,
    QueuedCancellationReservation,
    AdmittedAgentCommand,
) {
    let (store, mut agent, control, mut source) = fixtures::fixture().await;
    agent.status = AgentStatus::Ready;
    agent.conversation.active_turn = None;
    source.status = AgentResponseStatus::Queued;
    source.work = CommandWorkState::default();
    source.queue_deadline = "2030-09-05T00:00:00Z".into();
    let request = QueuedCancellationReservation {
        command: control.command,
        target: QueuedCancellationTarget {
            response_id: source.receipt.response_id.clone().unwrap(),
            source_command_id: source.receipt.envelope.command_id.clone(),
            expected_command_revision: source.revision,
        },
        observed_agent_revision: agent.revision,
        agent_created_at: agent.created_at.clone(),
    };
    {
        let mut state = store.lock();
        state.agents.insert(agent.id.clone(), agent.clone());
        state
            .agent_commands
            .insert(source.receipt.envelope.command_id.clone(), source.clone());
    }
    (store, agent, request, source)
}

fn applied(value: QueuedCancellationOutcome) -> AdmittedQueuedCancellation {
    match value {
        QueuedCancellationOutcome::Applied(value) => *value,
        other => panic!("expected applied cancellation, got {other:?}"),
    }
}

pub(super) fn claim_request(agent: &Agent, holder: &str) -> CommandClaimRequest {
    let now = std::time::SystemTime::now();
    CommandClaimRequest {
        agent_id: agent.id.clone(),
        holder: holder.into(),
        now: crate::rfc3339_at(now),
        expires_at: crate::rfc3339_at(now + std::time::Duration::from_secs(30)),
    }
}

#[tokio::test]
async fn queued_cancellation_is_atomic_terminal_and_works_without_runtime_or_queue_capacity() {
    let (store, mut agent, mut request, source) = queued().await;
    agent.status = AgentStatus::Paused;
    agent.capabilities = crate::RuntimeCapabilities::default();
    store.lock().agents.insert(agent.id.clone(), agent.clone());
    request.command.queue_capacity = 1;
    let original_logs = store.lock().agent_logs.get(&agent.id).cloned();
    let original = applied(store.cancel_queued_response(request.clone()).await.unwrap());
    assert_eq!(original.response.receipt, source.receipt);
    assert_eq!(original.response.status, AgentResponseStatus::Cancelled);
    assert_eq!(original.response.revision, source.revision + 1);
    assert_eq!(original.response.work, CommandWorkState::default());
    assert_eq!(original.record.status, AgentResponseStatus::Completed);
    assert_eq!(original.record.target_turn, None);
    assert_eq!(original.record.work, CommandWorkState::default());
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap(),
        Some(agent.clone())
    );
    let recovered = applied(store.cancel_queued_response(request.clone()).await.unwrap());
    assert_eq!(recovered, original);
    {
        let state = store.lock();
        assert_eq!(state.agent_commands.len(), 2);
        assert_eq!(state.queued_cancellations.len(), 1);
        assert_eq!(state.agent_command_orders[&agent.id].last_order, 2);
        assert_eq!(state.agent_logs.get(&agent.id).cloned(), original_logs);
    }
    let legacy = store.reserve_command(request.command).await.unwrap();
    assert_eq!(
        legacy,
        CommandReservationResult::Existing(Box::new(original.record))
    );
}

#[tokio::test]
async fn queued_cancellation_invalidates_a_live_claim_without_reusing_its_fence() {
    let (store, agent, mut request, _) = queued().await;
    let claimed = store
        .claim_next_command(claim_request(&agent, "old-worker"))
        .await
        .unwrap()
        .unwrap();
    let claim = claimed.work.lease.as_ref().unwrap().claim.clone();
    request.target.expected_command_revision = claimed.revision;
    let next_fence = store.lock().agent_command_orders[&agent.id].next_fence;
    let result = applied(store.cancel_queued_response(request).await.unwrap());
    assert_eq!(result.response.status, AgentResponseStatus::Cancelled);
    assert_eq!(store.lock().agent_command_orders[&agent.id].lease, None);
    assert_eq!(
        store.lock().agent_command_orders[&agent.id].next_fence,
        next_fence
    );
    assert_eq!(
        store
            .release_command_claim(&claim, crate::now_rfc3339())
            .await
            .unwrap_err(),
        CommandLedgerError::StaleClaim
    );
    assert!(store
        .claim_next_command(claim_request(&agent, "next-worker"))
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        store
            .reconcile_command(&result.binding.target.source_command_id)
            .await
            .unwrap(),
        result.response
    );
}

#[tokio::test]
async fn queued_cancellation_preserves_an_unrelated_lane_and_its_order() {
    let (store, agent, request, _) = queued().await;
    let mut other = request.command.record(2, 0, Some(17)).unwrap();
    other.receipt.envelope.command_id = crate::ids::AgentCommandId::generate();
    let claim = {
        let mut state = store.lock();
        let lane = state.agent_command_orders.get_mut(&agent.id).unwrap();
        lane.last_order = 2;
        assert!(lane
            .claim(&mut other, &claim_request(&agent, "unrelated-worker"))
            .unwrap());
        let claim = lane.lease.clone();
        state
            .agent_commands
            .insert(other.receipt.envelope.command_id.clone(), other.clone());
        claim
    };
    applied(store.cancel_queued_response(request).await.unwrap());
    let state = store.lock();
    assert_eq!(state.agent_command_orders[&agent.id].lease, claim);
    assert_eq!(state.agent_command_orders[&agent.id].last_order, 3);
    assert_eq!(
        state.agent_commands[&other.receipt.envelope.command_id],
        other
    );
}

#[tokio::test]
async fn queued_cancellation_concurrent_keys_keep_one_effect_and_recover_exact_receipt() {
    let (store, _, request, _) = queued().await;
    let mut same = request.clone();
    same.command.envelope.command_id = crate::ids::AgentCommandId::generate();
    let (one, two) = tokio::join!(
        store.cancel_queued_response(request.clone()),
        store.cancel_queued_response(same)
    );
    assert_eq!(one.unwrap(), two.unwrap());
    let mut different = request.clone();
    different.command.key_sha256 = command_key_digest("other-cancellation").unwrap();
    assert_eq!(
        store.cancel_queued_response(different).await.unwrap(),
        QueuedCancellationOutcome::NotAdmitted
    );
    for change in 0..3 {
        let mut changed = request.clone();
        match change {
            0 => changed.target.source_command_id = crate::ids::AgentCommandId::generate(),
            1 => changed.target.response_id = crate::ids::AgentResponseId::generate(),
            _ => changed.target.expected_command_revision += 1,
        }
        assert_eq!(
            store.cancel_queued_response(changed).await.unwrap_err(),
            CommandLedgerError::IdempotencyConflict
        );
    }
    assert_eq!(store.lock().queued_cancellations.len(), 1);
}

#[tokio::test]
async fn queued_cancellation_human_authority_can_settle_unbound_channel_hold() {
    let (store, _, request, mut source) = queued().await;
    source.status = AgentResponseStatus::WaitingAction;
    source.receipt.envelope.actor = AgentActor::Channel {
        bridge_id: "bridge-original".into(),
        grant_id: "grant-original".into(),
        platform: "telegram".into(),
        chat_id: "chat-original".into(),
        sender_id: "sender-original".into(),
    };
    source.receipt.envelope.source.kind = AgentSourceKind::Channel;
    source.validate().unwrap();
    store
        .lock()
        .agent_commands
        .insert(source.receipt.envelope.command_id.clone(), source.clone());
    assert!(queued_cancellation_eligible(&source));
    let result = applied(store.cancel_queued_response(request).await.unwrap());
    assert_eq!(result.response.receipt, source.receipt);
    assert_eq!(result.response.status, AgentResponseStatus::Cancelled);
    assert!(matches!(
        result.record.receipt.envelope.actor,
        AgentActor::Human { .. }
    ));
}
