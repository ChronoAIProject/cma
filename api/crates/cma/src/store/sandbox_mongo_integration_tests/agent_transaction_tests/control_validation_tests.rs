use super::*;

#[test]
fn mongo_agent_control_plans_exact_replays_and_conflicts() {
    let current = agent_for_worker("agt_control_rules", NOW);
    let stop = AgentStopRequest {
        target: AgentControlTarget::from(&current),
        reason: AgentStopReason::Owner,
        requested_at: NOW.into(),
    };
    let scheduled = match plan_stop(current, &stop).unwrap() {
        AgentControlResult::Scheduled(agent) => agent,
        AgentControlResult::Existing(_) => panic!("new stop must be scheduled"),
    };
    assert_eq!(
        plan_stop(scheduled.clone(), &stop).unwrap(),
        AgentControlResult::Existing(scheduled.clone())
    );
    let mut changed = stop.clone();
    changed.reason = AgentStopReason::Operator;
    assert_eq!(
        plan_stop(scheduled.clone(), &changed),
        Err(AgentControlStoreError::StopReasonConflict)
    );

    let mut stopped = scheduled.clone();
    stopped.work.schedule(AgentWorkStep::None, MINUTE.into());
    stopped
        .transition(
            AgentStatus::Stopped,
            MINUTE.into(),
            Some(AgentStopReason::Owner),
        )
        .unwrap();
    let stopped_request = AgentStopRequest {
        target: AgentControlTarget::from(&stopped),
        reason: AgentStopReason::Owner,
        requested_at: MINUTE.into(),
    };
    assert_eq!(
        plan_stop(stopped.clone(), &stopped_request),
        Err(AgentControlStoreError::StatusConflict {
            status: AgentStatus::Stopped,
        })
    );

    let delete = AgentDeleteRequest {
        target: AgentControlTarget::from(&stopped),
        requested_at: MINUTE.into(),
    };
    let deleting = match plan_delete(stopped, &delete).unwrap() {
        AgentControlResult::Scheduled(agent) => agent,
        AgentControlResult::Existing(_) => panic!("new delete must be scheduled"),
    };
    assert_eq!(
        plan_delete(deleting.clone(), &delete).unwrap(),
        AgentControlResult::Existing(deleting)
    );
}

