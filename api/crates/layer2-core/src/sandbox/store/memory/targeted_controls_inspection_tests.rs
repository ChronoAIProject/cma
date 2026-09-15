use super::*;

fn advance(store: &InMemorySandboxStore, agent: &Agent, request: &mut TargetedControlReservation) {
    let mut state = store.lock();
    let current = state.agents.get_mut(&agent.id).unwrap();
    current.revision += 1;
    current.conversation.active_turn = Some(40);
    request.observed_agent_revision = current.revision;
}

#[tokio::test]
async fn absent_stale_control_is_proven_without_ledger_changes_and_cannot_reserve_later() {
    let (store, agent, mut request, source) = fixture().await;
    let old = request.clone();
    advance(&store, &agent, &mut request);
    store
        .lock()
        .agent_commands
        .get_mut(&source.receipt.envelope.command_id)
        .unwrap()
        .status = AgentResponseStatus::Completed;
    let before = counts(&store);
    assert_eq!(
        store
            .inspect_targeted_control(request.clone())
            .await
            .unwrap(),
        TargetedControlInspection::NotAdmitted
    );
    assert_eq!(
        store
            .lookup_targeted_control(request.clone())
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        store.reserve_targeted_control(old).await.unwrap_err(),
        CommandLedgerError::Conflict
    );
    assert!(store
        .reserve_targeted_control(request.clone())
        .await
        .is_err());
    advance(&store, &agent, &mut request);
    assert_eq!(
        store.inspect_targeted_control(request).await.unwrap(),
        TargetedControlInspection::NotAdmitted
    );
    assert_eq!(counts(&store), before);
}

#[tokio::test]
async fn inspection_recovers_existing_receipt_before_proving_staleness() {
    let (store, agent, mut request, _) = fixture().await;
    let existing = store
        .reserve_targeted_control(request.clone())
        .await
        .unwrap();
    advance(&store, &agent, &mut request);
    assert_eq!(
        store
            .inspect_targeted_control(request.clone())
            .await
            .unwrap(),
        TargetedControlInspection::Existing(Box::new(existing))
    );
    request.command.expected_agent_revision += 1;
    assert_eq!(
        store.inspect_targeted_control(request).await.unwrap_err(),
        CommandLedgerError::IdempotencyConflict
    );
}

#[tokio::test]
async fn equal_and_future_revisions_cannot_prove_refusal() {
    let (store, _, mut request, _) = fixture().await;
    for _ in 0..2 {
        assert_eq!(
            store
                .inspect_targeted_control(request.clone())
                .await
                .unwrap(),
            TargetedControlInspection::Missing
        );
        request.command.expected_agent_revision += 1;
    }
}

#[tokio::test]
async fn stale_inspection_requires_current_authority_and_original_source_incarnation() {
    for change in 0..8 {
        let (store, agent, mut request, source) = fixture().await;
        advance(&store, &agent, &mut request);
        let expected = match change {
            0 => {
                request.observed_agent_revision -= 1;
                CommandLedgerError::Conflict
            }
            1 => {
                request.command.authority_deadline = Some(tokio::time::Instant::now());
                CommandLedgerError::Conflict
            }
            2 => {
                request.command.parent.expected_revision += 1;
                CommandLedgerError::Conflict
            }
            3 => {
                store
                    .lock()
                    .agent_commands
                    .remove(&source.receipt.envelope.command_id);
                CommandLedgerError::NotFound
            }
            4 => {
                request.target.turn_sequence += 1;
                CommandLedgerError::NotAdmissible
            }
            5 => {
                store
                    .lock()
                    .agent_commands
                    .get_mut(&source.receipt.envelope.command_id)
                    .unwrap()
                    .work
                    .dispatch = None;
                CommandLedgerError::NotAdmissible
            }
            6 => {
                request.agent_created_at = "2026-09-05T00:00:01Z".into();
                store.lock().agents.get_mut(&agent.id).unwrap().created_at =
                    request.agent_created_at.clone();
                CommandLedgerError::NotFound
            }
            _ => {
                request.command.policy.policy_revision += 1;
                CommandLedgerError::Conflict
            }
        };
        let before = counts(&store);
        assert_eq!(
            store.inspect_targeted_control(request).await.unwrap_err(),
            expected,
            "change {change}"
        );
        assert_eq!(counts(&store), before);
    }
}
