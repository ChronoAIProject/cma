use super::*;
use crate::store::mongo_read_telemetry::{isolated_test, snapshot};
use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::{event::command::CommandEvent, event::EventHandler, options::ClientOptions};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Trace {
    probes: Vec<Document>,
    catalog_commands: usize,
    read_commands: usize,
    source_rows: usize,
    source_bytes: usize,
    max_source_batch: usize,
    max_reply_bytes: usize,
}

impl Trace {
    fn observe(&mut self, event: CommandEvent) {
        if let CommandEvent::Started(event) = &event {
            if event.command.get_str("listIndexes").ok() == Some("agent_log") {
                self.catalog_commands += 1;
            }
            if event.command.get_str("find").ok() == Some("agent_log")
                && event
                    .command
                    .get_document("projection")
                    .is_ok_and(|p| p.contains_key("found"))
            {
                let mut command = event.command.clone();
                // Explain the actual query options, excluding transport/session
                // context: explain is a separate operation after the timed read.
                for key in [
                    "$db",
                    "$clusterTime",
                    "lsid",
                    "readConcern",
                    "txnNumber",
                    "autocommit",
                    "startTransaction",
                ] {
                    command.remove(key);
                }
                self.probes.push(command);
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
        let (agent, entries) = segmented_with_width(&store, closed, 200, 2).await;
        certify(&store, &agent, &entries).await;
        store
            .agent_log
            .insert_many((0..1000).map(|i| {
                doc! {
                    "_id":format!("foreign:{i}"),"agent_id":format!("foreign:{i}"),"seq":null,
                }
            }))
            .bypass_document_validation(true)
            .await
            .unwrap();
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
            let page_elapsed = started.elapsed();
            let reading = trace.lock().unwrap().clone();
            assert!(reading.read_commands <= 15);
            assert_eq!(reading.catalog_commands, 2);
            assert_eq!(reading.probes.len(), 4);
            assert_eq!(&consumed[..3], &[1, 0, u64::from(limit)]);
            assert_eq!(reading.source_rows, usize::from(limit));
            assert_eq!(reading.source_bytes as u64, consumed[3]);
            assert!(reading.max_source_batch <= 64);
            assert!(reading.max_reply_bytes < 16 * 1024 * 1024 + 1024);
            let explain_started = std::time::Instant::now();
            let (keys, docs) = explain_probes(&db, &reading).await;
            eprintln!(
                "sealed_page_bounds segments={closed} page={limit} source_rows={} source_bytes={} read_commands={} catalog_commands={} max_source_batch={} max_reply_bytes={} probe_keys={keys} probe_docs={docs} elapsed_us={} explain_us={}",
                consumed[2], consumed[3], reading.read_commands, reading.catalog_commands, reading.max_source_batch,
                reading.max_reply_bytes, page_elapsed.as_micros(), explain_started.elapsed().as_micros(),
            );
            if let Some(previous) = baselines[index] {
                assert_eq!(consumed, previous);
            } else {
                baselines[index] = Some(consumed);
            }
        }
        *trace.lock().unwrap() = Trace::default();
        let started = std::time::Instant::now();
        let mut session = client.start_session().snapshot(true).await.unwrap();
        let proof = segments::capture::capture(
            &store,
            &agent,
            &mut session,
            segments::capture::CaptureMode::Snapshot,
        )
        .await
        .unwrap();
        assert_eq!(proof.latest_entry(), entries.last());
        let elapsed = started.elapsed();
        let reading = trace.lock().unwrap().clone();
        assert_eq!(reading.source_rows, 2);
        assert_eq!(reading.catalog_commands, 1);
        assert_eq!(reading.probes.len(), 2);
        assert!(reading.read_commands <= 12);
        let explain_started = std::time::Instant::now();
        let (keys, docs) = explain_probes(&db, &reading).await;
        eprintln!("capture_key_bounds segments={closed} source_rows={} source_bytes={} read_commands={} catalog_commands={} max_reply_bytes={} probe_keys={keys} probe_docs={docs} elapsed_us={} explain_us={}",
            reading.source_rows, reading.source_bytes, reading.read_commands, reading.catalog_commands, reading.max_reply_bytes,
            elapsed.as_micros(), explain_started.elapsed().as_micros());
        drop(session);
        *trace.lock().unwrap() = Trace::default();
        let audit_started = std::time::Instant::now();
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
        let audit_elapsed = audit_started.elapsed();
        let reading = trace.lock().unwrap().clone();
        assert_eq!(reading.catalog_commands, 1);
        assert_eq!(reading.probes.len(), 2);
        let explain_started = std::time::Instant::now();
        let (keys, docs) = explain_probes(&db, &reading).await;
        let explain_elapsed = explain_started.elapsed();
        assert_eq!(&consumed[..3], &[1, 0, 200]);
        assert_eq!(reading.source_rows, 200);
        assert_eq!(reading.source_bytes as u64, consumed[3]);
        assert!(reading.max_source_batch <= 512);
        assert!(reading.max_reply_bytes < 16 * 1024 * 1024 + 1024);
        assert!(reading.read_commands <= 32);
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
            "audit_unit_bounds segments={closed} source_rows={} source_bytes={} read_commands={} catalog_commands={} max_source_batch={} max_reply_bytes={} probe_keys={keys} probe_docs={docs} elapsed_us={} explain_us={}",
            consumed[2], consumed[3], reading.read_commands, reading.catalog_commands, reading.max_source_batch,
            reading.max_reply_bytes, audit_elapsed.as_micros(), explain_elapsed.as_micros(),
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

async fn explain_probes(db: &mongodb::Database, reading: &Trace) -> (i32, i32) {
    let (mut keys, mut docs) = (0, 0);
    for command in &reading.probes {
        let explain = db
            .run_command(doc! {"explain":command,"verbosity":"executionStats"})
            .await
            .unwrap();
        let stats = explain.get_document("executionStats").unwrap();
        let examined = stats.get_i32("totalKeysExamined").unwrap();
        assert_eq!(stats.get_i32("totalDocsExamined").unwrap(), 0, "{stats:?}");
        if command.get_str("hint").unwrap()
            == crate::store::migrations::agent_log_source_keys::INDEX
        {
            assert_eq!(
                examined, 0,
                "foreign malformed rows must not be scanned: {stats:?}"
            );
        } else {
            assert!(examined <= 2, "{stats:?}");
        }
        keys += examined;
        docs += stats.get_i32("totalDocsExamined").unwrap();
    }
    (keys, docs)
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn malformed_probe_has_exact_string_bounds_and_constant_witness_bytes() {
    let trace = Arc::new(Mutex::new(Trace::default()));
    let observed = trace.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        observed.lock().unwrap().observe(event)
    }));
    let client = Client::with_options(options).unwrap();
    let name = format!("cma_key_bounds_{}", uuid::Uuid::new_v4().simple());
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    segmented(&store, 1).await;
    let mut head = Head::read(
        collection(&store, HEADS)
            .find_one(doc! {})
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    store
        .agent_log
        .insert_many((0..1000).map(|i| {
            doc! {
                "_id":format!("foreign:{i}"),"agent_id":format!("foreign:{i}"),"seq":null,
            }
        }))
        .bypass_document_validation(true)
        .await
        .unwrap();
    for id in ["", "a", "a\0", "a\0b", "é", "😀", "prefix"] {
        head.agent_id = id.into();
        store
            .agent_log
            .insert_many([
                doc! {"_id":"neighbor-null","agent_id":format!("{id}\0"),"seq":null},
                doc! {"_id":"neighbor-suffix","agent_id":format!("{id}x"),"seq":null},
            ])
            .bypass_document_validation(true)
            .await
            .unwrap();
        *trace.lock().unwrap() = Trace::default();
        let mut session = client.start_session().snapshot(true).await.unwrap();
        segments::source_keys::prove(&db, &head, &mut session, true)
            .await
            .unwrap();
        let reading = trace.lock().unwrap().clone();
        assert_eq!(reading.probes.len(), 2);
        assert_eq!(reading.source_rows, 0);
        explain_probes(&db, &reading).await;
        drop(session);
        // Neither a huge malformed source ID nor payload may enter the reply.
        let bad_id = "x".repeat(1024 * 1024);
        store
            .agent_log
            .insert_one(
                doc! {"_id":&bad_id,"agent_id":id,"seq":null,"payload":"x".repeat(1024*1024)},
            )
            .bypass_document_validation(true)
            .await
            .unwrap();
        *trace.lock().unwrap() = Trace::default();
        let mut session = client.start_session().snapshot(true).await.unwrap();
        let result = segments::source_keys::prove(&db, &head, &mut session, true).await;
        assert!(matches!(
            result,
            Err(
                crate::store::agent_log_integrity::TransactionLogError::Domain(
                    SandboxStoreError::Corrupt(_)
                )
            )
        ));
        let reading = trace.lock().unwrap().clone();
        assert_eq!(reading.probes.len(), 1);
        assert_eq!(reading.read_commands, 1);
        assert_eq!(reading.catalog_commands, 1);
        assert_eq!(reading.source_rows, 1);
        assert!(reading.source_bytes <= 32);
        assert!(reading.max_reply_bytes < 1024);
        let explain = db
            .run_command(doc! {"explain":&reading.probes[0],"verbosity":"executionStats"})
            .await
            .unwrap();
        let stats = explain.get_document("executionStats").unwrap();
        assert_eq!(stats.get_i32("totalKeysExamined").unwrap(), 1);
        assert_eq!(stats.get_i32("totalDocsExamined").unwrap(), 1);
        eprintln!("malformed_witness id={id:?} keys=1 docs=1 source_bytes={} read_commands=1 catalog_commands=1 max_reply_bytes={}", reading.source_bytes, reading.max_reply_bytes);
        drop(session);
        store
            .agent_log
            .delete_many(doc! {"_id":{"$in":[bad_id,"neighbor-null","neighbor-suffix"]}})
            .await
            .unwrap();
    }
    db.drop().await.unwrap();
}
