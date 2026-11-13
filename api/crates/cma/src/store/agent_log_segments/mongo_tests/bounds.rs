use super::*;
use crate::store::mongo_read_telemetry::{isolated_test, snapshot};
use mongodb::{event::command::CommandEvent, event::EventHandler, options::ClientOptions};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Trace {
    bound_probe: Option<Document>,
    read_commands: usize,
    source_rows: usize,
    source_bytes: usize,
    max_source_batch: usize,
    max_reply_bytes: usize,
}

impl Trace {
    fn observe(&mut self, event: CommandEvent) {
        if let CommandEvent::Started(event) = &event {
            if event.command.get_str("find").ok() == Some("agent_log") {
                let filter = event.command.get_document("filter").unwrap();
                if filter.contains_key("$or") {
                    self.bound_probe = Some(filter.clone());
                }
            }
        }
        if let CommandEvent::Succeeded(event) = event {
            if !matches!(event.command_name.as_str(), "find" | "getMore") {
                return;
            }
            self.read_commands += 1;
            self.max_reply_bytes = self
                .max_reply_bytes
                .max(bson::serialize_to_vec(&event.reply).unwrap().len());
            if let Ok(cursor) = event.reply.get_document("cursor") {
                if cursor
                    .get_str("ns")
                    .is_ok_and(|ns| ns.ends_with(".agent_log"))
                {
                    let rows = cursor
                        .get_array("firstBatch")
                        .or_else(|_| cursor.get_array("nextBatch"))
                        .unwrap();
                    self.source_rows += rows.len();
                    self.max_source_batch = self.max_source_batch.max(rows.len());
                    self.source_bytes += rows
                        .iter()
                        .map(|row| {
                            bson::serialize_to_vec(row.as_document().unwrap())
                                .unwrap()
                                .len()
                        })
                        .sum::<usize>();
                }
            }
        }
    }
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn sealed_pages_and_audit_units_have_bounded_work_as_history_grows() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let trace = Arc::new(Mutex::new(Trace::default()));
    let observed = trace.clone();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        observed.lock().unwrap().observe(event);
    }));
    let client = Client::with_options(options).unwrap();
    let mut baselines = [None; 3];
    let mut audit_baseline = None;
    for closed in [1, 10, 100, 1000] {
        let name = format!("cma_segments_bounds_{}", uuid::Uuid::new_v4().simple());
        let db = client.database(&name);
        schema::migrate(&db).await.unwrap();
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let (agent, _) = segmented_with_width(&store, closed, 200, 2).await;
        for (index, limit) in [20_u16, 100, 200].into_iter().enumerate() {
            *trace.lock().unwrap() = Trace::default();
            let started = std::time::Instant::now();
            let consumed = isolated_test(async {
                let mut delivered = 0;
                // Exercise the production streaming port without retaining a
                // page Vec, while setup may construct arbitrary fixture history.
                crate::store::agent_log_store::visit_page(
                    &store,
                    &agent.id,
                    0,
                    200,
                    limit,
                    false,
                    &mut |entry| {
                        delivered += 1;
                        assert_eq!(entry.seq, delivered);
                        Ok(())
                    },
                )
                .await
                .unwrap();
                assert_eq!(delivered, u64::from(limit));
                snapshot("agent_log")
            })
            .await;
            let reading = trace.lock().unwrap().clone();
            assert!(reading.read_commands <= 12);
            assert_eq!(&consumed[..3], &[1, 0, u64::from(limit)]);
            assert_eq!(reading.source_rows, usize::from(limit));
            assert_eq!(reading.source_bytes as u64, consumed[3]);
            assert!(reading.max_source_batch <= 64);
            assert!(reading.max_reply_bytes < 16 * 1024 * 1024 + 1024);
            // Explain the actual observed production probe. A constant reply
            // size alone would conceal a server-side lifetime scan.
            let explain = db
                .run_command(doc! {
                    "explain":{"find":"agent_log", "filter":reading.bound_probe.unwrap(),
                        "projection":{"_id":1}, "limit":1_i64},
                    "verbosity":"executionStats",
                })
                .await
                .unwrap();
            let stats = explain.get_document("executionStats").unwrap();
            assert_eq!(stats.get_i32("totalDocsExamined").unwrap(), 0);
            let keys = stats.get_i32("totalKeysExamined").unwrap();
            assert!(keys <= 2, "{stats:?}");
            eprintln!(
                "sealed_page_bounds segments={closed} page={limit} source_rows={} source_bytes={} read_commands={} max_source_batch={} max_reply_bytes={} probe_keys={keys} elapsed_us={}",
                consumed[2], consumed[3], reading.read_commands, reading.max_source_batch,
                reading.max_reply_bytes, started.elapsed().as_micros(),
            );
            if let Some(previous) = baselines[index] {
                assert_eq!(consumed, previous);
            } else {
                baselines[index] = Some(consumed);
            }
        }
        *trace.lock().unwrap() = Trace::default();
        let consumed = isolated_test(async {
            assert_eq!(
                segments::audit::run_unit(&client, &db, agent.id.as_str(), "scale", false)
                    .await
                    .unwrap(),
                "running"
            );
            snapshot("agent_log")
        })
        .await;
        let reading = trace.lock().unwrap().clone();
        assert_eq!(&consumed[..3], &[1, 0, 200]);
        assert_eq!(reading.source_rows, 200);
        assert_eq!(reading.source_bytes as u64, consumed[3]);
        assert!(reading.max_source_batch <= 512);
        assert!(reading.max_reply_bytes < 16 * 1024 * 1024 + 1024);
        assert!(reading.read_commands <= 30);
        let work = (consumed, reading.read_commands, reading.max_reply_bytes);
        if let Some(previous) = audit_baseline {
            assert_eq!(work, previous);
        } else {
            audit_baseline = Some(work);
        }
        let progress = collection(&store, AUDITS)
            .find_one(doc! {"audit_id":"scale"})
            .await
            .unwrap()
            .unwrap();
        let completed = progress.get_document("completed").unwrap();
        assert_eq!(completed.get_i64("next_ordinal").unwrap(), 1);
        assert_eq!(completed.get_i64("checked_entries").unwrap(), 200);
        eprintln!(
            "audit_unit_bounds segments={closed} source_rows={} source_bytes={} read_commands={} max_source_batch={} max_reply_bytes={}",
            consumed[2], consumed[3], reading.read_commands, reading.max_source_batch,
            reading.max_reply_bytes,
        );
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn failed_leaf_validation_accounts_for_consumed_source_without_a_lifetime_scan() {
    let (client, name) = database("read_failure").await;
    schema::migrate(&client.database(&name)).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, _) = segmented(&store, 3).await;
    store
        .agent_log
        .update_one(
            doc! {"agent_id":agent.id.as_str(),"seq":2_i64},
            doc! {"$set":{"record.appended_at":"2026-11-02T00:00:00Z"}},
        )
        .await
        .unwrap();
    isolated_test(async {
        assert!(store.agent_log_range(&agent.id, 0, 2, 2).await.is_err());
        let consumed = snapshot("agent_log");
        assert_eq!(&consumed[..3], &[1, 1, 2]);
        assert!(consumed[3] > 0);
    })
    .await;
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn global_history_exceeds_physical_segment_budget_without_a_lifetime_vector() {
    let (client, name) = database("lifetime").await;
    schema::migrate(&client.database(&name)).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, entries) = segmented_with_width(&store, 2, 10_000, 2).await;
    assert!(agent.conversation.last_seq > MAX_ENTRIES as u64);
    assert_eq!(
        store
            .agent_log_range(&agent.id, 9_998, 10_003, 5)
            .await
            .unwrap(),
        entries[9_998..10_003],
    );
    let mut seen = Vec::new();
    crate::store::agent_log_store::visit_page(
        &store,
        &agent.id,
        19_999,
        20_002,
        3,
        true,
        &mut |entry| {
            seen.push(entry);
            Ok(())
        },
    )
    .await
    .unwrap();
    assert_eq!(seen, entries[19_998..]);
    assert!(matches!(
        store
            .agent_log_prefix(&agent.id, agent.conversation.last_seq)
            .await,
        Err(layer2_core::sandbox::store::SandboxStoreError::AgentLogCapacityExceeded),
    ));
    client.database(&name).drop().await.unwrap();
}