#[test]
fn agent_control_receipts_bind_the_exact_invocation_and_request() {
    let current = agent_for_worker("agt_control_receipt", NOW);
    let request = AgentStopRequest {
        target: AgentControlTarget::from(&current),
        reason: AgentStopReason::Owner,
        requested_at: NOW.into(),
    };
    let operation = ControlOperation::new(ControlRequest::Stop(request.clone()));
    let scheduled = operation.request.plan(current).unwrap().into_agent();
    let receipt = operation.receipt_update(&scheduled).unwrap();
    let stored = read_control_receipts(&receipt, &scheduled).unwrap();
    assert!(operation.was_recorded(&stored, &scheduled));

    let later = ControlOperation::new(ControlRequest::Stop(request));
    let later_receipt = later.receipt_update(&scheduled).unwrap();
    let stored_later = read_control_receipts(&later_receipt, &scheduled).unwrap();
    assert!(!operation.was_recorded(&stored_later, &scheduled));
    assert!(later.was_recorded(&stored_later, &scheduled));

    let no_receipt = read_control_receipts(&Document::new(), &scheduled).unwrap();
    assert!(!operation.was_recorded(&no_receipt, &scheduled));

    let mut other_agent = scheduled.clone();
    other_agent.id = AgentId("agt_other".into());
    assert!(matches!(
        read_control_receipts(&receipt, &other_agent),
        Err(AgentControlStoreError::Corrupt(_))
    ));
    let mut future_request = match operation.request.clone() {
        ControlRequest::Stop(request) => request,
        ControlRequest::Delete(_) => unreachable!("the fixture is a stop request"),
    };
    future_request.target.expected_revision = scheduled.revision + 5;
    let future = ControlOperation::new(ControlRequest::Stop(future_request.clone()));
    let mut future_scheduled = scheduled.clone();
    future_scheduled.revision = future_request.target.expected_revision + 1;
    let future_receipt = future.receipt_update(&future_scheduled).unwrap();
    assert!(matches!(
        read_control_receipts(&future_receipt, &scheduled),
        Err(AgentControlStoreError::Corrupt(_))
    ));

    let target_mutations = [
        ("agent_id", Bson::String("agt_other".into())),
        ("worktree_id", Bson::String("wkt_other".into())),
        ("workspace_id", Bson::String("wks_other".into())),
        ("sandbox_id", Bson::String("sbx_other".into())),
        ("owner", Bson::String("other-owner".into())),
        ("expected_status", Bson::String("ready".into())),
        (
            "expected_revision",
            Bson::Int64(i64::try_from(scheduled.revision).unwrap()),
        ),
    ];
    for (field, value) in target_mutations {
        let mut changed = receipt.clone();
        stop_receipt_request_mut(&mut changed)
            .get_document_mut("target")
            .unwrap()
            .insert(field, value);
        assert_receipt_rejected(&changed, &operation, &scheduled);
    }
    for (field, value) in [
        ("reason", Bson::String("operator".into())),
        ("requested_at", Bson::String("2026-09-05T00:00:01Z".into())),
    ] {
        let mut changed = receipt.clone();
        stop_receipt_request_mut(&mut changed).insert(field, value);
        assert_receipt_rejected(&changed, &operation, &scheduled);
    }
    let mut changed_kind = receipt.clone();
    changed_kind
        .get_document_mut(STOP_RECEIPT_FIELD)
        .unwrap()
        .get_document_mut("request")
        .unwrap()
        .insert("kind", "delete");
    assert_receipt_rejected(&changed_kind, &operation, &scheduled);
    let mut changed_wrapper = receipt.clone();
    changed_wrapper
        .get_document_mut(STOP_RECEIPT_FIELD)
        .unwrap()
        .get_document_mut("request")
        .unwrap()
        .insert("unexpected", true);
    assert!(matches!(
        read_control_receipts(&changed_wrapper, &scheduled),
        Err(AgentControlStoreError::Corrupt(_))
    ));

    for (field, value) in [
        ("document_schema_version", Bson::Int64(2)),
        ("operation_id", Bson::String("ctl_not-a-uuid".into())),
        (
            "scheduled_revision",
            Bson::Int64(i64::try_from(scheduled.revision + 1).unwrap()),
        ),
        ("unexpected", Bson::Boolean(true)),
    ] {
        let mut malformed = receipt.clone();
        malformed
            .get_document_mut(STOP_RECEIPT_FIELD)
            .unwrap()
            .insert(field, value);
        assert!(matches!(
            read_control_receipts(&malformed, &scheduled),
            Err(AgentControlStoreError::Corrupt(_))
        ));
    }
}

fn stop_receipt_request_mut(document: &mut Document) -> &mut Document {
    document
        .get_document_mut(STOP_RECEIPT_FIELD)
        .unwrap()
        .get_document_mut("request")
        .unwrap()
        .get_document_mut("request")
        .unwrap()
}

fn assert_receipt_rejected(document: &Document, operation: &ControlOperation, scheduled: &Agent) {
    match read_control_receipts(document, scheduled) {
        Ok(receipts) => assert!(!operation.was_recorded(&receipts, scheduled)),
        Err(AgentControlStoreError::Corrupt(_)) => {}
        Err(error) => panic!("receipt mutation returned an unexpected error: {error}"),
    }
}

fn assert_control_receipts_corrupt(document: &Document, agent: &Agent) {
    assert!(matches!(
        read_control_receipts(document, agent),
        Err(AgentControlStoreError::Corrupt(_))
    ));
}

