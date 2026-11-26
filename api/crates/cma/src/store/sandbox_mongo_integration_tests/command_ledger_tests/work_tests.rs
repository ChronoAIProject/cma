use std::time::{Duration, UNIX_EPOCH};

use layer2_core::sandbox::{AgentOperationDelivery, AgentOperationIntent, AgentOperationRecord};

use super::*;
use crate::store::agent_conversation_store::{reconcile_conversation, ConversationReceipt};
use layer2_core::sandbox::AgentUpdateRecorder;

fn after(at: &str, seconds: u64) -> String {
    layer2_core::rfc3339_at(
        UNIX_EPOCH + Duration::from_secs(layer2_core::seconds_from_rfc3339(at).unwrap() + seconds),
    )
}

pub(super) fn claim_request(agent: &Agent, holder: &str, now: &str) -> CommandClaimRequest {
    CommandClaimRequest {
        agent_id: agent.id.clone(),
        holder: holder.into(),
        now: now.into(),
        expires_at: after(now, 30),
    }
}

pub(in crate::store) fn live_request(
    agent: &Agent,
    parent: &Sandbox,
    key: &str,
) -> CommandReservation {
    let mut value = request(agent, parent, key);
    value.admitted_at = layer2_core::now_rfc3339();
    value.queue_deadline = after(&value.admitted_at, 300);
    value
}

