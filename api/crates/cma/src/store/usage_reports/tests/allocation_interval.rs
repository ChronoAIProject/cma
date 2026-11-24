//! Exercise the native snapshot reader, including full cursor consumption.
use super::*;
use crate::store::sandbox_usage::{event_document, read_event};
use crate::store::sandbox_usage_rollups::AllocationDelta;
use layer2_core::sandbox_usage::UsageEvent;
use mongodb::event::{command::CommandEvent, EventHandler};
use std::sync::Mutex;

async fn observed(database_variable: &str) -> (Arc<ReportingStore>, Arc<Mutex<Vec<Document>>>) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set required");
    let database = match std::env::var(database_variable) {
        Ok(database) => database,
        Err(std::env::VarError::NotPresent) => {
            format!("metrics_2140_{}", uuid::Uuid::new_v4().simple())
        }
        Err(error) => panic!("invalid named disposable database: {error}"),
    };
    assert!(database.starts_with("metrics_2140_"));
    let original = ReportingStore::connect(&uri, &database, &[19; 32])
        .await
        .unwrap();
    let commands = Arc::new(Mutex::new(Vec::new()));
    let recorded = commands.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    configure_pool(&mut options, 4);
    super::super::budget::observe(&mut options, original.circuit.clone());
    options.command_event_handler = Some(EventHandler::callback(move |event| match event {
        CommandEvent::Started(event)
            if matches!(
                event.command.get_str("find").ok(),
                Some("sandbox_usage_events" | "sandbox_usage_rollups")
            ) || matches!(
                event.command.get_str("update").ok(),
                Some("sandbox_usage_rollups" | "sandbox_usage_report_control")
            ) =>
        {
            recorded.lock().unwrap().push(event.command);
        }
        _ => {}
    }));
    let client = Client::with_options(options).unwrap();
    let mut store = (*original).clone();
    store.persistence = store.persistence.with_client(client);
    (Arc::new(store), commands)
}