#[test]
fn agent_control_receipts_reject_impossible_histories() {
    let current = agent_for_worker("agt_control_receipt_history", NOW);
    let stop = ControlOperation::new(ControlRequest::Stop(AgentStopRequest {
        target: AgentControlTarget::from(&current),
        reason: AgentStopReason::Owner,
        requested_at: NOW.into(),
    }));
    let scheduled = stop.request.plan(current).unwrap().into_agent();
    let stop_receipt = stop.receipt_update(&scheduled).unwrap();

    for status in ["stopped", "deleted", "failed"] {
        let mut malformed = stop_receipt.clone();
        stop_receipt_request_mut(&mut malformed)
            .get_document_mut("target")
            .unwrap()
            .insert("expected_status", status);
        assert_control_receipts_corrupt(&malformed, &scheduled);
    }

    let mut cleared_stop_workflow = scheduled.clone();
    cleared_stop_workflow.requested_stop_reason = None;
    cleared_stop_workflow.work.step = AgentWorkStep::None;
    assert_control_receipts_corrupt(&stop_receipt, &cleared_stop_workflow);

    let mut stopped = scheduled.clone();
    stopped.work.schedule(AgentWorkStep::None, MINUTE.into());
    stopped
        .transition(
            AgentStatus::Stopped,
            MINUTE.into(),
            Some(AgentStopReason::Owner),
        )
        .unwrap();
    let delete = ControlOperation::new(ControlRequest::Delete(AgentDeleteRequest {
        target: AgentControlTarget::from(&stopped),
        requested_at: TWO_MINUTES.into(),
    }));
    let deleting = delete.request.plan(stopped).unwrap().into_agent();
    let delete_receipt = delete.receipt_update(&deleting).unwrap();

    for status in [
        "starting",
        "ready",
        "working",
        "waiting_approval",
        "paused",
        "deleted",
    ] {
        let mut malformed = delete_receipt.clone();
        malformed
            .get_document_mut(DELETE_RECEIPT_FIELD)
            .unwrap()
            .get_document_mut("request")
            .unwrap()
            .get_document_mut("request")
            .unwrap()
            .get_document_mut("target")
            .unwrap()
            .insert("expected_status", status);
        assert_control_receipts_corrupt(&malformed, &deleting);
    }

    let mut history = stop_receipt.clone();
    history.extend(delete_receipt.clone());
    read_control_receipts(&history, &deleting).unwrap();

    let mut out_of_order = history.clone();
    let delete_revision = i64::try_from(deleting.revision).unwrap();
    let stop = out_of_order.get_document_mut(STOP_RECEIPT_FIELD).unwrap();
    stop.insert("scheduled_revision", delete_revision);
    stop.get_document_mut("request")
        .unwrap()
        .get_document_mut("request")
        .unwrap()
        .get_document_mut("target")
        .unwrap()
        .insert("expected_revision", delete_revision - 1);
    assert_control_receipts_corrupt(&out_of_order, &deleting);

    let mut duplicate_operation = history;
    let stop_operation_id = duplicate_operation
        .get_document(STOP_RECEIPT_FIELD)
        .unwrap()
        .get_str("operation_id")
        .unwrap()
        .to_owned();
    duplicate_operation
        .get_document_mut(DELETE_RECEIPT_FIELD)
        .unwrap()
        .insert("operation_id", stop_operation_id);
    assert_control_receipts_corrupt(&duplicate_operation, &deleting);
}

