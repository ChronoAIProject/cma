use super::super::work_tests::message_mutation;
use super::bounded_admission_tests::prepare_history;
use super::*;
use crate::store::agent_log_integrity::INTEGRITY_FIELD;
use crate::store::agent_log_telemetry::{isolated_test as isolated, snapshot, ScanCaller};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn newer_turn_between_claim_and_dispatch_invalidates_after_turn_target() {
    let (client, name) = database("bounded_newer_turn").await;
    let (legacy, agent, parent) = fixture(&name, &client).await;
    let (store, agent) = prepare_history(legacy, agent, 1000).await;
    let mut request = live_request(&agent, &parent, "after-previous-turn");
    let AgentCommand::Message { order, .. } = &mut request.envelope.command else {
        unreachable!()
    };
    *order = AgentInputOrder::AfterTurn {
        turn_sequence: Some(997),
    };
    request.envelope.request_fingerprint = request.envelope.command.fingerprint().unwrap();
    store.reserve_command(request).await.unwrap();
    let claimed = store
        .claim_next_command(claim_request(&agent, "claim", &layer2_core::now_rfc3339()))
        .await
        .unwrap()
        .unwrap();
    // An independent conversation writer completes a newer turn after the
    // command was claimed. The old request must not bind to this newer state.
    let mut newer = agent.clone();
    newer.revision += 1;
    newer.conversation.last_seq += 4;
    store
        .commit_agent_conversation(AgentConversationMutation {
            settings_fence: None,
            command_claim: None,
            execution_fence: None,
            trigger_execution_fence: None,
            admission_guard: Some(SandboxMutationGuard::capture(&parent)),
            expected_agent_revision: agent.revision,
            expected_last_seq: agent.conversation.last_seq,
            runtime_generation: 1,
            agent: newer.clone(),
            appended_at: NOW.into(),
            agent_logs: vec![
                AgentLogKind::TurnStarted {
                    turn_sequence: 1001,
                    origin_key: None,
                    model: None,
                },
                AgentLogKind::OwnerMessage {
                    content: "new question".into(),
                },
                AgentLogKind::AgentMessage {
                    content: "new answer".into(),
                },
                AgentLogKind::TurnCompleted {
                    turn_sequence: 1001,
                },
            ],
        })
        .await
        .unwrap();
    isolated(async {
        let mutation =
            message_mutation(&newer, &parent, &claimed.work.lease.as_ref().unwrap().claim);
        assert_eq!(
            store.commit_agent_conversation(mutation).await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(snapshot(ScanCaller::ConversationPublication, true)[0], 0);
        assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(newer));
        let row = store
            .agent_commands
            .find_one(doc! {
                "_id": claimed.receipt.envelope.command_id.as_str()
            })
            .await
            .unwrap()
            .unwrap();
        assert!(crate::store::agent_commands::read_command(&row)
            .unwrap()
            .work
            .dispatch
            .is_none());
    })
    .await;
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn latest_started_turn_survives_all_outcomes_and_restart_at_dispatch() {
    for outcome in ["completed", "failed", "interrupted", "empty"] {
        for target in [Some(997), Some(996), None] {
            let (client, name) = database("bounded_latest_turn").await;
            let (legacy, agent, parent) = fixture(&name, &client).await;
            let (store, agent) = prepare_history(legacy, agent, 1000).await;
            // Construct each legacy history before auditing the prepared cohort.
            let kind = match outcome {
                "failed" => AgentLogKind::TurnFailed {
                    turn_sequence: 997,
                    failure: TypedFailure {
                        class: "test".into(),
                        detail: "test".into(),
                    },
                },
                "interrupted" => AgentLogKind::TurnInterrupted { turn_sequence: 997 },
                _ => AgentLogKind::TurnCompleted { turn_sequence: 997 },
            };
            store
                .agent_log
                .replace_one(
                    doc! { "agent_id": agent.id.as_str(), "seq": 1000_i64 },
                    log_document(&AgentLogEntry {
                        agent_id: agent.id.clone(),
                        seq: 1000,
                        appended_at: NOW.into(),
                        kind,
                    })
                    .unwrap(),
                )
                .await
                .unwrap();
            if outcome == "empty" {
                store
                    .agent_log
                    .update_one(
                        doc! { "agent_id": agent.id.as_str(), "seq": 999_i64 },
                        doc! { "$set": { "record.kind.content": "   " } },
                    )
                    .await
                    .unwrap();
            }
            store
                .agents
                .update_one(
                    doc! { "_id": agent.id.as_str() },
                    doc! { "$unset": { INTEGRITY_FIELD: "" } },
                )
                .await
                .unwrap();
            let mut request = live_request(&agent, &parent, "after-turn");
            let AgentCommand::Message { order, .. } = &mut request.envelope.command else {
                unreachable!()
            };
            *order = AgentInputOrder::AfterTurn {
                turn_sequence: target,
            };
            request.envelope.request_fingerprint = request.envelope.command.fingerprint().unwrap();
            store.reserve_command(request).await.unwrap();
            let claimed = store
                .claim_next_command(claim_request(&agent, "claim", &layer2_core::now_rfc3339()))
                .await
                .unwrap()
                .unwrap();
            let restarted = MongoSandboxStore::open(&client, &name)
                .await
                .unwrap()
                .prepare_bounded_admission(std::slice::from_ref(&agent.id))
                .await
                .unwrap();
            isolated(async {
                let mutation =
                    message_mutation(&agent, &parent, &claimed.work.lease.as_ref().unwrap().claim);
                let result = restarted.commit_agent_conversation(mutation).await;
                assert_eq!(
                    result.is_ok(),
                    target == Some(997),
                    "{outcome} {target:?}: {result:?}"
                );
                assert_eq!(snapshot(ScanCaller::CommandAdmission, true)[0], 0);
            })
            .await;
            client.database(&name).drop().await.unwrap();
        }
    }
}

// The complete evidence fold is an independent oracle for the new prefix entry
// point, including terminal outcomes after the actual worker dispatch.
pub(super) async fn assert_suffix_equivalence(store: &MongoSandboxStore, id: &AgentId) {
    let row = store
        .agent_commands
        .find_one(doc! { "agent_id": id.as_str() })
        .await
        .unwrap()
        .unwrap();
    let command = crate::store::agent_commands::read_command(&row).unwrap();
    let dispatch = command.work.dispatch.as_ref().unwrap();
    let lane_row = store
        .agent_command_orders
        .find_one(doc! { "_id": id.as_str() })
        .await
        .unwrap()
        .unwrap();
    let lane = crate::store::agent_commands::read_command_lane(&lane_row, id.as_str()).unwrap();
    let agent = store.get_agent(id).await.unwrap().unwrap();
    let rows: Vec<Document> = store
        .agent_log
        .find(doc! { "agent_id": id.as_str() })
        .sort(doc! { "seq": 1 })
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let entries: Vec<_> = rows
        .iter()
        .map(|row| crate::store::agent_log::read_entry(row).unwrap())
        .collect();
    for outcome in [
        None,
        Some(AgentLogKind::TurnCompleted {
            turn_sequence: dispatch.turn_sequence,
        }),
        Some(AgentLogKind::TurnInterrupted {
            turn_sequence: dispatch.turn_sequence,
        }),
        Some(AgentLogKind::TurnFailed {
            turn_sequence: dispatch.turn_sequence,
            failure: TypedFailure {
                class: "test".into(),
                detail: "test".into(),
            },
        }),
    ] {
        let mut entries = entries.clone();
        let mut agent = agent.clone();
        if let Some(kind) = outcome {
            agent.conversation.last_seq += 1;
            agent.conversation.active_turn = None;
            agent.status = AgentStatus::Ready;
            entries.push(AgentLogEntry {
                agent_id: id.clone(),
                seq: agent.conversation.last_seq,
                appended_at: NOW.into(),
                kind,
            });
        }
        let mut full = CommandEvidence::new(&command).unwrap();
        let mut bounded = CommandEvidence::from_trusted_message_prefix(&command).unwrap();
        for entry in &entries {
            full.observe(entry).unwrap();
            if entry.seq >= dispatch.operation_sequence {
                bounded.observe(entry).unwrap();
            }
        }
        let (mut full_command, mut bounded_command) = (command.clone(), command.clone());
        let (mut full_lane, mut bounded_lane) = (lane.clone(), lane.clone());
        assert_eq!(
            full.apply(&mut full_command, &mut full_lane, &agent),
            bounded.apply(&mut bounded_command, &mut bounded_lane, &agent)
        );
        assert_eq!(full_command, bounded_command);
        assert_eq!(full_lane, bounded_lane);
    }
    let mut broken = CommandEvidence::from_trusted_message_prefix(&command).unwrap();
    let fence = entries
        .iter()
        .find(|entry| entry.seq == dispatch.operation_sequence)
        .unwrap();
    let mut wrong = fence.clone();
    wrong.appended_at = TWO_MINUTES.into();
    assert_eq!(broken.observe(&wrong), Err(CommandLedgerError::Corrupt));
}