async fn baseline(
    store: &ReportingStore,
    ids: &[&str],
    range: Range,
) -> std::collections::BTreeMap<String, Vec<UsageEvent>> {
    let mut session = store.snapshot().await.unwrap();
    let mut cursor = store
        .collection("sandbox_usage_events")
        .find(
            doc! {"sandbox_id":{"$in":ids},"ended_at":{"$gt":range.from as i64},
            "started_at":{"$lt":range.to as i64}},
        )
        .sort(doc! {"sandbox_id":1,"revision":1})
        .batch_size(2_000)
        .max_time(DB_DEADLINE)
        .session(&mut session)
        .await
        .unwrap();
    let mut expected = std::collections::BTreeMap::<String, Vec<UsageEvent>>::new();
    while let Some(document) = cursor.next(&mut session).await.transpose().unwrap() {
        let event = read_event(&document).unwrap();
        expected
            .entry(event.sandbox_id.to_string())
            .or_default()
            .push(event);
    }
    expected
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn allocation_interval_native_cursor_matches_projection_with_and_without_index() {
    let (store, commands) = observed("CHRONO_TEST_ALLOCATION_DB_01").await;
    let day = (now() / 86_400 - 2) * 86_400;
    let range = Range {
        from: day + 60,
        to: day + 60 * 1_201,
    };
    let mut states = Vec::new();
    for id in 0..2 {
        let mut state = state(id, day);
        state.revision = 1_202;
        state.last_observed_at = day + 60 * 1_202;
        // Reverse insertion forces a real ordering correction without an index.
        let docs = (0..1_202)
            .rev()
            .map(|n| event_document(&history_batch::event(&state, n, day + n * 60)).unwrap())
            .collect::<Vec<_>>();
        store
            .collection("sandbox_usage_events")
            .insert_many(docs)
            .await
            .unwrap();
        states.push(state);
    }
    let ids = states
        .iter()
        .map(|s| s.sandbox_id.as_str())
        .collect::<Vec<_>>();
    for with_interval_index in [false, true] {
        if with_interval_index {
            store
                .collection("sandbox_usage_events")
                .create_indexes([
                    mongodb::IndexModel::builder()
                        .keys(doc! {"sandbox_id":1,"revision":1})
                        .build(),
                    mongodb::IndexModel::builder()
                        .keys(doc! {"sandbox_id":1,"ended_at":1,"started_at":1,"revision":1})
                        .build(),
                ])
                .await
                .unwrap();
        }
        let expected = baseline(&store, &ids, range).await;
        commands.lock().unwrap().clear();
        let actual = store
            .allocation_events(&mut store.snapshot().await.unwrap(), &ids, range)
            .await
            .unwrap();
        assert_eq!(actual, expected);
        assert_eq!(actual.values().map(Vec::len).sum::<usize>(), 2_400);
        for state in &states {
            let projected = allocation::project(state, &actual[state.sandbox_id.as_str()], range);
            assert!(projected.period.complete);
            assert_eq!(projected.period.uptime_seconds, 72_000.);
            assert_eq!(
                serde_json::to_value(projected).unwrap(),
                serde_json::to_value(allocation::project(
                    state,
                    &expected[state.sandbox_id.as_str()],
                    range
                ))
                .unwrap()
            );
        }
        let recorded = commands.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert!(!recorded[0].contains_key("sort"));
        assert!(!recorded[0].contains_key("hint"));
        let integer = |key: &str| {
            recorded[0]
                .get(key)
                .and_then(|value| value.as_i64().or_else(|| value.as_i32().map(i64::from)))
        };
        assert_eq!(integer("batchSize"), Some(2_000));
        assert_eq!(
            recorded[0]
                .get_document("readConcern")
                .unwrap()
                .get_str("level")
                .unwrap(),
            "snapshot"
        );
        assert_eq!(integer("maxTimeMS"), Some(5_000));
    }
    commands.lock().unwrap().clear();
    let projections = store
        .allocation_periods(&mut store.snapshot().await.unwrap(), &states, range)
        .await
        .unwrap();
    assert!(projections.values().all(|p| p.period.complete));
    // The longer command budget is confined to the allocation cache/raw path.
    store
        .bulk_updates(
            CONTROL,
            vec![doc! {"q":{"_id":"deadline-test"},
            "u":{"$set":{"document_schema_version":1_i64}},"upsert":true,"multi":false}],
        )
        .await
        .unwrap();
    {
        let recorded = commands.lock().unwrap();
        assert_eq!(recorded.len(), 4);
        for command in recorded.iter() {
            let ordinary = command.get_str("update").ok() == Some(CONTROL);
            let max_time = command.get("maxTimeMS").unwrap();
            assert_eq!(
                max_time
                    .as_i64()
                    .or_else(|| max_time.as_i32().map(i64::from)),
                Some(if ordinary { 2_000 } else { 5_000 })
            );
        }
    }
    // A corrupt later event must reject the complete day, never publish a prefix.
    let corrupt_state = state(2, day);
    let mut corrupt = event_document(&history_batch::event(&corrupt_state, 0, range.from)).unwrap();
    corrupt.insert("owner", "foreign");
    store
        .collection("sandbox_usage_events")
        .insert_one(corrupt)
        .await
        .unwrap();
    let mut all_ids = ids.clone();
    all_ids.push(corrupt_state.sandbox_id.as_str());
    assert!(matches!(
        store
            .allocation_events(&mut store.snapshot().await.unwrap(), &all_ids, range)
            .await,
        Err(MetricsError::Unavailable)
    ));
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn allocation_interval_native_boundaries_delta_and_snapshot_remain_exact() {
    let (store, commands) = observed("CHRONO_TEST_ALLOCATION_DB_02").await;
    let day = (now() / 86_400 - 2) * 86_400;
    let state = state(0, day);
    let range = Range {
        from: day + 60,
        to: day + 120,
    };
    let mut events = Vec::new();
    for (revision, start, end) in [
        (1, day, range.from),
        (2, day + 30, day + 90),
        (3, day + 90, day + 150),
        (4, range.to, day + 180),
    ] {
        let mut event = history_batch::event(&state, revision - 1, start);
        event.ended_at = end;
        event.quantity = event.allocation.usage(end - start).unwrap();
        events.push(event);
    }
    store
        .collection("sandbox_usage_events")
        .insert_many(
            events
                .iter()
                .rev()
                .map(|e| event_document(e).unwrap())
                .collect::<Vec<_>>(),
        )
        .await
        .unwrap();
    store
        .collection("sandbox_usage_events")
        .create_index(
            mongodb::IndexModel::builder()
                .keys(doc! {"sandbox_id":1,"revision":1})
                .build(),
        )
        .await
        .unwrap();
    let id = state.sandbox_id.as_str();
    let mut snapshot = store.snapshot().await.unwrap();
    let actual = store
        .allocation_events(&mut snapshot, &[id], range)
        .await
        .unwrap();
    assert_eq!(actual[id], events[1..3]);
    let mut later = events[2].clone();
    later.revision = 5;
    store
        .collection("sandbox_usage_events")
        .insert_one(event_document(&later).unwrap())
        .await
        .unwrap();
    assert_eq!(
        store
            .allocation_events(&mut snapshot, &[id], range)
            .await
            .unwrap(),
        actual
    );
    commands.lock().unwrap().clear();
    let delta = store
        .allocation_events_after(
            &mut snapshot,
            &[AllocationDelta {
                sandbox_id: id,
                after_revision: 2,
                through_revision: 3,
            }],
            range,
        )
        .await
        .unwrap();
    assert_eq!(delta[id], vec![events[2].clone()]);
    let command = commands.lock().unwrap().last().unwrap().clone();
    assert_eq!(
        command.get_document("sort").unwrap(),
        &doc! {"sandbox_id":1,"revision":1}
    );
    assert!(!command.contains_key("hint"));
    assert!(store
        .allocation_events(&mut snapshot, &[], range)
        .await
        .unwrap()
        .is_empty());
    assert!(store
        .allocation_events(&mut snapshot, &["absent"], range)
        .await
        .unwrap()
        .is_empty());
    assert!(matches!(
        store
            .allocation_events(&mut snapshot, &[id; 201], range)
            .await,
        Err(MetricsError::Busy)
    ));
    assert!(matches!(
        store
            .allocation_events(
                &mut snapshot,
                &[id],
                Range {
                    from: day,
                    to: day + 86_401
                }
            )
            .await,
        Err(MetricsError::Busy)
    ));
    store.db.drop().await.unwrap();
}
