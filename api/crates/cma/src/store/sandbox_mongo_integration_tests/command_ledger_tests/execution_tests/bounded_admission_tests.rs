use super::*;
use crate::store::agent_log_integrity::{parse_integrity, INTEGRITY_FIELD, QUARANTINE_FIELD};
use crate::store::agent_log_telemetry::{isolated_test as isolated, snapshot, ScanCaller};
use crate::store::agents::agent_replacement_fields;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn zero_head_probes_all_rows_including_malformed_sequences() {
    for orphan in [
        None,
        Some(Bson::Int64(1)),
        Some(Bson::Int64(2)),
        Some(Bson::Int64(0)),
        Some(Bson::Int64(-1)),
        Some(Bson::String("malformed".into())),
    ] {
        let (client, name) = database("bounded_zero_head").await;
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let mut agent = ready_agent(&active_sandbox("sbx_zero"), "agt_zero");
        agent.conversation.last_seq = 0;
        agent.conversation.conversation_id = None;
        agent.conversation.runtime_generation = None;
        store.create_agent(agent.clone()).await.unwrap();
        if let Some(seq) = &orphan {
            store
                .agent_log
                .insert_one(doc! { "_id": "orphan", "document_schema_version": 1_i64,
                "agent_id": agent.id.as_str(), "seq": seq.clone(), "record": {} })
                .bypass_document_validation(true)
                .await
                .unwrap();
        }
        let result = store
            .append_agent_log(
                &agent.id,
                NOW.into(),
                AgentLogKind::Created {
                    workspace_id: agent.workspace_id.clone(),
                    worktree_id: agent.worktree_id.clone(),
                },
            )
            .await;
        assert_eq!(result.is_ok(), orphan.is_none(), "orphan={orphan:?}");
        let current = store.get_agent(&agent.id).await.unwrap().unwrap();
        assert_eq!(current.conversation.last_seq, u64::from(orphan.is_none()));
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn audit_detects_middle_damage_and_quarantines_across_restart() {
    let (client, name) = database("bounded_middle_audit").await;
    let (legacy, agent, _) = fixture(&name, &client).await;
    let (store, agent) = prepare_history(legacy, agent, 1000).await;
    store
        .agent_log
        .delete_one(doc! { "agent_id": agent.id.as_str(), "seq": 500_i64 })
        .await
        .unwrap();
    // The tail proof is intentionally not advertised as a proof of middle bytes.
    assert!(store
        .clone()
        .prepare_bounded_admission(std::slice::from_ref(&agent.id))
        .await
        .is_err());
    let restarted = MongoSandboxStore::open(&client, &name).await.unwrap();
    let row = restarted
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert!(row.contains_key(QUARANTINE_FIELD));
    assert!(restarted
        .append_agent_log(
            &agent.id,
            NOW.into(),
            AgentLogKind::WorkStepCompleted {
                step: AgentWorkStep::None
            }
        )
        .await
        .is_err());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn abort_and_concurrent_appends_keep_metadata_atomic() {
    use crate::store::sandbox_mongo::start_transaction;
    let (client, name) = database("bounded_atomicity").await;
    let (legacy, agent, _) = fixture(&name, &client).await;
    let (store, agent) = prepare_history(legacy, agent, 1000).await;
    let filter = doc! { "_id": agent.id.as_str() };
    let before = store
        .agents
        .find_one(filter.clone())
        .await
        .unwrap()
        .unwrap();
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let kind = AgentLogKind::WorkStepCompleted {
        step: AgentWorkStep::None,
    };
    assert!(crate::store::agent_log::append_in_transaction(
        &store,
        &mut session,
        &agent.id,
        NOW.into(),
        kind.clone()
    )
    .await
    .is_ok());
    session.abort_transaction().await.unwrap();
    assert_eq!(
        store
            .agents
            .find_one(filter.clone())
            .await
            .unwrap()
            .unwrap(),
        before
    );
    assert_eq!(
        store
            .agent_log
            .count_documents(doc! { "agent_id": agent.id.as_str() })
            .await
            .unwrap(),
        1000
    );
    let (a, b) = tokio::join!(
        store.append_agent_log(&agent.id, NOW.into(), kind.clone()),
        store.append_agent_log(&agent.id, NOW.into(), kind)
    );
    let mut sequences = [a.unwrap().seq, b.unwrap().seq];
    sequences.sort();
    assert_eq!(sequences, [1001, 1002]);
    let raw: Vec<mongodb::bson::RawDocumentBuf> = store
        .agent_log
        .clone_with_type()
        .find(doc! { "agent_id": agent.id.as_str() })
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let after = store.agents.find_one(filter).await.unwrap().unwrap();
    let integrity = parse_integrity(&after).unwrap().unwrap();
    assert_eq!(integrity.entry_count, 1002);
    assert_eq!(
        integrity.encoded_bytes,
        raw.iter().map(|row| row.as_bytes().len()).sum::<usize>()
    );
    // Restart/audit proves the exact raw tail hash and persisted reducer too.
    store
        .prepare_bounded_admission(std::slice::from_ref(&agent.id))
        .await
        .unwrap();
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn successful_last_normal_append_and_full_capacity_refusal_stay_bounded() {
    let (client, name) = database("bounded_capacity").await;
    let (legacy, agent, _) = fixture(&name, &client).await;
    let (store, agent) = prepare_history(legacy, agent, 9999).await;
    isolated(async {
        let kind = AgentLogKind::WorkStepCompleted {
            step: AgentWorkStep::None,
        };
        assert_eq!(
            store
                .append_agent_log(&agent.id, NOW.into(), kind.clone())
                .await
                .unwrap()
                .seq,
            10000
        );
        assert_eq!(
            store.append_agent_log(&agent.id, NOW.into(), kind).await,
            Err(SandboxStoreError::AgentLogCapacityExceeded)
        );
        assert_eq!(snapshot(ScanCaller::Other, true)[0], 0);
    })
    .await;
    client.database(&name).drop().await.unwrap();
}

pub(super) async fn prepare_history(
    store: MongoSandboxStore,
    mut agent: Agent,
    count: u64,
) -> (MongoSandboxStore, Agent) {
    assert!(count >= 6);
    // Replace the legacy test helper's deliberately incomplete first event.
    store
        .agent_log
        .delete_many(doc! { "agent_id": agent.id.as_str() })
        .await
        .unwrap();
    let mut kinds = vec![
        AgentLogKind::Created {
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.creation_worktree_id().clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: agent.conversation.conversation_id.clone().unwrap(),
            runtime_generation: 1,
        },
    ];
    kinds.resize(
        count as usize - 4,
        AgentLogKind::WorkStepCompleted {
            step: AgentWorkStep::None,
        },
    );
    kinds.extend([
        AgentLogKind::TurnStarted {
            turn_sequence: count - 3,
            origin_key: None,
            model: None,
        },
        AgentLogKind::OwnerMessage {
            content: "question".into(),
        },
        AgentLogKind::AgentMessage {
            content: "answer".into(),
        },
        AgentLogKind::TurnCompleted {
            turn_sequence: count - 3,
        },
    ]);
    let rows = kinds
        .into_iter()
        .enumerate()
        .map(|(index, kind)| {
            log_document(&AgentLogEntry {
                agent_id: agent.id.clone(),
                seq: index as u64 + 1,
                appended_at: NOW.into(),
                kind,
            })
            .unwrap()
        })
        .collect::<Vec<_>>();
    store.agent_log.insert_many(rows).await.unwrap();
    agent.conversation.last_seq = count;
    agent.revision += 1;
    let mut fields = agent_replacement_fields(&agent).unwrap();
    fields.insert("last_seq", count as i64);
    store
        .agents
        .update_one(
            doc! { "_id": agent.id.as_str() },
            // This fixture replaces the legacy history before initialization.
            // The old helper's tail hash cannot describe the replacement rows.
            doc! { "$set": fields, "$unset": { INTEGRITY_FIELD: "" } },
        )
        .await
        .unwrap();
    let store = store
        .prepare_bounded_admission(std::slice::from_ref(&agent.id))
        .await
        .unwrap();
    (store, agent)
}

#[derive(Default, Debug)]
struct MessageMetrics {
    enabled: bool,
    commands: std::collections::BTreeMap<String, u64>,
    checkout_total_us: u128,
    checkout_max_us: u128,
}

async fn profiled_message(count: u64, enabled: bool) {
    let metrics = Arc::new(Mutex::new(MessageMetrics::default()));
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let observed = metrics.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            let mut values = observed.lock().unwrap();
            if values.enabled {
                *values.commands.entry(event.command_name).or_default() += 1;
            }
        }
    }));
    let observed = metrics.clone();
    options.cmap_event_handler = Some(EventHandler::callback(move |event| {
        if let mongodb::event::cmap::CmapEvent::ConnectionCheckedOut(event) = event {
            let mut values = observed.lock().unwrap();
            if values.enabled {
                values.checkout_total_us += event.duration.as_micros();
                values.checkout_max_us = values.checkout_max_us.max(event.duration.as_micros());
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let name = format!("cma_bounded_profile_{}", uuid::Uuid::new_v4().simple());
    let (store, agent, parent, execution, _, sends) = composed_with_history(
        &client,
        &name,
        Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        active_sandbox("sbx_commands"),
        Some((count, enabled)),
    )
    .await;
    let mut request = live_request(&agent, &parent, "bounded-message");
    AnswerContext {
        turn_sequence: count - 3,
        answer_sequence: count - 1,
    }
    .bind(&mut request.envelope.command)
    .unwrap();
    request.envelope.request_fingerprint = request.envelope.command.fingerprint().unwrap();
    let db = client.database(&name);
    db.run_command(doc! { "profile": 2, "slowms": 0 })
        .await
        .unwrap();
    let started = std::time::Instant::now();
    metrics.lock().unwrap().enabled = true;
    let consumed = crate::store::mongo_read_telemetry::isolated_test(async {
        isolated(async {
            let admitted = record(store.reserve_command(request.clone()).await.unwrap());
            // Same-key response loss does not repeat admission or dispatch.
            assert_eq!(
                record(store.reserve_command(request).await.unwrap()).receipt,
                admitted.receipt
            );
            AgentCommandWorker::new(store.clone(), execution, "bounded-worker".into())
                .unwrap()
                .run_one(&admitted.receipt.envelope.command_id)
                .await
                .unwrap();
            if sends.load(Ordering::SeqCst) != 1 {
                panic!(
                    "message did not dispatch: {:?}",
                    store
                        .get_command(&admitted.receipt.envelope.command_id)
                        .await
                        .unwrap()
                );
            }
            if enabled {
                assert_eq!(snapshot(ScanCaller::CommandAdmission, true)[0], 0);
                assert_eq!(snapshot(ScanCaller::ConversationPublication, true)[0], 0);
            } else {
                assert!(snapshot(ScanCaller::CommandAdmission, true)[0] > 0);
            }
        })
        .await;
        crate::store::mongo_read_telemetry::snapshot("agent_log")
    })
    .await;
    metrics.lock().unwrap().enabled = false;
    let elapsed = started.elapsed();
    db.run_command(doc! { "profile": 0 }).await.unwrap();
    super::bounded_dispatch_tests::assert_suffix_equivalence(&store, &agent.id).await;
    let rows: Vec<Document> = db
        .collection("system.profile")
        .find(doc! {})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    // Keep raw local profiler evidence private; stdout contains only counters.
    let evidence =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/admission-evidence");
    std::fs::create_dir_all(&evidence).unwrap();
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(evidence.join(format!("profile-{count}-{enabled}.json")))
        .unwrap();
    serde_json::to_writer(file, &rows).unwrap();
    let mut examined = std::collections::BTreeMap::<String, i64>::new();
    for row in &rows {
        let value = row
            .get_i64("docsExamined")
            .or_else(|_| row.get_i32("docsExamined").map(i64::from))
            .unwrap_or(0);
        *examined
            .entry(row.get_str("ns").unwrap_or("command").into())
            .or_default() += value;
    }
    let total: i64 = examined.values().sum();
    println!("TC-046 history={count} operations={} total_docs_examined={total} by_collection={examined:?}", rows.len());
    println!(
        "TC-046 enabled={enabled} elapsed_us={} agent_log_consumed={consumed:?} metrics={:?}",
        elapsed.as_micros(),
        metrics.lock().unwrap()
    );
    if enabled {
        assert!(
            total <= 100,
            "whole admission/dispatch/settlement exceeded TC-046: {examined:?}"
        );
    } else {
        assert!(
            total > count as i64,
            "baseline did not exercise retained-history scans"
        );
    }
    let row = store
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    let integrity = parse_integrity(&row).unwrap().unwrap();
    assert_eq!(integrity.head_seq as i64, row.get_i64("last_seq").unwrap());
    assert_eq!(
        integrity.bound_agent_revision as i64,
        row.get_i64("revision").unwrap()
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set with profiler access"]
async fn real_message_admission_dispatch_and_settlement_are_bounded() {
    for count in [1_000, 9_990] {
        profiled_message(count, false).await;
        profiled_message(count, true).await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn repeated_real_messages_keep_trust_after_revision_writes_and_restart() {
    use layer3_adapter::{
        ContentBlock, ContentChunk, ConversationUpdate, PromptResponse, SessionUpdate,
        TurnStopReason,
    };
    let (client, name) = database("bounded_repeated_messages").await;
    let (mut store, agent, _, execution, _, sends) = composed_with_history(
        &client,
        &name,
        Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        active_sandbox("sbx_commands"),
        Some((1000, true)),
    )
    .await;
    for attempt in 0..3 {
        let mut current = store.get_agent(&agent.id).await.unwrap().unwrap();
        let revision = current.revision;
        current.revision += 1;
        store.save_agent(revision, current.clone()).await.unwrap();
        let row = store
            .agents
            .find_one(doc! { "_id": agent.id.as_str() })
            .await
            .unwrap()
            .unwrap();
        let checkpoint = parse_integrity(&row).unwrap().unwrap();
        assert_eq!(checkpoint.bound_agent_revision, current.revision);
        let parent = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
        let mut request = live_request(&current, &parent, &format!("message-{attempt}"));
        checkpoint
            .semantic
            .answer_context()
            .unwrap()
            .bind(&mut request.envelope.command)
            .unwrap();
        request.envelope.request_fingerprint = request.envelope.command.fingerprint().unwrap();
        let admitted = isolated(async {
            let admitted = record(store.reserve_command(request).await.unwrap());
            AgentCommandWorker::new(store.clone(), execution.clone(), "repeat-worker".into())
                .unwrap()
                .run_one(&admitted.receipt.envelope.command_id)
                .await
                .unwrap();
            assert_eq!(sends.load(Ordering::SeqCst), attempt + 1);
            assert_eq!(snapshot(ScanCaller::CommandAdmission, true)[0], 0);
            assert_eq!(snapshot(ScanCaller::ConversationPublication, true)[0], 0);
            admitted
        })
        .await;
        let recorder = DurableAgentUpdateRecorder::new(store.clone());
        let mut assembly = AgentMessageAssembly::default();
        for update in [
            ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(ContentChunk::new(
                ContentBlock::from("answer"),
            ))),
            ConversationUpdate::TurnEnded(PromptResponse::new(TurnStopReason::EndTurn)),
        ] {
            recorder
                .record_update(
                    &agent.id,
                    1,
                    &mut assembly,
                    update,
                    layer2_core::now_rfc3339(),
                )
                .await
                .unwrap();
        }
        assert_eq!(
            store
                .reconcile_command(&admitted.receipt.envelope.command_id)
                .await
                .unwrap()
                .status,
            AgentResponseStatus::Completed
        );
        store = Arc::new(
            MongoSandboxStore::open(&client, &name)
                .await
                .unwrap()
                .prepare_bounded_admission(std::slice::from_ref(&agent.id))
                .await
                .unwrap(),
        );
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn tail_tamper_cannot_be_repaired_by_a_successful_structural_scan() {
    let (client, name) = database("bounded_tail_tamper").await;
    let (legacy, agent, _) = fixture(&name, &client).await;
    let (store, agent) = prepare_history(legacy, agent, 1000).await;
    let before = store
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    store
        .agent_log
        .update_one(
            doc! { "agent_id": agent.id.as_str(), "seq": 1000_i64 },
            doc! { "$set": { "record.appended_at": TWO_MINUTES } },
        )
        .await
        .unwrap();
    for _ in 0..2 {
        assert!(store
            .append_agent_log(
                &agent.id,
                NOW.into(),
                AgentLogKind::WorkStepCompleted {
                    step: AgentWorkStep::None,
                }
            )
            .await
            .is_err());
    }
    let row = store
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert!(row.contains_key(QUARANTINE_FIELD));
    assert_eq!(row.get_i64("last_seq").unwrap(), 1000);
    assert_eq!(row.get(INTEGRITY_FIELD), before.get(INTEGRITY_FIELD));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn revision_carry_never_promotes_an_old_writer_checkpoint() {
    let (client, name) = database("bounded_revision").await;
    let (legacy, agent, _) = fixture(&name, &client).await;
    let (store, mut agent) = prepare_history(legacy, agent, 1000).await;
    let revision = agent.revision;
    agent.revision += 1;
    store.save_agent(revision, agent.clone()).await.unwrap();
    let row = store
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        parse_integrity(&row).unwrap().unwrap().bound_agent_revision,
        agent.revision
    );
    // Generation-173-shaped revision-only $set leaves the block bound to the old revision.
    let mut next = agent.clone();
    next.revision += 1;
    store
        .agents
        .update_one(
            doc! { "_id": agent.id.as_str() },
            doc! { "$set": agent_replacement_fields(&next).unwrap() },
        )
        .await
        .unwrap();
    let revision = next.revision;
    next.revision += 1;
    store.save_agent(revision, next.clone()).await.unwrap();
    let row = store
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        parse_integrity(&row).unwrap().unwrap().bound_agent_revision,
        agent.revision
    );
    isolated(async {
        store
            .append_agent_log(
                &agent.id,
                NOW.into(),
                AgentLogKind::WorkStepCompleted {
                    step: AgentWorkStep::None,
                },
            )
            .await
            .unwrap();
        assert_eq!(snapshot(ScanCaller::Other, true)[0], 1);
        store
            .append_agent_log(
                &agent.id,
                NOW.into(),
                AgentLogKind::WorkStepCompleted {
                    step: AgentWorkStep::None,
                },
            )
            .await
            .unwrap();
        assert_eq!(snapshot(ScanCaller::Other, true)[0], 1);
    })
    .await;
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn insert_failure_after_staged_metadata_aborts_the_entire_append() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let app_name = format!("cma-admission-atomic-{suffix}");
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    let client = Client::with_options(options).unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_admission_atomic_{suffix}");
    let (legacy, agent, _) = fixture(&name, &client).await;
    let (store, agent) = prepare_history(legacy, agent, 1000).await;
    let filter = doc! { "_id": agent.id.as_str() };
    let before = store
        .agents
        .find_one(filter.clone())
        .await
        .unwrap()
        .unwrap();
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": { "times": 1_i32 },
            "data": { "failCommands": ["insert"], "appName": app_name, "errorCode": 13_i32 }
        })
        .await
        .unwrap();
    let kind = AgentLogKind::WorkStepCompleted {
        step: AgentWorkStep::None,
    };
    let result = store
        .append_agent_log(&agent.id, NOW.into(), kind.clone())
        .await;
    admin
        .database("admin")
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await
        .unwrap();
    assert!(result.is_err());
    assert_eq!(
        store.agents.find_one(filter).await.unwrap().unwrap(),
        before
    );
    assert_eq!(
        store
            .agent_log
            .count_documents(doc! { "agent_id": agent.id.as_str() })
            .await
            .unwrap(),
        1000
    );
    assert_eq!(
        store
            .append_agent_log(&agent.id, NOW.into(), kind)
            .await
            .unwrap()
            .seq,
        1001
    );
    client.database(&name).drop().await.unwrap();
}