#[test]
fn agent_control_reconciliation_accepts_only_valid_workflow_descendants() {
    let current = agent_for_worker("agt_control_descendant", NOW);
    let stop_request = AgentStopRequest {
        target: AgentControlTarget::from(&current),
        reason: AgentStopReason::Owner,
        requested_at: NOW.into(),
    };
    let stop = ControlRequest::Stop(stop_request);
    let scheduled = stop.plan(current).unwrap().into_agent();
    assert!(stop.durable_replay(&scheduled, &scheduled));

    let mut stopping = scheduled.clone();
    stopping.work.step = AgentWorkStep::CloseProcess;
    stopping.revision += 1;
    assert!(stop.durable_replay(&stopping, &scheduled));

    let mut stopped = scheduled.clone();
    stopped.work.schedule(AgentWorkStep::None, MINUTE.into());
    stopped
        .transition(
            AgentStatus::Stopped,
            MINUTE.into(),
            Some(AgentStopReason::Owner),
        )
        .unwrap();
    assert!(stop.durable_replay(&stopped, &scheduled));

    let mut invalid_stop = stopped.clone();
    invalid_stop.work.step = AgentWorkStep::AddWorktree;
    assert!(!stop.durable_replay(&invalid_stop, &scheduled));

    let delete_request = AgentDeleteRequest {
        target: AgentControlTarget::from(&stopped),
        requested_at: TWO_MINUTES.into(),
    };
    let delete = ControlRequest::Delete(delete_request);
    let deleting = delete.plan(stopped).unwrap().into_agent();
    assert!(stop.durable_replay(&deleting, &scheduled));
    assert!(delete.durable_replay(&deleting, &deleting));

    let mut changed_terminal_status = deleting.clone();
    changed_terminal_status.status = AgentStatus::Failed;
    assert!(!delete.durable_replay(&changed_terminal_status, &deleting));

    let mut changed_terminal_evidence = deleting.clone();
    changed_terminal_evidence.stop_reason = Some(AgentStopReason::Operator);
    assert!(!delete.durable_replay(&changed_terminal_evidence, &deleting));

    let mut deleted = deleting.clone();
    deleted
        .transition(AgentStatus::Deleted, TWO_MINUTES.into(), None)
        .unwrap();
    assert!(stop.durable_replay(&deleted, &scheduled));
    assert!(delete.durable_replay(&deleted, &deleting));

    deleted.work.step = AgentWorkStep::FenceInteraction;
    assert!(!stop.durable_replay(&deleted, &scheduled));
    assert!(!delete.durable_replay(&deleted, &deleting));

    let mut failed = agent_for_worker("agt_control_failed_descendant", NOW);
    failed.work.schedule(AgentWorkStep::None, NOW.into());
    failed
        .fail(
            TypedFailure {
                class: "startup_failed".into(),
                detail: "runtime did not start".into(),
            },
            MINUTE.into(),
        )
        .unwrap();
    let failed_delete = ControlRequest::Delete(AgentDeleteRequest {
        target: AgentControlTarget::from(&failed),
        requested_at: TWO_MINUTES.into(),
    });
    let failed_deleting = failed_delete.plan(failed).unwrap().into_agent();
    assert!(failed_delete.durable_replay(&failed_deleting, &failed_deleting));

    let mut changed_failed_status = failed_deleting.clone();
    changed_failed_status.status = AgentStatus::Stopped;
    assert!(!failed_delete.durable_replay(&changed_failed_status, &failed_deleting));

    let mut failed_deleted = failed_deleting.clone();
    failed_deleted
        .transition(AgentStatus::Deleted, TWO_MINUTES.into(), None)
        .unwrap();
    assert!(failed_delete.durable_replay(&failed_deleted, &failed_deleting));

    failed_deleted.failure = None;
    assert!(!failed_delete.durable_replay(&failed_deleted, &failed_deleting));
}

#[test]
fn agent_control_reconciliation_binds_profile_and_origin() {
    let expected = agent_for_worker("agt_control_incarnation", NOW);
    assert!(same_agent_incarnation(&expected, &expected));

    let mut changed_profile = expected.clone();
    changed_profile.profile.digest = "e".repeat(64);
    assert!(!same_agent_incarnation(&changed_profile, &expected));

    let mut changed_origin = expected.clone();
    changed_origin.origin = AgentOrigin::Trigger {
        trigger_id: "trigger-1".into(),
        launch_id: "launch-1".into(),
    };
    assert!(!same_agent_incarnation(&changed_origin, &expected));
}

#[test]
fn agent_control_reconciliation_errors_fail_closed() {
    let commit = mongodb::error::Error::custom("ambiguous commit");
    assert!(matches!(
        reconciliation_error(
            LineageError::Domain(AgentControlStoreError::NotFound),
            &commit,
        ),
        AgentControlStoreError::Unavailable(_)
    ));
    assert!(matches!(
        reconciliation_error(
            LineageError::Domain(AgentControlStoreError::Corrupt("parent mismatch".into())),
            &commit,
        ),
        AgentControlStoreError::Unavailable(_)
    ));
    assert_eq!(
        reconciliation_error(LineageError::ReceiptCorrupt("bad receipt".into()), &commit),
        AgentControlStoreError::Corrupt("bad receipt".into())
    );
}

