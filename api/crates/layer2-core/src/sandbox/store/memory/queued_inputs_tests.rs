use super::queued_cancellations_tests::{claim_request, queued};
use crate::sandbox::agent_io::*;
fn applied(outcome: QueuedInputOutcome) -> AdmittedQueuedInput {
    match outcome {
        QueuedInputOutcome::Applied(value) => *value,
        other => panic!("expected edit: {other:?}"),
    }
}
#[tokio::test]
async fn queued_input_preserves_admission_order_and_recovers_each_immutable_edit() {
    let (store, agent, cancellation, source) = queued().await;
    let request = QueuedInputReservation {
        cancellation,
        text: "first edit\n<attachment>changed file</attachment>".into(),
    };
    let first = applied(store.edit_queued_input(request.clone()).await.unwrap());
    assert_eq!(first.response.receipt, source.receipt);
    assert_eq!(first.response.status, AgentResponseStatus::Queued);
    assert_eq!(first.response.revision, source.revision + 1);
    assert_eq!(
        first.response.effective_command().unwrap(),
        AgentCommand::message(&request.text)
    );
    let mut next = request.clone();
    next.text = "second edit".into();
    next.cancellation.target.expected_command_revision = first.response.revision;
    next.cancellation.command.key_sha256 = command_key_digest("second-edit").unwrap();
    next.cancellation.command.envelope.command_id = crate::ids::AgentCommandId::generate();
    let second = applied(store.edit_queued_input(next).await.unwrap());
    let replay = applied(store.edit_queued_input(request.clone()).await.unwrap());
    assert_eq!(replay.record, first.record);
    assert_eq!(replay.binding, first.binding);
    assert_eq!(replay.response, second.response);
    assert_eq!(
        store.list_queued_inputs(&agent.id).await.unwrap(),
        vec![second.response]
    );
    let mut changed = request;
    changed.text = "reuse key with changed bytes".into();
    assert_eq!(
        store.edit_queued_input(changed).await.unwrap_err(),
        CommandLedgerError::IdempotencyConflict
    );
}
#[tokio::test]
async fn queued_input_edit_invalidates_old_claim_and_next_claim_reads_edited_payload() {
    let (store, agent, mut cancellation, _) = queued().await;
    let old = store
        .claim_next_command(claim_request(&agent, "old"))
        .await
        .unwrap()
        .unwrap();
    let claim = old.work.lease.as_ref().unwrap().claim.clone();
    cancellation.target.expected_command_revision = old.revision;
    let request = QueuedInputReservation {
        cancellation,
        text: "new payload".into(),
    };
    let edited = applied(store.edit_queued_input(request).await.unwrap());
    assert_eq!(
        store
            .release_command_claim(&claim, crate::now_rfc3339())
            .await
            .unwrap_err(),
        CommandLedgerError::StaleClaim
    );
    let new = store
        .claim_next_command(claim_request(&agent, "new"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(new.receipt, old.receipt);
    assert_eq!(
        new.effective_command().unwrap(),
        AgentCommand::message("new payload")
    );
    assert!(new.work.lease.as_ref().unwrap().claim.fence > claim.fence);
    assert!(new.revision > edited.response.revision);
}
#[tokio::test]
async fn queued_input_edit_and_cancel_have_one_effect_and_no_dispatch_lease() {
    let (store, agent, cancellation, source) = queued().await;
    let edit = QueuedInputReservation {
        cancellation: cancellation.clone(),
        text: "edit".into(),
    };
    let mut cancel = cancellation;
    cancel.command.key_sha256 = command_key_digest("separate-cancel").unwrap();
    cancel.command.envelope.command_id = crate::ids::AgentCommandId::generate();
    let (one, two) = tokio::join!(
        store.edit_queued_input(edit),
        store.cancel_queued_response(cancel)
    );
    assert!(matches!(
        (&one, &two),
        (
            Ok(QueuedInputOutcome::Applied(_)),
            Ok(QueuedCancellationOutcome::NotAdmitted)
        ) | (
            Ok(QueuedInputOutcome::NotAdmitted),
            Ok(QueuedCancellationOutcome::Applied(_))
        )
    ));
    let current = store
        .get_command(&source.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.revision, source.revision + 1);
    assert!(current.work.lease.is_none());
    assert!(store.lock().agent_command_orders[&agent.id].lease.is_none());
}
#[tokio::test]
async fn queued_input_rejects_started_dispatch_unknown_delivery_and_stale_revision() {
    let (store, agent, cancellation, source) = queued().await;
    let request = QueuedInputReservation {
        cancellation,
        text: "edit".into(),
    };
    store
        .claim_next_command(claim_request(&agent, "worker"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        store.edit_queued_input(request).await.unwrap(),
        QueuedInputOutcome::NotAdmitted
    );
    assert_eq!(source.queued_input, None);
    for delivery in [
        crate::sandbox::AgentOperationDelivery::DispatchFenced,
        crate::sandbox::AgentOperationDelivery::RecoveryRequired,
        crate::sandbox::AgentOperationDelivery::PreWriteFailed,
    ] {
        let (store, _, control, mut source) =
            super::targeted_controls_tests::fixtures::fixture().await;
        source.work.dispatch.as_mut().unwrap().delivery = delivery;
        store
            .lock()
            .agent_commands
            .insert(source.receipt.envelope.command_id.clone(), source.clone());
        let request = QueuedInputReservation {
            text: "edit".into(),
            cancellation: QueuedCancellationReservation {
                command: control.command,
                target: QueuedCancellationTarget {
                    response_id: source.receipt.response_id.clone().unwrap(),
                    source_command_id: source.receipt.envelope.command_id.clone(),
                    expected_command_revision: source.revision,
                },
                observed_agent_revision: control.observed_agent_revision,
                agent_created_at: control.agent_created_at,
            },
        };
        assert_eq!(
            store.edit_queued_input(request).await.unwrap_err(),
            CommandLedgerError::Conflict
        );
        assert_eq!(
            store
                .get_command(&source.receipt.envelope.command_id)
                .await
                .unwrap(),
            Some(source)
        );
    }
}
