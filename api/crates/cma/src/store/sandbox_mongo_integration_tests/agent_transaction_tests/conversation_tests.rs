use super::*;

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
    assert!(matches!(
        reconcile_conversation(&store, &receipt).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    let deleted = store
        .agent_log
        .delete_one(doc! {
            "agent_id": orphan.agent_id.as_str(),
            "seq": i64::try_from(orphan.seq).unwrap(),
        })
        .await
        .unwrap();
    assert_eq!(deleted.deleted_count, 1);
    reset_repaired_fixture(&store, &receipt.agent).await;

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
    reset_repaired_fixture(&store, &receipt.agent).await;

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
    reset_repaired_fixture(&store, &receipt.agent).await;

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
    assert!(matches!(
        reopened.pending_agent_approvals(&working.id).await,
        Err(SandboxStoreError::Corrupt(_))
    ));

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
    assert!(matches!(
        reopened.commit_agent_conversation(resume.clone()).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    reopened
        .agent_log
        .delete_one(doc! {
            "agent_id": orphan.agent_id.as_str(),
            "seq": 6_i64,
        })
        .await
        .unwrap();
    reset_repaired_fixture(&reopened, &working).await;

    reopened
        .agent_log
        .delete_one(doc! { "agent_id": working.id.as_str(), "seq": 4_i64 })
        .await
        .unwrap();
    assert_eq!(
        reopened.pending_agent_approvals(&working.id).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert!(matches!(
        reopened.commit_agent_conversation(resume).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    assert!(matches!(
        reopened
            .append_agent_log(
                &working.id,
                TWO_MINUTES.into(),
                AgentLogKind::AgentMessage {
                    content: "must not append after a gap".into(),
                },
            )
            .await,
        Err(SandboxStoreError::Corrupt(_))
    ));

    client.database(&name).drop().await.unwrap();
}

// Each synthetic corruption is restored and audited before the next case.
// Ordinary reads/writes never clear the durable marker by restoring row bytes.
async fn reset_repaired_fixture(store: &MongoSandboxStore, agent: &Agent) {
    use crate::store::agent_log_integrity::{scan_collection, QUARANTINE_FIELD};
    use crate::store::agent_log_telemetry::ScanCaller;
    assert!(matches!(
        store.agent_log(&agent.id, 0, 20).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    let mut session = store.client.start_session().snapshot(true).await.unwrap();
    assert!(scan_collection(
        ScanCaller::RecoveryProof,
        &store.agent_log,
        &agent.id,
        agent.conversation.last_seq,
        &mut session,
        true,
        |_| Ok(())
    )
    .await
    .is_ok());
    store
        .agents
        .update_one(
            doc! { "_id": agent.id.as_str() },
            doc! { "$unset": { QUARANTINE_FIELD: "" } },
        )
        .await
        .unwrap();
}
