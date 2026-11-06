use super::*;
use layer2_core::sandbox::{AgentMessageAssembly, AgentUpdateRecorder, DurableAgentUpdateRecorder};
use layer3_adapter::{
    ConversationUpdate, RuntimeOutputReceipt, RuntimeOutputSource, TurnStopReason,
};

mod capacity_tests;

#[test]
fn agent_log_terminal_capacity_requires_a_terminal_publication() {
    let starting = agent_for_worker("agt_log_capacity", NOW);
    let publication =
        |agent: Agent,
         appended_at: &str,
         agent_logs: Vec<AgentLogKind>,
         sandbox_event: Option<SandboxEventKind>| AgentWorkMutation {
            expected_agent_revision: 1,
            holder: "worker".into(),
            fence: 1,
            agent,
            worktree: None,
            appended_at: appended_at.into(),
            sandbox_event,
            agent_logs,
        };

    let mut ready = starting.clone();
    ready.status = AgentStatus::Ready;
    let ready = publication(ready, NOW, vec![], None);
    assert_eq!(
        publication_capacity(&starting, &ready),
        AppendCapacity::Normal
    );

    let failure = TypedFailure {
        class: "startup_failed".into(),
        detail: "runtime did not start".into(),
    };
    let mut failed = starting.clone();
    failed.status = AgentStatus::Failed;
    failed.updated_at = MINUTE.into();
    failed.failure = Some(failure.clone());
    let failed = publication(
        failed,
        MINUTE,
        vec![AgentLogKind::WorkStepFailed {
            step: starting.work.step,
            failure: failure.clone(),
        }],
        Some(SandboxEventKind::AgentStatusChanged {
            agent_id: starting.id.clone(),
            from: AgentStatus::Starting,
            to: AgentStatus::Failed,
        }),
    );
    assert_eq!(
        publication_capacity(&starting, &failed),
        AppendCapacity::StopOrFailure
    );
    let mut incomplete_failure = failed.clone();
    incomplete_failure.agent_logs.clear();
    assert_eq!(
        publication_capacity(&starting, &incomplete_failure),
        AppendCapacity::Normal
    );

    let mut stopped = starting.clone();
    stopped.status = AgentStatus::Stopped;
    stopped.updated_at = MINUTE.into();
    stopped.stopped_at = Some(MINUTE.into());
    stopped.stop_reason = Some(AgentStopReason::Owner);
    let stopped = publication(
        stopped,
        MINUTE,
        vec![AgentLogKind::Stopped {
            reason: AgentStopReason::Owner.as_str().into(),
        }],
        None,
    );
    assert_eq!(
        publication_capacity(&starting, &stopped),
        AppendCapacity::StopOrFailure
    );
    let mut wrong_stop = stopped.clone();
    wrong_stop.agent_logs = vec![AgentLogKind::Stopped {
        reason: AgentStopReason::Operator.as_str().into(),
    }];
    assert_eq!(
        publication_capacity(&starting, &wrong_stop),
        AppendCapacity::Normal
    );

    let mut deleted = stopped.agent.clone();
    deleted.status = AgentStatus::Deleted;
    deleted.updated_at = TWO_MINUTES.into();
    deleted.deleted_at = Some(TWO_MINUTES.into());
    let deleted = publication(
        deleted,
        TWO_MINUTES,
        vec![AgentLogKind::Deleted],
        Some(SandboxEventKind::AgentDeleted {
            agent_id: starting.id.clone(),
        }),
    );
    assert_eq!(
        publication_capacity(&stopped.agent, &deleted),
        AppendCapacity::Delete
    );
    let mut duplicate_delete = deleted.clone();
    duplicate_delete.agent_logs.push(AgentLogKind::Deleted);
    assert_eq!(
        publication_capacity(&stopped.agent, &duplicate_delete),
        AppendCapacity::Normal
    );
    assert_eq!(
        publication_capacity(&starting, &deleted),
        AppendCapacity::Normal
    );
}

