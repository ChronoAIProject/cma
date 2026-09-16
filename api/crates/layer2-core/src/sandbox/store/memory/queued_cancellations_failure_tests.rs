use super::*;

#[tokio::test]
async fn queued_cancellation_never_changes_dispatched_or_unknown_runtime_work() {
    for delivery in [
        crate::sandbox::AgentOperationDelivery::DispatchFenced,
        crate::sandbox::AgentOperationDelivery::RecoveryRequired,
        crate::sandbox::AgentOperationDelivery::PreWriteFailed,
    ] {
        let (store, _, control, mut source) = fixtures::fixture().await;
        source.work.dispatch.as_mut().unwrap().delivery = delivery;
        store
            .lock()
            .agent_commands
            .insert(source.receipt.envelope.command_id.clone(), source.clone());
        let request = QueuedCancellationReservation {
            command: control.command,
            target: QueuedCancellationTarget {
                response_id: source.receipt.response_id.clone().unwrap(),
                source_command_id: source.receipt.envelope.command_id.clone(),
                expected_command_revision: source.revision,
            },
            observed_agent_revision: control.observed_agent_revision,
            agent_created_at: control.agent_created_at,
        };
        assert!(!queued_cancellation_eligible(&source));
        assert_eq!(
            store.cancel_queued_response(request).await.unwrap_err(),
            CommandLedgerError::Conflict
        );
        assert_eq!(
            store.lock().agent_commands[&source.receipt.envelope.command_id],
            source
        );
        assert!(store.lock().queued_cancellations.is_empty());
    }
}

#[tokio::test]
async fn queued_cancellation_refusal_proof_requires_current_authority_and_exact_incarnation() {
    let (store, agent, mut request, _) = queued().await;
    let claimed = store
        .claim_next_command(claim_request(&agent, "worker"))
        .await
        .unwrap()
        .unwrap();
    assert!(request.target.expected_command_revision < claimed.revision);
    let mut expired = request.clone();
    expired.command.authority_deadline = Some(tokio::time::Instant::now());
    assert_eq!(
        store.cancel_queued_response(expired).await.unwrap_err(),
        CommandLedgerError::Conflict
    );
    let mut wrong = request.clone();
    wrong.target.response_id = crate::ids::AgentResponseId::generate();
    assert_eq!(
        store.cancel_queued_response(wrong).await.unwrap_err(),
        CommandLedgerError::NotFound
    );
    assert_eq!(
        store.cancel_queued_response(request.clone()).await.unwrap(),
        QueuedCancellationOutcome::NotAdmitted
    );
    assert_eq!(
        store
            .get_command(&claimed.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(claimed)
    );
    let mut recreated = agent.clone();
    recreated.created_at = "2026-09-06T00:00:00Z".into();
    store.lock().agents.insert(agent.id, recreated.clone());
    assert_eq!(
        store
            .cancel_queued_response(request.clone())
            .await
            .unwrap_err(),
        CommandLedgerError::NotFound
    );
    request.agent_created_at = recreated.created_at;
    assert_eq!(
        store.cancel_queued_response(request).await.unwrap_err(),
        CommandLedgerError::NotFound
    );
}

#[tokio::test]
async fn queued_cancellation_does_not_backfill_a_legacy_key_or_claim_future_revision() {
    let (store, agent, request, _) = queued().await;
    let mut future = request.clone();
    future.target.expected_command_revision += 1;
    assert_eq!(
        store.cancel_queued_response(future).await.unwrap_err(),
        CommandLedgerError::Conflict
    );
    let mut legacy = request.command.record(2, 0, Some(17)).unwrap();
    legacy.status = AgentResponseStatus::Completed;
    {
        let mut state = store.lock();
        state.agent_command_keys.insert(
            (legacy.scope(), legacy.key_sha256.clone()),
            legacy.receipt.envelope.command_id.clone(),
        );
        state
            .agent_commands
            .insert(legacy.receipt.envelope.command_id.clone(), legacy);
        state
            .agent_command_orders
            .get_mut(&agent.id)
            .unwrap()
            .last_order = 2;
    }
    assert_eq!(
        store.cancel_queued_response(request).await.unwrap_err(),
        CommandLedgerError::IdempotencyConflict
    );
    assert!(store.lock().queued_cancellations.is_empty());
}
