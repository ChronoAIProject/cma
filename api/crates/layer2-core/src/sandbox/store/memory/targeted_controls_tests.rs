use crate::sandbox::agent_io::*;
use crate::sandbox::store::InMemorySandboxStore;
use crate::sandbox::{Agent, AgentApprovalDecision};

#[path = "targeted_controls_failure_tests.rs"]
mod failures;
#[path = "targeted_controls_fixtures.rs"]
mod fixtures;
use fixtures::*;

#[tokio::test]
async fn explicit_replay_survives_completion_new_turn_and_full_queue() {
    let (store, agent, mut request, response) = fixture().await;
    let original = store
        .reserve_targeted_control(request.clone())
        .await
        .unwrap();
    assert_eq!(original.record.target_turn, Some(2));
    assert_eq!(original.record.receipt.response_id, None);
    {
        let mut state = store.lock();
        let current = state.agents.get_mut(&agent.id).unwrap();
        current.revision += 1;
        current.conversation.active_turn = Some(40);
        let source = state
            .agent_commands
            .get_mut(&response.receipt.envelope.command_id)
            .unwrap();
        source.status = AgentResponseStatus::Completed;
        source.revision += 1;
    }
    let before = counts(&store);
    request.observed_agent_revision += 1;
    request.command.queue_capacity = 1;
    assert_eq!(
        store
            .reserve_targeted_control(request.clone())
            .await
            .unwrap(),
        original
    );
    assert_eq!(
        store
            .lookup_targeted_control(request.clone())
            .await
            .unwrap(),
        Some(original.clone())
    );
    assert_eq!(counts(&store), before);
    assert_eq!(
        store.lock().agents[&agent.id].conversation.active_turn,
        Some(40)
    );
    for change in 0..3 {
        let mut changed = request.clone();
        match change {
            0 => changed.target.response_id = crate::ids::AgentResponseId::generate(),
            1 => changed.target.turn_sequence += 1,
            _ => changed.command.expected_agent_revision += 1,
        }
        assert_eq!(
            store.reserve_targeted_control(changed).await.unwrap_err(),
            CommandLedgerError::IdempotencyConflict
        );
    }
    assert_eq!(counts(&store), before);
}

#[tokio::test]
async fn concurrent_identical_controls_keep_one_receipt_and_sidecar() {
    let (store, _, request, _) = fixture().await;
    let mut retry = request.clone();
    retry.command.envelope.command_id = crate::ids::AgentCommandId::generate();
    retry.command.envelope.authorization.policy_revision += 1;
    let (a, b) = tokio::join!(
        store.reserve_targeted_control(request),
        store.reserve_targeted_control(retry)
    );
    assert_eq!(a.unwrap(), b.unwrap());
    assert_eq!(counts(&store), (2, 1, 2, 0, vec![2]));
}

#[tokio::test]
async fn legacy_same_key_is_not_backfilled_and_ordinary_v1_replay_is_preserved() {
    let (store, _, request, _) = fixture().await;
    let original = store
        .reserve_command(request.command.clone())
        .await
        .unwrap();
    let before = counts(&store);
    assert_eq!(
        store
            .reserve_targeted_control(request.clone())
            .await
            .unwrap_err(),
        CommandLedgerError::IdempotencyConflict
    );
    assert_eq!(
        store
            .lookup_targeted_control(request.clone())
            .await
            .unwrap_err(),
        CommandLedgerError::IdempotencyConflict
    );
    let CommandReservationResult::Created(original) = original else {
        panic!("new legacy command")
    };
    assert_eq!(
        store.reserve_command(request.command).await.unwrap(),
        CommandReservationResult::Existing(original)
    );
    assert_eq!(counts(&store), before);
}

#[tokio::test]
async fn changed_steer_body_or_approval_decision_conflicts_with_permanent_key() {
    for command in [
        AgentCommand::Steer {
            input: AgentInput::text("first"),
        },
        AgentCommand::AnswerApproval {
            approval_id: "permission".into(),
            decision: AgentApprovalDecision::Allow,
        },
    ] {
        let (store, _, mut request, response) = fixture().await;
        set_command(&mut request, command.clone());
        // Insert the completed durable receipt to exercise replay independent
        // of whether this old approval is still pending in the current log.
        let mut record = request.command.record(2, 0, Some(2)).unwrap();
        record.status = AgentResponseStatus::Completed;
        let admitted = request.bind(record, &response).unwrap();
        {
            let mut state = store.lock();
            let id = admitted.binding.command_id.clone();
            state.agent_command_keys.insert(
                (request.command.scope(), request.command.key_sha256.clone()),
                id.clone(),
            );
            state
                .agent_commands
                .insert(id.clone(), admitted.record.clone());
            state.targeted_controls.insert(id, admitted.binding.clone());
        }
        assert_eq!(
            store
                .reserve_targeted_control(request.clone())
                .await
                .unwrap(),
            admitted
        );
        let changed = match command {
            AgentCommand::Steer { .. } => AgentCommand::Steer {
                input: AgentInput::text("second"),
            },
            _ => AgentCommand::AnswerApproval {
                approval_id: "permission".into(),
                decision: AgentApprovalDecision::Deny,
            },
        };
        set_command(&mut request, changed);
        assert_eq!(
            store.reserve_targeted_control(request).await.unwrap_err(),
            CommandLedgerError::IdempotencyConflict
        );
    }
}

#[tokio::test]
async fn sidecar_survives_ordinary_worker_refusal_without_retargeting() {
    let (store, agent, request, _) = fixture().await;
    let original = store
        .reserve_targeted_control(request.clone())
        .await
        .unwrap();
    let claimed = store
        .claim_next_command(CommandClaimRequest {
            agent_id: agent.id,
            holder: "old-worker".into(),
            now: NOW.into(),
            expires_at: "2026-09-05T00:00:30Z".into(),
        })
        .await
        .unwrap()
        .unwrap();
    store
        .reject_command(
            &claimed.work.lease.unwrap().claim,
            CommandFailureCode::TargetTurnChanged,
            NOW.into(),
        )
        .await
        .unwrap();
    let recovered = store
        .lookup_targeted_control(request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.binding, original.binding);
    assert_eq!(recovered.record.target_turn, Some(2));
    assert_eq!(recovered.record.status, AgentResponseStatus::Failed);
    assert_eq!(recovered.record.receipt, original.record.receipt);
}