pub(super) fn message_mutation(
    agent: &Agent,
    parent: &Sandbox,
    claim: &CommandClaim,
) -> AgentConversationMutation {
    let now = layer2_core::now_rfc3339();
    let mut next = agent.clone();
    let sequence = agent.conversation.last_seq + 1;
    let turn = sequence + 1;
    next.transition(AgentStatus::Working, now.clone(), None)
        .unwrap();
    next.conversation.active_turn = Some(turn);
    next.conversation.operation = Some(
        AgentOperationRecord::new(
            sequence,
            1,
            turn,
            AgentOperationIntent::send_message("Inspect this."),
            now.clone(),
        )
        .unwrap(),
    );
    next.conversation.last_seq += 4;
    AgentConversationMutation {
        settings_fence: None,
        command_claim: Some(claim.clone()),
        execution_fence: None,
        trigger_execution_fence: None,
        admission_guard: Some(SandboxMutationGuard::capture(parent)),
        expected_agent_revision: agent.revision,
        expected_last_seq: agent.conversation.last_seq,
        runtime_generation: 1,
        agent: next,
        appended_at: now,
        agent_logs: vec![
            AgentLogKind::OperationDeliveryChanged {
                operation_sequence: sequence,
                operation: layer2_core::sandbox::AgentOperationKind::SendMessage,
                state: AgentOperationDelivery::DispatchFenced,
            },
            AgentLogKind::TurnStarted {
                model: Some(agent.settings().model),
                turn_sequence: turn,
                origin_key: Some(format!("command:{}", claim.command_id.as_str())),
            },
            AgentLogKind::OwnerMessage {
                content: "Inspect this.".into(),
            },
            AgentLogKind::StatusChanged {
                from: AgentStatus::Ready,
                to: AgentStatus::Working,
            },
        ],
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn competing_claims_take_one_lane_and_stale_workers_cannot_renew_release_or_refuse() {
    let (client, name) = database("command_claim").await;
    let (one, agent, parent) = fixture(&name, &client).await;
    let two = MongoSandboxStore::open(&client, &name).await.unwrap();
    one.reserve_command(live_request(&agent, &parent, "one"))
        .await
        .unwrap();
    one.reserve_command(live_request(&agent, &parent, "two"))
        .await
        .unwrap();
    let now = layer2_core::now_rfc3339();
    let (a, b) = tokio::join!(
        one.claim_next_command(claim_request(&agent, "one", &now)),
        two.claim_next_command(claim_request(&agent, "two", &now))
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_ne!(a.is_some(), b.is_some());
    let claimed = a.or(b).unwrap();
    assert_eq!(claimed.receipt.agent_order, 1);
    let old = &claimed.work.lease.as_ref().unwrap().claim;
    let takeover_time = after(&now, 30);
    let replacement = two
        .claim_next_command(claim_request(&agent, "replacement", &takeover_time))
        .await
        .unwrap()
        .unwrap();
    let claim = &replacement.work.lease.as_ref().unwrap().claim;
    assert_eq!(old.command_id, claim.command_id);
    assert!(claim.fence > old.fence);
    assert_eq!(
        one.renew_command_claim(old, takeover_time.clone(), after(&takeover_time, 30))
            .await,
        Err(CommandLedgerError::StaleClaim)
    );
    assert_eq!(
        one.release_command_claim(old, takeover_time.clone()).await,
        Err(CommandLedgerError::StaleClaim)
    );
    assert_eq!(
        one.reject_command(
            old,
            CommandFailureCode::AuthorityRevoked,
            takeover_time.clone()
        )
        .await,
        Err(CommandLedgerError::StaleClaim)
    );
    two.renew_command_claim(claim, after(&takeover_time, 1), after(&takeover_time, 40))
        .await
        .unwrap();
    assert_eq!(
        two.renew_command_claim(claim, after(&takeover_time, 1), after(&takeover_time, 39))
            .await,
        Err(CommandLedgerError::Invalid)
    );
    assert_eq!(
        two.reject_command(
            claim,
            CommandFailureCode::QueueTimeout,
            takeover_time.clone()
        )
        .await,
        Err(CommandLedgerError::Invalid)
    );
    let refused = two
        .reject_command(
            claim,
            CommandFailureCode::AuthorityRevoked,
            takeover_time.clone(),
        )
        .await
        .unwrap();
    assert_eq!(refused.status, AgentResponseStatus::Failed);
    assert_eq!(refused.receipt, claimed.receipt);
    let next = one
        .claim_next_command(claim_request(&agent, "next", &takeover_time))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next.receipt.agent_order, 2);
    assert!(next.work.lease.unwrap().claim.fence > claim.fence);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn dispatch_transaction_rolls_back_invalid_input_and_binds_one_exact_commit() {
    let (client, name) = database("command_bind").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    let reservation = live_request(&agent, &parent, "one");
    let admitted = record(store.reserve_command(reservation.clone()).await.unwrap());
    let claimed = store
        .claim_next_command(claim_request(&agent, "one", &layer2_core::now_rfc3339()))
        .await
        .unwrap()
        .unwrap();
    let claim = &claimed.work.lease.as_ref().unwrap().claim;
    let valid = message_mutation(&agent, &parent, claim);
    let mut invalid = valid.clone();
    invalid.agent_logs[2] = AgentLogKind::OwnerMessage {
        content: "different payload".into(),
    };
    let before_parent = store
        .sandboxes
        .find_one(doc! { "_id": parent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        store.commit_agent_conversation(invalid).await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap(),
        Some(agent.clone())
    );
    assert_eq!(
        store.get_command(&claim.command_id).await.unwrap(),
        Some(claimed.clone())
    );
    assert_eq!(
        store
            .sandboxes
            .find_one(doc! { "_id": parent.id.as_str() })
            .await
            .unwrap()
            .unwrap(),
        before_parent
    );
    assert_eq!(store.agent_log(&agent.id, 0, 20).await.unwrap().len(), 1);
    let replica = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (a, b) = tokio::join!(
        store.commit_agent_conversation(valid.clone()),
        replica.commit_agent_conversation(valid.clone())
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let logs = a.or(b).unwrap();
    let bound = store.get_command(&claim.command_id).await.unwrap().unwrap();
    assert_eq!(bound.receipt, admitted.receipt);
    assert_eq!(bound.status, AgentResponseStatus::InProgress);
    assert_eq!(
        record(replica.reserve_command(reservation).await.unwrap()),
        bound
    );
    let dispatch = bound.work.dispatch.clone().unwrap();
    assert_eq!(dispatch.claim, *claim);
    assert_eq!(
        dispatch.operation_sequence,
        valid
            .agent
            .conversation
            .operation
            .as_ref()
            .unwrap()
            .sequence
    );
    let receipt = ConversationReceipt {
        segmented: None,
        agent: valid.agent,
        logs,
        command_dispatch: Some(dispatch),
    };
    assert!(reconcile_conversation(&store, &receipt).await.unwrap());
    let mut wrong = receipt.clone();
    wrong.command_dispatch.as_mut().unwrap().claim.fence += 1;
    assert!(!reconcile_conversation(&store, &wrong).await.unwrap());
    wrong.command_dispatch.as_mut().unwrap().claim.command_id = AgentCommandId::generate();
    assert!(!reconcile_conversation(&store, &wrong).await.unwrap());
    store
        .release_command_claim(claim, layer2_core::now_rfc3339())
        .await
        .unwrap();
    assert!(replica
        .claim_next_command(claim_request(
            &agent,
            "replacement",
            &after(&layer2_core::now_rfc3339(), 30)
        ))
        .await
        .unwrap()
        .is_none());
    assert_eq!(store.agent_log(&agent.id, 0, 20).await.unwrap().len(), 5);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn identical_control_logs_do_not_prove_a_different_command_committed() {
    let (client, name) = database("command_control_proof").await;
    let (store, mut agent, parent) = fixture(&name, &client).await;
    agent.capabilities = layer2_core::RuntimeCapabilities::new([
        layer2_core::RuntimeCapability::Conversation,
        layer2_core::RuntimeCapability::Interrupt,
    ]);
    replace_agent_fixture(&store, &agent).await;
    let turn = begin_stored_conversation_turn(&store, agent).await;
    let working = turn.agent.clone();
    store.commit_agent_conversation(turn).await.unwrap();
    let mut first = live_request(&working, &parent, "one");
    first.envelope.command = AgentCommand::Interrupt;
    first.envelope.request_fingerprint = first.envelope.command.fingerprint().unwrap();
    first.response_id = None;
    let mut second = first.clone();
    second.envelope.command_id = AgentCommandId::generate();
    second.key_sha256 = command_key_digest("two").unwrap();
    let other = record(store.reserve_command(second).await.unwrap());
    store.reserve_command(first).await.unwrap();
    let claimed = store
        .claim_next_command(claim_request(
            &working,
            "worker",
            &layer2_core::now_rfc3339(),
        ))
        .await
        .unwrap()
        .unwrap();
    let claim = &claimed.work.lease.as_ref().unwrap().claim;
    // The first queued control is selected regardless of its external name.
    assert_eq!(claimed.receipt, other.receipt);
    let now = layer2_core::now_rfc3339();
    let sequence = working.conversation.last_seq + 1;
    let mut next = working.clone();
    next.revision += 1;
    next.updated_at = now.clone();
    next.conversation.last_seq += 1;
    next.conversation.operation = Some(
        AgentOperationRecord::new(
            sequence,
            1,
            next.conversation.active_turn.unwrap(),
            AgentOperationIntent::Interrupt,
            now.clone(),
        )
        .unwrap(),
    );
    let mutation = AgentConversationMutation {
        settings_fence: None,
        command_claim: Some(claim.clone()),
        execution_fence: None,
        trigger_execution_fence: None,
        admission_guard: Some(SandboxMutationGuard::capture(&parent)),
        expected_agent_revision: working.revision,
        expected_last_seq: working.conversation.last_seq,
        runtime_generation: 1,
        agent: next.clone(),
        appended_at: now,
        agent_logs: vec![AgentLogKind::OperationDeliveryChanged {
            operation_sequence: sequence,
            operation: layer2_core::sandbox::AgentOperationKind::Interrupt,
            state: AgentOperationDelivery::DispatchFenced,
        }],
    };
    let logs = store.commit_agent_conversation(mutation).await.unwrap();
    let bound = store.get_command(&claim.command_id).await.unwrap().unwrap();
    let receipt = ConversationReceipt {
        segmented: None,
        agent: next,
        logs,
        command_dispatch: bound.work.dispatch,
    };
    assert!(reconcile_conversation(&store, &receipt).await.unwrap());
    let mut wrong = receipt;
    let mut commands = store
        .agent_commands
        .find(doc! { "status": "queued" })
        .await
        .unwrap();
    let unbound = commands.try_next().await.unwrap().unwrap();
    wrong.command_dispatch.as_mut().unwrap().claim.command_id =
        AgentCommandId(unbound.get_str("_id").unwrap().into());
    assert!(!reconcile_conversation(&store, &wrong).await.unwrap());
    client.database(&name).drop().await.unwrap();
}

mod maintenance_reconcile_tests;
mod settlement;
