//! Keep both historical near-capacity cases and the canonical Mongo cache.
use super::*;
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Default)]
struct Trace {
    queries: usize,
    source_rows: usize,
    largest_reply: usize,
    // Session/transaction keys stay inside the test; output is aggregate-only.
    writes: BTreeMap<String, (usize, usize)>,
}

#[tokio::test]
#[ignore = "requires a disposable replica set with the canonical bounded cache"]
async fn near_capacity_audit_keeps_source_bounds_and_limits_each_write_transaction() {
    for large in [false, true] {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
        let trace = Arc::new(Mutex::new(Trace::default()));
        let observed = trace.clone();
        let mut options = ClientOptions::parse(uri).await.unwrap();
        options.command_event_handler = Some(EventHandler::callback(move |event| match event {
            CommandEvent::Succeeded(event)
                if matches!(event.command_name.as_str(), "find" | "getMore") =>
            {
                let bytes = bson::serialize_to_vec(&event.reply).unwrap().len();
                let mut reading = observed.lock().unwrap();
                reading.queries += 1;
                reading.largest_reply = reading.largest_reply.max(bytes);
                if let Ok(cursor) = event.reply.get_document("cursor") {
                    if cursor
                        .get_str("ns")
                        .is_ok_and(|ns| ns.ends_with(".agent_log"))
                    {
                        reading.source_rows += cursor
                            .get_array("firstBatch")
                            .or_else(|_| cursor.get_array("nextBatch"))
                            .map_or(0, Vec::len);
                    }
                }
            }
            CommandEvent::Started(event)
                if event.command_name == "update"
                    && event.command.get_bool("autocommit") == Ok(false) =>
            {
                let key = format!(
                    "{:?}:{}",
                    event.command.get_document("lsid").unwrap(),
                    event.command.get_i64("txnNumber").unwrap()
                );
                let count = event.command.get_array("updates").unwrap().len();
                let bytes = bson::serialize_to_vec(&event.command).unwrap().len();
                let mut reading = observed.lock().unwrap();
                let total = reading.writes.entry(key).or_default();
                total.0 += count;
                total.1 += bytes;
            }
            _ => {}
        }));
        let client = Client::with_options(options).unwrap();
        let name = format!(
            "cma_segments_audit_bounds_{}",
            uuid::Uuid::new_v4().simple()
        );
        let db = client.database(&name);
        schema::migrate(&db).await.unwrap();
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let (agent, source) = super::audit_tests::conversation(
            &store,
            if large { 63 } else { 9996 },
            if large { 1024 * 1024 } else { 1 },
        )
        .await;
        *trace.lock().unwrap() = Trace::default();
        let started = Instant::now();
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            segments::audit::run_unit(&client, &db, agent.id.as_str(), "capacity", false),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result, "complete");
        {
            let reading = trace.lock().unwrap();
            assert_eq!(reading.source_rows, source.len());
            assert!(
                reading.queries < 200,
                "audit must preserve the historical bounded query envelope: {}",
                reading.queries
            );
            assert!(reading.largest_reply < 16 * 1024 * 1024 + 1024);
            assert!(
                reading.writes.len() > 1,
                "source validation and evidence persistence must use bounded transactions"
            );
            assert!(reading
                .writes
                .values()
                .all(|(rows, bytes)| *rows <= 129 && *bytes <= 3 * 1024 * 1024));
            eprintln!("audit_bounds large={large} rows={} queries={} transactions={} max_write_rows={} max_write_bytes={} elapsed_ms={}",
                reading.source_rows,reading.queries,reading.writes.len(),reading.writes.values().map(|v|v.0).max().unwrap_or(0),
                reading.writes.values().map(|v|v.1).max().unwrap_or(0),started.elapsed().as_millis());
        }
        assert_eq!(
            segments::audit::install(&client, &db, agent.id.as_str(), "capacity")
                .await
                .unwrap(),
            "installed"
        );
        db.drop().await.unwrap();
    }
}