#[test]
fn historical_owner_stop_is_separate_from_the_replacement_stop() {
    use layer3_adapter::{AgentCloseRequest, AgentProcessAction, AgentSlot};

    let mut original = agent_for_worker("agt_restart_stop_predecessor", NOW);
    original.conversation.conversation_id = Some("retained-conversation".into());
    original.conversation.runtime_generation = Some(1);
    let first_stop = ControlOperation::new(ControlRequest::Stop(AgentStopRequest {
        target: AgentControlTarget::from(&original),
        reason: AgentStopReason::Owner,
        requested_at: NOW.into(),
    }));
    let old_scheduled = first_stop
        .request
        .plan(original.clone())
        .unwrap()
        .into_agent();
    let old_receipt = first_stop.receipt_update(&old_scheduled).unwrap();
    let mut replacement = original;
    replacement.revision = old_scheduled.revision + 4;
    replacement
        .work
        .schedule(AgentWorkStep::None, MINUTE.into());
    replacement.status = AgentStatus::Starting;
    let close = AgentCloseRequest::new(
        AgentProcessAction::Close.operation_id(replacement.id.as_str(), 1),
        AgentSlot::new(replacement.id.as_str()),
    );
    let field = "control_restart_predecessor_v1";
    let mut document = doc! {
        (field): {
            "restart_revision": replacement.revision as i64,
            "conversation_id": "retained-conversation",
            "provision_generation": 1_i64,
            "closed_incarnation": 1_i64,
            "restart_incarnation": 2_i64,
            "close_operation_id": &close.operation_id,
            "close_request_sha256": &close.request_sha256,
            "stop": old_receipt.get_document(STOP_RECEIPT_FIELD).unwrap(),
        },
    };
    let decoded = read_control_receipts(&document, &replacement).unwrap();
    assert!(!first_stop.was_recorded(&decoded, &old_scheduled));

    replacement.status = AgentStatus::Ready;
    replacement.revision += 1;
    replacement.conversation.runtime_generation = Some(2);
    let second_stop = ControlOperation::new(ControlRequest::Stop(AgentStopRequest {
        target: AgentControlTarget::from(&replacement),
        reason: AgentStopReason::Owner,
        requested_at: MINUTE.into(),
    }));
    let new_scheduled = second_stop
        .request
        .plan(replacement.clone())
        .unwrap()
        .into_agent();
    document.extend(second_stop.receipt_update(&new_scheduled).unwrap());
    let decoded = read_control_receipts(&document, &new_scheduled).unwrap();
    assert!(second_stop.was_recorded(&decoded, &new_scheduled));
    assert!(!first_stop.was_recorded(&decoded, &old_scheduled));

    let mut stale = document.clone();
    stale.extend(old_receipt.clone());
    assert!(matches!(
        read_control_receipts(&stale, &new_scheduled),
        Err(AgentControlStoreError::Corrupt(_))
    ));
    let mut wrong_close = document.clone();
    wrong_close
        .get_document_mut(field)
        .unwrap()
        .insert("close_request_sha256", "0".repeat(64));
    assert!(matches!(
        read_control_receipts(&wrong_close, &new_scheduled),
        Err(AgentControlStoreError::Corrupt(_))
    ));
    let mut wrong_owner = document.clone();
    let request = wrong_owner
        .get_document_mut(field)
        .unwrap()
        .get_document_mut("stop")
        .unwrap()
        .get_document_mut("request")
        .unwrap()
        .get_document_mut("request")
        .unwrap();
    request
        .get_document_mut("target")
        .unwrap()
        .insert("owner", "other-owner");
    assert!(matches!(
        read_control_receipts(&wrong_owner, &new_scheduled),
        Err(AgentControlStoreError::Corrupt(_))
    ));
}
