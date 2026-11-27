use super::*;
use crate::store::agent_log_integrity::parse_integrity;
use crate::store::agent_log_telemetry::{isolated_test as isolated, snapshot, ScanCaller};
use crate::store::test_measurement::{process_cpu, PoolUsage};
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
    let pool = Arc::new(Mutex::new(PoolUsage::default()));
    pool.lock().unwrap().set_limit(options.max_pool_size);
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
    let observed_pool = pool.clone();
    options.cmap_event_handler = Some(EventHandler::callback(move |event| {
        observed_pool.lock().unwrap().observe(&event);
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
    pool.lock().unwrap().reset();
    let cpu_before = process_cpu();
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
            assert_eq!(
                sends.load(Ordering::SeqCst),
                1,
                "message did not dispatch: {:?}",
                store
                    .get_command(&admitted.receipt.envelope.command_id)
                    .await
                    .unwrap()
            );
            if enabled {
                assert_eq!(
                    (snapshot(ScanCaller::CommandAdmission, true)[0]
                        + snapshot(ScanCaller::CommandReconciliation, true)[0]),
                    0
                );
                assert_eq!(snapshot(ScanCaller::ConversationPublication, true)[0], 0);
            } else {
                assert!(
                    (snapshot(ScanCaller::CommandAdmission, true)[0]
                        + snapshot(ScanCaller::CommandReconciliation, true)[0])
                        > 0
                );
            }
        })
        .await;
        crate::store::mongo_read_telemetry::snapshot("agent_log")
    })
    .await;
    metrics.lock().unwrap().enabled = false;
    let elapsed = started.elapsed();
    let cpu = process_cpu() - cpu_before;
    let pool = pool.lock().unwrap().report();
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
    println!(
        "TC046_MESSAGE_RESOURCES {}",
        serde_json::json!({
            "history":count,"enabled":enabled,"wall_us":elapsed.as_micros(),
            "fixture_process_cpu_us":cpu.as_micros(),"docs_examined":total,"pool":pool,
        })
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
            assert_eq!(
                (snapshot(ScanCaller::CommandAdmission, true)[0]
                    + snapshot(ScanCaller::CommandReconciliation, true)[0]),
                0
            );
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