#[test]
fn mongo_agent_conversation_validation_fences_every_durable_boundary() {
    let current = conversation_agent_for_rules("agt_conversation_rules");
    let mutation = begin_conversation_turn(current.clone());
    assert!(validate_mutation(&current, 2, &mutation).is_ok());

    let rejected = |mutation: &AgentConversationMutation, actual_last_seq| {
        assert_eq!(
            validate_mutation(&current, actual_last_seq, mutation),
            Err(SandboxStoreError::Conflict)
        );
    };
    let mut invalid = mutation.clone();
    invalid.appended_at = "not-a-timestamp".into();
    rejected(&invalid, 2);
    let mut invalid = mutation.clone();
    invalid.runtime_generation = 0;
    rejected(&invalid, 2);
    let mut invalid = mutation.clone();
    invalid.expected_agent_revision += 1;
    rejected(&invalid, 2);
    let mut invalid = mutation.clone();
    invalid.expected_last_seq += 1;
    rejected(&invalid, 2);
    rejected(&mutation, 1);
    let mut invalid = mutation.clone();
    invalid.agent.conversation.last_seq += 1;
    rejected(&invalid, 2);
    let mut invalid = mutation.clone();
    invalid.agent.conversation.conversation_id = Some("different-conversation".into());
    rejected(&invalid, 2);
    let mut invalid = mutation.clone();
    invalid.agent.conversation.runtime_generation = Some(2);
    rejected(&invalid, 2);
    let mut invalid = mutation.clone();
    invalid.agent.requested_stop_reason = Some(AgentStopReason::Owner);
    rejected(&invalid, 2);
    let mut invalid = mutation;
    invalid.agent_logs.clear();
    rejected(&invalid, 2);
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_conversation_atomically_saves_and_reconciles_aggregate_and_logs() {
    let (client, name) = database("agent_conversation_atomic").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let ready = ready_conversation_fixture(&store, "conversation_atomic").await;
    let mutation = begin_stored_conversation_turn(&store, ready).await;
    let expected = mutation.agent.clone();

    let appended = store.commit_agent_conversation(mutation).await.unwrap();
    assert_eq!(
        appended.iter().map(|entry| entry.seq).collect::<Vec<_>>(),
        [3, 4]
    );
    assert_eq!(
        store.get_agent(&expected.id).await.unwrap(),
        Some(expected.clone())
    );
    assert_eq!(store.agent_log(&expected.id, 0, 10).await.unwrap().len(), 4);
    let receipt = ConversationReceipt {
        command_dispatch: None,
        agent: expected,
        logs: appended,
    };
    assert!(reconcile_conversation(&store, &receipt).await.unwrap());

    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn mongo_recorder_persists_identity_terminal_without_changing_receipt_shape() {
    let (client, name) = database("identity_terminal_receipt").await;
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let ready = ready_conversation_fixture(&store, "identity_terminal_receipt").await;
    let turn = begin_stored_conversation_turn(&store, ready).await;
    let working = turn.agent.clone();
    store.commit_agent_conversation(turn).await.unwrap();
    let recorder = DurableAgentUpdateRecorder::new(store.clone());
    let source = |cursor: u64, hash: &str| {
        RuntimeOutputReceipt::new(
            RuntimeOutputSource {
                stream_id: "d99003d1-0c3c-46cd-aa47-9bbdbed50b12".into(),
                cursor,
                native_conversation_id: working.conversation.conversation_id.clone().unwrap(),
                payload_sha256: hash.into(),
            },
            tokio::sync::mpsc::channel(1).0,
        )
    };
    recorder
        .record_update(
            &working.id,
            1,
            &mut AgentMessageAssembly::default(),
            ConversationUpdate::RuntimeOutput {
                receipt: source(1, &"a".repeat(64)),
                update: None,
            },
            NOW.into(),
        )
        .await
        .unwrap();
    let prompt_id = layer3_adapter::PromptIdentity::new(
        working.conversation.conversation_id.as_deref().unwrap(),
        working.conversation.runtime_generation.unwrap(),
        working.conversation.active_turn.unwrap(),
    );
    let outcome = recorder
        .record_update(
            &working.id,
            1,
            &mut AgentMessageAssembly::default(),
            ConversationUpdate::RuntimeOutput {
                receipt: source(2, &"b".repeat(64)),
                update: Some(Box::new(ConversationUpdate::TurnEndedForPrompt {
                    prompt_id,
                    response: layer3_adapter::PromptResponse::new(TurnStopReason::EndTurn),
                })),
            },
            MINUTE.into(),
        )
        .await
        .unwrap();
    assert!(outcome.turn_closed);
    let checkpoint = store
        .agent_message_checkpoint(&working.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(checkpoint.output.unwrap().cursor, 2);
    assert_eq!(
        store.get_agent(&working.id).await.unwrap().unwrap().status,
        AgentStatus::Ready
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_conversation_reconciliation_uses_one_complete_snapshot() {
    let (client, name) = database("agent_conversation_reconcile_snapshot").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let ready = ready_conversation_fixture(&store, "conversation_reconcile_snapshot").await;
    let mutation = begin_stored_conversation_turn(&store, ready).await;
    let expected = mutation.agent.clone();
    let appended = store.commit_agent_conversation(mutation).await.unwrap();
    let receipt = ConversationReceipt {
        command_dispatch: None,
        agent: expected,
        logs: appended,
    };
    let complete = store.agent_log(&receipt.agent.id, 0, 10).await.unwrap();

    let orphan = AgentLogEntry {
        agent_id: receipt.agent.id.clone(),
        seq: receipt.agent.conversation.last_seq + 1,
        appended_at: TWO_MINUTES.into(),
        kind: AgentLogKind::Deleted,
    };
    store
        .agent_log
        .insert_one(log_document(&orphan).unwrap())
        .await
        .unwrap();
    assert_eq!(
        reconcile_conversation(&store, &receipt).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    let deleted = store
        .agent_log
        .delete_one(doc! {
            "agent_id": orphan.agent_id.as_str(),
            "seq": i64::try_from(orphan.seq).unwrap(),
        })
        .await
        .unwrap();
    assert_eq!(deleted.deleted_count, 1);

    let earlier = complete.first().unwrap().clone();
    store
        .agent_log
        .delete_one(doc! { "agent_id": earlier.agent_id.as_str(), "seq": 1_i64 })
        .await
        .unwrap();
    assert_eq!(
        reconcile_conversation(&store, &receipt).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    store
        .agent_log
        .insert_one(log_document(&earlier).unwrap())
        .await
        .unwrap();

    let mut wrong = receipt.clone();
    wrong.logs[0].kind = AgentLogKind::Deleted;
    assert_eq!(
        reconcile_conversation(&store, &wrong).await,
        Err(SandboxStoreError::SequenceConflict)
    );

    let mut log_snapshot = client.start_session().snapshot(true).await.unwrap();
    store
        .agents
        .find_one(doc! { "_id": receipt.agent.id.as_str() })
        .read_concern(ReadConcern::snapshot())
        .session(&mut log_snapshot)
        .await
        .unwrap();
    let receipt_entry = receipt.logs[0].clone();
    store
        .agent_log
        .delete_one(doc! {
            "agent_id": receipt_entry.agent_id.as_str(),
            "seq": i64::try_from(receipt_entry.seq).unwrap(),
        })
        .await
        .unwrap();
    assert!(
        reconcile_conversation_snapshot(&store, &receipt, &mut log_snapshot)
            .await
            .unwrap()
    );
    assert_eq!(
        reconcile_conversation(&store, &receipt).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    store
        .agent_log
        .insert_one(log_document(&receipt_entry).unwrap())
        .await
        .unwrap();

    let mut aggregate_snapshot = client.start_session().snapshot(true).await.unwrap();
    store
        .agent_log
        .find_one(doc! { "agent_id": receipt.agent.id.as_str(), "seq": 1_i64 })
        .read_concern(ReadConcern::snapshot())
        .session(&mut aggregate_snapshot)
        .await
        .unwrap();
    let mut later = receipt.agent.clone();
    later.revision += 1;
    later.updated_at = TWO_MINUTES.into();
    store
        .save_agent(receipt.agent.revision, later)
        .await
        .unwrap();
    assert!(
        reconcile_conversation_snapshot(&store, &receipt, &mut aggregate_snapshot)
            .await
            .unwrap()
    );
    assert!(!reconcile_conversation(&store, &receipt).await.unwrap());

    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_conversation_rejects_invalid_and_stale_mutations_without_partial_writes() {
    let (client, name) = database("agent_conversation_rollback").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let ready = ready_conversation_fixture(&store, "conversation_rollback").await;
    let mutation = begin_stored_conversation_turn(&store, ready.clone()).await;
    let mut invalid_mutations = Vec::new();

    let mut invalid = mutation.clone();
    invalid.appended_at = "not-a-timestamp".into();
    invalid_mutations.push(invalid);
    let mut invalid = mutation.clone();
    invalid.runtime_generation = 2;
    invalid_mutations.push(invalid);
    let mut invalid = mutation.clone();
    invalid.expected_agent_revision += 1;
    invalid_mutations.push(invalid);
    let mut invalid = mutation.clone();
    invalid.expected_last_seq += 1;
    invalid_mutations.push(invalid);
    let mut invalid = mutation.clone();
    invalid.agent.capabilities = layer2_core::RuntimeCapabilities::default();
    invalid_mutations.push(invalid);
    let mut invalid = mutation;
    invalid.agent.requested_stop_reason = Some(AgentStopReason::Owner);
    invalid_mutations.push(invalid);

    for invalid in invalid_mutations {
        assert_eq!(
            store.commit_agent_conversation(invalid).await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(
            store.get_agent(&ready.id).await.unwrap(),
            Some(ready.clone())
        );
        assert_eq!(store.agent_log(&ready.id, 0, 10).await.unwrap().len(), 2);
    }

    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn terminal_agents_reject_matching_generation_runtime_updates_without_writes() {
    let (client, name) = database("agent_conversation_terminal_updates").await;
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());

    for (suffix, status) in [
        ("stopped", AgentStatus::Stopped),
        ("deleted", AgentStatus::Deleted),
    ] {
        let ready = ready_conversation_fixture(&store, suffix).await;
        let mut terminal = ready.clone();
        terminal.status = status;
        terminal.stopped_at = Some(MINUTE.into());
        terminal.stop_reason = Some(AgentStopReason::Owner);
        terminal.updated_at = MINUTE.into();
        terminal.deleted_at = (status == AgentStatus::Deleted).then(|| MINUTE.into());
        terminal.revision += 1;
        store
            .save_agent(ready.revision, terminal.clone())
            .await
            .unwrap();
        let before_logs = store.agent_log(&terminal.id, 0, 20).await.unwrap();
        let recorder = DurableAgentUpdateRecorder::new(store.clone());

        let updates = [
            ConversationUpdate::Lost { frames: 3 },
            ConversationUpdate::SignInRequired {
                url: "https://signin.example/continue".into(),
                code: Some("verification-code".into()),
                message: Some("sign in required".into()),
            },
        ];
        for update in updates {
            let mut assembly = AgentMessageAssembly::default();
            assert_eq!(
                recorder
                    .record_update(&terminal.id, 1, &mut assembly, update, TWO_MINUTES.into())
                    .await,
                Err(layer2_core::sandbox::AgentConversationError::InvalidState(
                    "conversation updates require a live agent"
                ))
            );
            assert!(assembly.is_empty());
            assert_eq!(
                store.get_agent(&terminal.id).await.unwrap(),
                Some(terminal.clone())
            );
            assert_eq!(
                store.agent_log(&terminal.id, 0, 20).await.unwrap(),
                before_logs
            );
        }
    }

    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_conversation_rebuilds_approvals_and_rejects_incomplete_logs() {
    let (client, name) = database("agent_conversation_approvals").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let ready = ready_conversation_fixture(&store, "conversation_approvals").await;
    let turn = begin_stored_conversation_turn(&store, ready).await;
    let mut working = turn.agent.clone();
    store.commit_agent_conversation(turn).await.unwrap();

    let expected_agent_revision = working.revision;
    let expected_last_seq = working.conversation.last_seq;
    working
        .transition(AgentStatus::WaitingApproval, MINUTE.into(), None)
        .unwrap();
    working.conversation.last_seq += 1;
    store
        .commit_agent_conversation(AgentConversationMutation {
            settings_fence: None,
            command_claim: None,
            execution_fence: None,
            trigger_execution_fence: None,
            admission_guard: None,
            expected_agent_revision,
            expected_last_seq,
            runtime_generation: 1,
            agent: working.clone(),
            appended_at: MINUTE.into(),
            agent_logs: vec![AgentLogKind::ApprovalRequested {
                approval_id: "approval-1".into(),
                action: Some("Run checks".into()),
                detail: None,
                runtime_locator: Some("runtime-route-1".into()),
            }],
        })
        .await
        .unwrap();

    drop(store);
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    let pending = reopened.pending_agent_approvals(&working.id).await.unwrap();
    assert!(matches!(
        pending.as_slice(),
        [AgentLogEntry {
            kind: AgentLogKind::ApprovalRequested { approval_id, .. },
            ..
        }] if approval_id == "approval-1"
    ));

    let orphan = AgentLogEntry {
        agent_id: working.id.clone(),
        seq: working.conversation.last_seq + 1,
        appended_at: TWO_MINUTES.into(),
        kind: AgentLogKind::ApprovalRequested {
            approval_id: "orphan-approval".into(),
            action: Some("Must not be hidden".into()),
            detail: None,
            runtime_locator: None,
        },
    };
    reopened
        .agent_log
        .insert_one(log_document(&orphan).unwrap())
        .await
        .unwrap();
    assert_eq!(
        reopened.pending_agent_approvals(&working.id).await,
        Err(SandboxStoreError::SequenceConflict)
    );

    let expected_agent_revision = working.revision;
    let expected_last_seq = working.conversation.last_seq;
    let mut resumed = working.clone();
    resumed
        .transition(AgentStatus::Working, TWO_MINUTES.into(), None)
        .unwrap();
    resumed.conversation.last_seq += 1;
    let resume = AgentConversationMutation {
        settings_fence: None,
        command_claim: None,
        execution_fence: None,
        trigger_execution_fence: None,
        admission_guard: None,
        expected_agent_revision,
        expected_last_seq,
        runtime_generation: 1,
        agent: resumed,
        appended_at: TWO_MINUTES.into(),
        agent_logs: vec![AgentLogKind::ApprovalResolved {
            approval_id: "approval-1".into(),
            decision: "approved".into(),
        }],
    };
    assert_eq!(
        reopened.commit_agent_conversation(resume.clone()).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    reopened
        .agent_log
        .delete_one(doc! {
            "agent_id": orphan.agent_id.as_str(),
            "seq": 6_i64,
        })
        .await
        .unwrap();

    reopened
        .agent_log
        .delete_one(doc! { "agent_id": working.id.as_str(), "seq": 4_i64 })
        .await
        .unwrap();
    assert_eq!(
        reopened.pending_agent_approvals(&working.id).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert_eq!(
        reopened.commit_agent_conversation(resume).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert_eq!(
        reopened
            .append_agent_log(
                &working.id,
                TWO_MINUTES.into(),
                AgentLogKind::AgentMessage {
                    content: "must not append after a gap".into(),
                },
            )
            .await,
        Err(SandboxStoreError::SequenceConflict)
    );

    client.database(&name).drop().await.unwrap();
}

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

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn agent_control_reconciles_a_committed_but_ambiguous_transaction() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI")
        .expect("CHRONO_TEST_MONGODB_FAILPOINT_URI must name a failpoint-enabled replica set");
    let sequence = DATABASE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let app_name = format!(
        "cma-agent-control-ambiguous-{}-{sequence}",
        std::process::id()
    );
    let name = format!(
        "cma_811_agent_control_ambiguous_{}_{sequence}",
        std::process::id()
    );
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let trace = Arc::new(Mutex::new(CommandTrace::default()));
    let observed_trace = Arc::clone(&trace);
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        let mut trace = observed_trace.lock().unwrap();
        match event {
            CommandEvent::Started(event) => {
                trace.started.push((event.command_name, event.command));
            }
            CommandEvent::Failed(event) if event.command_name == "commitTransaction" => {
                trace.failed_commits += 1;
            }
            CommandEvent::Succeeded(event) if event.command_name == "commitTransaction" => {
                trace.commit_replies.push(event.reply);
            }
            _ => {}
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let suffix = format!("ambiguous_{sequence}");
    let (parent, worktree, claimed) = claimed_agent_work_fixture(&store, &suffix).await;
    let workspace = store
        .get_workspace(&claimed.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let request = AgentStopRequest {
        target: AgentControlTarget::from(&claimed),
        reason: AgentStopReason::Owner,
        requested_at: NOW.into(),
    };
    let log_count_before = store
        .agent_log
        .count_documents(doc! { "agent_id": claimed.id.as_str() })
        .await
        .unwrap();
    let event_count_before = store
        .sandbox_events
        .count_documents(Document::new())
        .await
        .unwrap();
    *trace.lock().unwrap() = CommandTrace::default();

    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand",
            "mode": { "times": 5_i32 },
            "data": {
                "failCommands": ["commitTransaction"],
                "appName": app_name,
                "writeConcernError": {
                    "code": 50_i32,
                    "codeName": "MaxTimeMSExpired",
                    "errmsg": "injected ambiguous commit",
                },
            },
        })
        .await
        .unwrap();
    let outcome = store.schedule_agent_stop(request.clone()).await;
    let disable_result = admin
        .database("admin")
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await;
    disable_result.expect("commit failpoint must be disabled before assertions");

    let scheduled = match outcome.unwrap() {
        AgentControlResult::Scheduled(agent) => agent,
        AgentControlResult::Existing(_) => panic!("fresh stop unexpectedly replayed"),
    };
    let trace = trace.lock().unwrap().clone();
    let commits = trace
        .started
        .iter()
        .filter_map(|(name, command)| (name == "commitTransaction").then_some(command))
        .collect::<Vec<_>>();
    assert_eq!(commits.len(), 5);
    assert_eq!(trace.failed_commits, 0);
    assert_eq!(trace.commit_replies.len(), 5);
    assert!(trace.commit_replies.iter().all(|reply| {
        reply
            .get_document("writeConcernError")
            .ok()
            .and_then(|error| error.get_i32("code").ok())
            == Some(50)
    }));
    let lsid = commits[0].get_document("lsid").unwrap();
    let transaction_number = commits[0].get("txnNumber").unwrap();
    assert!(commits.iter().all(|command| {
        command.get_document("lsid").ok() == Some(lsid)
            && command.get("txnNumber") == Some(transaction_number)
            && command.get_bool("autocommit").ok() == Some(false)
    }));
    let last_commit_position = trace
        .started
        .iter()
        .rposition(|(name, _)| name == "commitTransaction")
        .unwrap();
    let reconciliation_reads = trace
        .started
        .iter()
        .skip(last_commit_position + 1)
        .filter_map(|(name, command)| (name == "find").then_some(command))
        .collect::<Vec<_>>();
    assert_eq!(reconciliation_reads.len(), 4);
    let reconciliation_lsid = reconciliation_reads[0].get_document("lsid").unwrap();
    assert_ne!(reconciliation_lsid, lsid);
    let snapshot_time = reconciliation_reads[0]
        .get_document("readConcern")
        .unwrap()
        .get_timestamp("atClusterTime")
        .unwrap();
    assert!(reconciliation_reads.iter().all(|command| {
        command.get_document("lsid").ok() == Some(reconciliation_lsid)
            && command_uses_snapshot_read(command)
            && command
                .get_document("readConcern")
                .ok()
                .and_then(|concern| concern.get_timestamp("atClusterTime").ok())
                == Some(snapshot_time)
    }));

    assert_eq!(scheduled.revision, claimed.revision + 1);
    assert_eq!(
        scheduled.conversation.last_seq,
        claimed.conversation.last_seq
    );
    assert_eq!(
        store.get_agent(&scheduled.id).await.unwrap(),
        Some(scheduled.clone())
    );
    assert_eq!(store.get_sandbox(&parent.id).await.unwrap(), Some(parent));
    assert_eq!(
        store.get_workspace(&workspace.id).await.unwrap(),
        Some(workspace)
    );
    assert_eq!(
        store.get_worktree(&worktree.id).await.unwrap(),
        Some(worktree)
    );
    assert_eq!(
        store
            .agent_log
            .count_documents(doc! { "agent_id": scheduled.id.as_str() })
            .await
            .unwrap(),
        log_count_before
    );
    assert_eq!(
        store
            .sandbox_events
            .count_documents(Document::new())
            .await
            .unwrap(),
        event_count_before
    );

    let agent_document = store
        .agents
        .find_one(doc! { "_id": scheduled.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        agent_document.get_i64("revision").unwrap(),
        i64::try_from(scheduled.revision).unwrap()
    );
    let receipt = agent_document.get_document(STOP_RECEIPT_FIELD).unwrap();
    assert_eq!(
        receipt.get_i64("document_schema_version").unwrap(),
        DOCUMENT_SCHEMA_VERSION
    );
    assert_eq!(
        receipt.get_i64("scheduled_revision").unwrap(),
        i64::try_from(scheduled.revision).unwrap()
    );
    assert_eq!(
        receipt.get("request"),
        Some(&mongodb::bson::serialize_to_bson(&ControlRequest::Stop(request.clone())).unwrap())
    );
    let operation_id = receipt.get_str("operation_id").unwrap().to_owned();
    assert!(operation_id.starts_with("ctl_") && operation_id.len() == 36);
    for document in [
        store
            .worktrees
            .find_one(doc! { "_id": scheduled.worktree_id.as_str() })
            .await
            .unwrap()
            .unwrap(),
        store
            .workspaces
            .find_one(doc! { "_id": scheduled.workspace_id.as_str() })
            .await
            .unwrap()
            .unwrap(),
        store
            .sandboxes
            .find_one(doc! { "_id": scheduled.sandbox_id.as_str() })
            .await
            .unwrap()
            .unwrap(),
    ] {
        assert_eq!(
            document.get_str("last_agent_control_operation").unwrap(),
            operation_id.as_str()
        );
    }
    assert_eq!(
        store
            .agents
            .count_documents(doc! { "_id": scheduled.id.as_str() })
            .await
            .unwrap(),
        1
    );

    assert_eq!(
        store.schedule_agent_stop(request.clone()).await.unwrap(),
        AgentControlResult::Existing(scheduled.clone())
    );
    let mut changed = request;
    changed.reason = AgentStopReason::Operator;
    assert_eq!(
        store.schedule_agent_stop(changed).await,
        Err(AgentControlStoreError::StopReasonConflict)
    );
    assert_eq!(
        store
            .agents
            .find_one(doc! { "_id": claimed.id.as_str() })
            .await
            .unwrap()
            .unwrap(),
        agent_document
    );

    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_control_concurrent_stop_schedules_once_and_replays_exactly() {
    let (client, name) = database("agent_control_stop").await;
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let (_, _, claimed) = claimed_agent_work_fixture(&store, "control_stop").await;
    let request = AgentStopRequest {
        target: AgentControlTarget::from(&claimed),
        reason: AgentStopReason::Owner,
        requested_at: NOW.into(),
    };
    let mut tasks = Vec::new();
    for _ in 0..16 {
        let store = Arc::clone(&store);
        let request = request.clone();
        tasks.push(tokio::spawn(async move {
            store.schedule_agent_stop(request).await.unwrap()
        }));
    }
    let mut scheduled_count = 0;
    let mut winner = None;
    for task in tasks {
        match task.await.unwrap() {
            AgentControlResult::Scheduled(agent) => {
                scheduled_count += 1;
                winner = Some(agent);
            }
            AgentControlResult::Existing(agent) => {
                winner.get_or_insert(agent);
            }
        };
    }
    assert_eq!(scheduled_count, 1);
    let winner = winner.unwrap();
    assert_eq!(winner.requested_stop_reason, Some(AgentStopReason::Owner));
    assert_eq!(winner.work.step, AgentWorkStep::FenceInteraction);
    assert!(winner.work.lease.is_none());
    assert_eq!(
        store.schedule_agent_stop(request.clone()).await.unwrap(),
        AgentControlResult::Existing(winner.clone())
    );
    let mut changed = request;
    changed.reason = AgentStopReason::Operator;
    assert_eq!(
        store.schedule_agent_stop(changed).await,
        Err(AgentControlStoreError::StopReasonConflict)
    );
    assert_eq!(store.get_agent(&winner.id).await.unwrap(), Some(winner));

    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_control_delete_schedules_once_and_hides_foreign_owners() {
    let (client, name) = database("agent_control_delete").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (_, _, claimed) = claimed_agent_work_fixture(&store, "control_delete").await;
    let expected_revision = claimed.revision;
    let mut stopped = claimed;
    stopped.work.schedule(AgentWorkStep::None, MINUTE.into());
    stopped
        .transition(
            AgentStatus::Stopped,
            MINUTE.into(),
            Some(AgentStopReason::Owner),
        )
        .unwrap();
    store
        .save_agent(expected_revision, stopped.clone())
        .await
        .unwrap();
    let request = AgentDeleteRequest {
        target: AgentControlTarget::from(&stopped),
        requested_at: TWO_MINUTES.into(),
    };
    let mut foreign = request.clone();
    foreign.target.owner = SubjectId("another-owner".into());
    assert_eq!(
        store.schedule_agent_delete(foreign).await,
        Err(AgentControlStoreError::NotFound)
    );

    let scheduled = match store.schedule_agent_delete(request.clone()).await.unwrap() {
        AgentControlResult::Scheduled(agent) => agent,
        AgentControlResult::Existing(_) => panic!("new delete must be scheduled"),
    };
    assert_eq!(scheduled.work.step, AgentWorkStep::EnsureProcessClosed);
    assert_eq!(
        store.schedule_agent_delete(request).await.unwrap(),
        AgentControlResult::Existing(scheduled)
    );

    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_control_receipts_and_lineage_fences_survive_worker_progression() {
    let (client, name) = database("agent_control_receipts").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (parent, worktree, claimed) = claimed_agent_work_fixture(&store, "receipts").await;
    let workspace_before = store
        .get_workspace(&claimed.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let request = AgentStopRequest {
        target: AgentControlTarget::from(&claimed),
        reason: AgentStopReason::Owner,
        requested_at: NOW.into(),
    };
    let scheduled = store
        .schedule_agent_stop(request)
        .await
        .unwrap()
        .into_agent();

    assert_eq!(store.get_sandbox(&parent.id).await.unwrap(), Some(parent));
    assert_eq!(
        store.get_workspace(&workspace_before.id).await.unwrap(),
        Some(workspace_before.clone())
    );
    assert_eq!(
        store.get_worktree(&worktree.id).await.unwrap(),
        Some(worktree.clone())
    );

    let agent_document = store
        .agents
        .find_one(doc! { "_id": scheduled.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    let stop_receipt = agent_document.get(STOP_RECEIPT_FIELD).unwrap().clone();
    assert!(agent_document.get(DELETE_RECEIPT_FIELD).is_none());
    let markers = [
        store
            .worktrees
            .find_one(doc! { "_id": scheduled.worktree_id.as_str() })
            .await
            .unwrap()
            .unwrap(),
        store
            .workspaces
            .find_one(doc! { "_id": scheduled.workspace_id.as_str() })
            .await
            .unwrap()
            .unwrap(),
        store
            .sandboxes
            .find_one(doc! { "_id": scheduled.sandbox_id.as_str() })
            .await
            .unwrap()
            .unwrap(),
    ];
    let marker = markers[0].get_str("last_agent_control_operation").unwrap();
    assert!(marker.starts_with("ctl_"));
    assert!(markers
        .iter()
        .all(|document| { document.get_str("last_agent_control_operation").ok() == Some(marker) }));

    let mut advanced_workspace = workspace_before;
    let expected_workspace_revision = advanced_workspace.revision;
    advanced_workspace.next_worktree_number += 1;
    advanced_workspace.revision += 1;
    store
        .save_workspace(expected_workspace_revision, advanced_workspace.clone())
        .await
        .unwrap();
    let advanced_workspace_document = store
        .workspaces
        .find_one(doc! { "_id": advanced_workspace.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        advanced_workspace_document
            .get_str("last_agent_control_operation")
            .unwrap(),
        marker
    );

    let mut claimed_stop = store
        .claim_agent_work(&scheduled.id, "stop-worker", NOW.into(), MINUTE.into())
        .await
        .unwrap()
        .unwrap();
    claimed_stop.work.step = AgentWorkStep::PublishStopped;
    claimed_stop.work.cleanup_not_before = None;
    replace_agent_fixture(&store, &claimed_stop).await;
    let stop_fence = claimed_stop.work.lease.as_ref().unwrap().fence;
    let mut stopped = claimed_stop.clone();
    stopped
        .work
        .advance(
            "stop-worker",
            stop_fence,
            AgentWorkStep::None,
            MINUTE.into(),
        )
        .unwrap();
    stopped
        .transition(
            AgentStatus::Stopped,
            MINUTE.into(),
            Some(AgentStopReason::Owner),
        )
        .unwrap();
    stopped.conversation.last_seq += 1;
    let mut failed_worktree = worktree;
    let expected_worktree_revision = failed_worktree.revision;
    failed_worktree.transition(WorktreeStatus::Failed).unwrap();
    store
        .commit_agent_work(AgentWorkMutation {
            expected_agent_revision: claimed_stop.revision,
            holder: "stop-worker".into(),
            fence: stop_fence,
            agent: stopped.clone(),
            worktree: Some(VersionedWorktree {
                expected_revision: expected_worktree_revision,
                worktree: failed_worktree.clone(),
            }),
            appended_at: MINUTE.into(),
            sandbox_event: Some(SandboxEventKind::AgentStatusChanged {
                agent_id: stopped.id.clone(),
                from: AgentStatus::Starting,
                to: AgentStatus::Stopped,
            }),
            agent_logs: vec![AgentLogKind::Stopped {
                reason: AgentStopReason::Owner.as_str().into(),
            }],
        })
        .await
        .unwrap();
    let after_stop = store
        .agents
        .find_one(doc! { "_id": stopped.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after_stop.get(STOP_RECEIPT_FIELD), Some(&stop_receipt));

    let deleting = store
        .schedule_agent_delete(AgentDeleteRequest {
            target: AgentControlTarget::from(&stopped),
            requested_at: TWO_MINUTES.into(),
        })
        .await
        .unwrap()
        .into_agent();
    let after_delete_request = store
        .agents
        .find_one(doc! { "_id": deleting.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after_delete_request.get(STOP_RECEIPT_FIELD),
        Some(&stop_receipt)
    );
    let delete_receipt = after_delete_request
        .get(DELETE_RECEIPT_FIELD)
        .unwrap()
        .clone();

    let mut claimed_delete = store
        .claim_agent_work(
            &deleting.id,
            "delete-worker",
            TWO_MINUTES.into(),
            "2026-09-05T00:03:00Z".into(),
        )
        .await
        .unwrap()
        .unwrap();
    claimed_delete.work.step = AgentWorkStep::PublishDeleted;
    replace_agent_fixture(&store, &claimed_delete).await;
    failed_worktree.status = WorktreeStatus::Removed;
    replace_worktree_fixture(&store, &failed_worktree).await;
    let delete_fence = claimed_delete.work.lease.as_ref().unwrap().fence;
    let mut deleted = claimed_delete.clone();
    deleted
        .work
        .advance(
            "delete-worker",
            delete_fence,
            AgentWorkStep::None,
            TWO_MINUTES.into(),
        )
        .unwrap();
    deleted
        .transition(AgentStatus::Deleted, TWO_MINUTES.into(), None)
        .unwrap();
    deleted.conversation.last_seq += 1;
    store
        .commit_agent_work(AgentWorkMutation {
            expected_agent_revision: claimed_delete.revision,
            holder: "delete-worker".into(),
            fence: delete_fence,
            agent: deleted.clone(),
            worktree: None,
            appended_at: TWO_MINUTES.into(),
            sandbox_event: Some(SandboxEventKind::AgentDeleted {
                agent_id: deleted.id.clone(),
            }),
            agent_logs: vec![AgentLogKind::Deleted],
        })
        .await
        .unwrap();
    let after_deleted = store
        .agents
        .find_one(doc! { "_id": deleted.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after_deleted.get(STOP_RECEIPT_FIELD), Some(&stop_receipt));
    assert_eq!(
        after_deleted.get(DELETE_RECEIPT_FIELD),
        Some(&delete_receipt)
    );

    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_control_snapshot_requires_exact_receipt_and_current_parents() {
    let (client, name) = database("agent_control_snapshot").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (_, _, claimed) = claimed_agent_work_fixture(&store, "snapshot").await;
    let request = AgentStopRequest {
        target: AgentControlTarget::from(&claimed),
        reason: AgentStopReason::Owner,
        requested_at: NOW.into(),
    };
    let operation = ControlOperation::new(ControlRequest::Stop(request));
    let scheduled = operation
        .request
        .plan(claimed.clone())
        .unwrap()
        .into_agent();
    let mut fields = agent_replacement_fields(&scheduled).unwrap();
    fields.extend(operation.receipt_update(&scheduled).unwrap());
    store
        .agents
        .update_one(
            doc! { "_id": claimed.id.as_str(), "revision": i64::try_from(claimed.revision).unwrap() },
            doc! { "$set": fields },
        )
        .await
        .unwrap();

    let mut session = client.start_session().snapshot(true).await.unwrap();
    let snapshot = load_control_snapshot(&store, operation.request.target(), &mut session, true)
        .await
        .unwrap();
    assert!(control_snapshot_matches(&operation, &scheduled, &snapshot));

    let mut stopped = scheduled.clone();
    stopped.work.schedule(AgentWorkStep::None, MINUTE.into());
    stopped
        .transition(
            AgentStatus::Stopped,
            MINUTE.into(),
            Some(AgentStopReason::Owner),
        )
        .unwrap();
    store
        .save_agent(scheduled.revision, stopped.clone())
        .await
        .unwrap();
    let delete_operation = ControlOperation::new(ControlRequest::Delete(AgentDeleteRequest {
        target: AgentControlTarget::from(&stopped),
        requested_at: TWO_MINUTES.into(),
    }));
    let deleting = delete_operation.request.plan(stopped).unwrap().into_agent();
    let mut fields = agent_replacement_fields(&deleting).unwrap();
    fields.extend(delete_operation.receipt_update(&deleting).unwrap());
    store
        .agents
        .update_one(
            doc! {
                "_id": claimed.id.as_str(),
                "revision": i64::try_from(deleting.revision - 1).unwrap(),
            },
            doc! { "$set": fields },
        )
        .await
        .unwrap();
    let mut session = client.start_session().snapshot(true).await.unwrap();
    let deleting_snapshot =
        load_control_snapshot(&store, operation.request.target(), &mut session, true)
            .await
            .unwrap();
    assert!(control_snapshot_matches(
        &operation,
        &scheduled,
        &deleting_snapshot
    ));
    assert!(control_snapshot_matches(
        &delete_operation,
        &deleting,
        &deleting_snapshot
    ));

    let mut deleted = deleting.clone();
    deleted
        .transition(AgentStatus::Deleted, TWO_MINUTES.into(), None)
        .unwrap();
    store.save_agent(deleting.revision, deleted).await.unwrap();
    let mut session = client.start_session().snapshot(true).await.unwrap();
    let deleted_snapshot =
        load_control_snapshot(&store, operation.request.target(), &mut session, true)
            .await
            .unwrap();
    assert!(control_snapshot_matches(
        &operation,
        &scheduled,
        &deleted_snapshot
    ));
    assert!(control_snapshot_matches(
        &delete_operation,
        &deleting,
        &deleted_snapshot
    ));

    store
        .agents
        .update_one(
            doc! { "_id": claimed.id.as_str() },
            doc! { "$unset": { STOP_RECEIPT_FIELD: "" } },
        )
        .await
        .unwrap();
    let mut session = client.start_session().snapshot(true).await.unwrap();
    let no_receipt = load_control_snapshot(&store, operation.request.target(), &mut session, true)
        .await
        .unwrap();
    assert!(!control_snapshot_matches(
        &operation,
        &scheduled,
        &no_receipt
    ));
    assert!(control_snapshot_matches(
        &delete_operation,
        &deleting,
        &no_receipt
    ));

    for (collection, id) in [
        (&store.worktrees, claimed.worktree_id.as_str()),
        (&store.workspaces, claimed.workspace_id.as_str()),
        (&store.sandboxes, claimed.sandbox_id.as_str()),
    ] {
        let original = collection
            .find_one(doc! { "_id": id })
            .await
            .unwrap()
            .unwrap();
        collection
            .update_one(
                doc! { "_id": id },
                doc! { "$set": { "owner": "different-owner" } },
            )
            .await
            .unwrap();
        let mut session = client.start_session().snapshot(true).await.unwrap();
        assert!(matches!(
            load_control_snapshot(&store, operation.request.target(), &mut session, true,).await,
            Err(LineageError::Domain(AgentControlStoreError::Corrupt(_)))
        ));
        collection
            .replace_one(doc! { "_id": id }, original)
            .await
            .unwrap();
    }

    store
        .agents
        .delete_one(doc! { "_id": claimed.id.as_str() })
        .await
        .unwrap();
    let mut session = client.start_session().snapshot(true).await.unwrap();
    assert!(matches!(
        load_control_snapshot(&store, operation.request.target(), &mut session, true,).await,
        Err(LineageError::Domain(AgentControlStoreError::NotFound))
    ));

    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_control_parent_fence_conflicts_with_a_concurrent_replacement() {
    let (client, name) = database("agent_control_parent_fence").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (_, _, claimed) = claimed_agent_work_fixture(&store, "parent_fence").await;
    let target = AgentControlTarget::from(&claimed);
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let snapshot = load_control_snapshot(&store, &target, &mut session, false)
        .await
        .unwrap();

    let mut workspace = store
        .get_workspace(&claimed.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let expected_revision = workspace.revision;
    workspace.next_worktree_number += 1;
    workspace.revision += 1;
    store
        .save_workspace(expected_revision, workspace)
        .await
        .unwrap();

    match fence_control_lineage(
        &store,
        &snapshot,
        "ctl_00000000000000000000000000000000",
        &mut session,
    )
    .await
    {
        Err(LineageError::Domain(AgentControlStoreError::Conflict)) => {}
        Err(LineageError::Driver(error)) if is_transient(&error) => {}
        Err(_) => panic!("stale parent fence returned an unexpected error"),
        Ok(()) => panic!("stale parent fence must not commit"),
    }
    session.abort_transaction().await.unwrap();

    let worktree = store
        .worktrees
        .find_one(doc! { "_id": claimed.worktree_id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert!(worktree.get("last_agent_control_operation").is_none());

    client.database(&name).drop().await.unwrap();
}
