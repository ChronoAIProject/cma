use super::*;
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
};
use std::sync::{Arc, Mutex};

use crate::store::migrations::activity_audit_index;

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn activity_summary_reads_are_bounded_projected_and_never_count_or_write() {
    let commands = Arc::new(Mutex::new(Vec::<(String, Document)>::new()));
    let seen = commands.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            seen.lock()
                .unwrap()
                .push((event.command_name, event.command));
        }
    }));
    let client = Client::with_options(options).unwrap();
    let name = format!("cma_activity_commands_{}", uuid::Uuid::new_v4().simple());
    let store = MongoActivityStore::open(&client, &name, Duration::from_secs(90 * 86400))
        .await
        .unwrap();
    store.append(&row(1)).await.unwrap();
    commands.lock().unwrap().clear();
    assert_eq!(
        store
            .list_summaries(&ActivityQuery {
                limit: 1,
                ..Default::default()
            })
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    let observed = commands.lock().unwrap().clone();
    assert_eq!(observed.len(), 1, "{observed:?}");
    assert_eq!(observed[0].0, "find");
    let query = &observed[0].1;
    assert_eq!(query.get_i64("limit").unwrap(), 2);
    // The driver requests one extra batch slot to avoid an empty getMore at the limit.
    assert_eq!(query.get_i32("batchSize").unwrap(), 3);
    assert_eq!(query.get_i32("maxTimeMS").unwrap(), 5000);
    assert!(!query.contains_key("skip") && !query.contains_key("hint"));
    let projection = query.get_document("projection").unwrap();
    let fields = [
        "_id",
        "document_schema_version",
        "recorded_at",
        "record.id",
        "record.at",
        "record.actor",
        "record.action",
        "record.target",
        "record.outcome",
        "record.detail.activity_channel",
        "record.detail.request_id",
        "record.detail.request_method",
        "record.detail.request_route",
        "record.detail.credential_kind",
        "record.detail.actor_scope",
        "record.detail.http_status",
    ];
    assert_eq!(projection.len(), fields.len());
    for field in fields {
        assert_eq!(projection.get_i32(field).ok(), Some(1), "{field}");
    }
    commands.lock().unwrap().clear();
    assert!(store
        .list_summaries(&ActivityQuery {
            cursor: Some("invalid".into()),
            ..Default::default()
        })
        .await
        .is_err());
    assert!(commands.lock().unwrap().is_empty());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn audit_fallback_continuation_seeks_with_the_first_pages_global_index() {
    let commands = Arc::new(Mutex::new(Vec::<Document>::new()));
    let seen = commands.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.command_name == "find" {
                seen.lock().unwrap().push(event.command);
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let name = format!("cma_activity_fallback_{}", uuid::Uuid::new_v4().simple());
    let db = client.database(&name);
    let at = DateTime::now();
    let older = DateTime::from_millis(at.timestamp_millis() - 1);
    let index_keys = doc! { "recorded_at": -1, "_id": -1 };
    let collection = db.collection::<Document>(ACTIVITY);
    collection
        .create_index(
            IndexModel::builder()
                .keys(index_keys.clone())
                .options(
                    IndexOptions::builder()
                        .name("activity_recorded_at_id_desc".to_owned())
                        .build(),
                )
                .build(),
        )
        .await
        .unwrap();
    for start in (0..4096).step_by(1024) {
        collection
            .insert_many((start..start + 1024).map(|id| {
                let mut doc = document(id, if id < 10 { older } else { at });
                let record = doc.get_document_mut("record").unwrap();
                let action = if (7..=12).contains(&id) && id != 10 {
                    "update"
                } else {
                    "read"
                };
                record.insert("action", action);
                if id == 10 {
                    record.insert("outcome", doc! { "status":"failed", "code":"fixture" });
                }
                doc
            }))
            .await
            .unwrap();
    }
    let store = MongoActivityStore::open(&client, &name, Duration::from_secs(90 * 86400))
        .await
        .unwrap();
    commands.lock().unwrap().clear();
    let mut query = ActivityQuery {
        exclude_successful_reads: true,
        limit: 2,
        ..Default::default()
    };
    let first = store.list_summaries(&query).await.unwrap();
    query.cursor = first.next_cursor;
    assert!(query.cursor.is_some());
    let second = store.list_summaries(&query).await.unwrap();
    assert!(second.next_cursor.is_some());
    assert_eq!(
        first
            .items
            .iter()
            .chain(&second.items)
            .map(|row| row.id.0.as_str())
            .collect::<Vec<_>>(),
        [
            "act_000000012",
            "act_000000011",
            "act_000000010",
            "act_000000009"
        ]
    );
    assert!(matches!(
        second.items[0].outcome,
        ActivityOutcome::Failed(_)
    ));

    let pages = commands.lock().unwrap().clone();
    assert_eq!(pages.len(), 2);
    for command in &pages {
        assert_eq!(command.get_document("hint").unwrap(), &index_keys);
        assert_eq!(command.get_i64("limit").unwrap(), 3);
        assert_eq!(command.get_i32("maxTimeMS").unwrap(), 5000);
    }
    assert!(!pages[0].contains_key("min"));
    let mut continuation = pages[1].clone();
    assert_eq!(
        continuation.get_document("min").unwrap(),
        &doc! { "recorded_at": at, "_id": "act_000000011" }
    );
    for key in ["$db", "lsid", "$clusterTime", "readConcern"] {
        continuation.remove(key);
    }
    let explain = db
        .run_command(doc! { "explain":continuation, "verbosity":"executionStats" })
        .await
        .unwrap();
    let stats = explain.get_document("executionStats").unwrap();
    for metric in ["totalDocsExamined", "totalKeysExamined"] {
        let count = stats
            .get_i32(metric)
            .map(i64::from)
            .or_else(|_| stats.get_i64(metric))
            .unwrap();
        assert!(count <= 4, "{metric}: {count}");
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn sparse_audit_pages_seek_past_successful_reads_and_tied_cursors() {
    let commands = Arc::new(Mutex::new(Vec::<Document>::new()));
    let seen = commands.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.command_name == "find" {
                seen.lock().unwrap().push(event.command);
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let name = format!("cma_activity_audit_{}", uuid::Uuid::new_v4().simple());
    let db = client.database(&name);
    let retention = Duration::from_secs(90 * 86400);
    let legacy = MongoActivityStore::open(&client, &name, retention)
        .await
        .unwrap();
    let at = DateTime::now();
    let collection = db.collection::<Document>(ACTIVITY);

    let non_reads = [
        "authenticate",
        "create",
        "update",
        "delete",
        "configure",
        "gateway_record_write",
        "gateway_record_delete",
    ];
    let total = non_reads.len() * 4000;
    for start in (0..total).step_by(1000) {
        collection
            .insert_many((start..start + 1000).map(|id| {
                let mut doc = document(id, at);
                let record = doc.get_document_mut("record").unwrap();
                let (action, status) = match id % 4000 {
                    0 => (non_reads[id / 4000], "succeeded"),
                    1 => ("read", "failed"),
                    2 => ("gateway_record_read", "refused"),
                    3 => ("list", "failed"),
                    _ => (["read", "list", "gateway_record_read"][id % 3], "succeeded"),
                };
                record.insert("action", action);
                let mut outcome = doc! {"status":status};
                if status != "succeeded" {
                    outcome.insert("code", "fixture");
                }
                record.insert("outcome", outcome);
                doc
            }))
            .await
            .unwrap();
    }

    let mut query = ActivityQuery {
        exclude_successful_reads: true,
        limit: 5,
        ..Default::default()
    };
    let unindexed = legacy.list_summaries(&query).await.unwrap();
    assert!(!commands
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .contains_key("hint"));
    collection
        .create_index(
            IndexModel::builder()
                .keys(doc! {"recorded_at":-1,"_id":-1})
                .options(
                    IndexOptions::builder()
                        .name("activity_recorded_at_id_desc".to_owned())
                        .build(),
                )
                .build(),
        )
        .await
        .unwrap();
    activity_audit_index::prepare_for_test(&db).await;

    let store = MongoActivityStore::open(&client, &name, retention)
        .await
        .unwrap();
    let expected: Vec<_> = (0..total)
        .rev()
        .filter(|id| id % 4000 < 4)
        .map(|id| ActivityId(format!("act_{id:09}")))
        .collect();
    let mut seen = Vec::new();
    loop {
        let page = store.list_summaries(&query).await.unwrap();
        if query.cursor.is_none() {
            assert_eq!(page.items, unindexed.items);
            assert_eq!(page.next_cursor, unindexed.next_cursor);
        }
        let mut command = commands.lock().unwrap().pop().unwrap();
        assert_eq!(
            command.get_str("hint").unwrap(),
            activity_audit_index::INDEX
        );
        assert_eq!(command.contains_key("min"), query.cursor.is_some());
        for key in ["$db", "lsid", "$clusterTime", "readConcern"] {
            command.remove(key);
        }
        let explain = db
            .run_command(doc! {
                "explain": command, "verbosity":"executionStats",
            })
            .await
            .unwrap();
        let stats = explain.get_document("executionStats").unwrap();
        for metric in ["totalDocsExamined", "totalKeysExamined"] {
            let count = stats
                .get_i32(metric)
                .map(i64::from)
                .or_else(|_| stats.get_i64(metric))
                .unwrap();
            assert!(count <= i64::from(query.limit + 2), "{metric}: {count}");
        }
        seen.extend(page.items.into_iter().map(|row| row.id));
        query.cursor = page.next_cursor;
        if query.cursor.is_none() {
            break;
        }
    }
    assert_eq!(seen, expected);

    let explicit = store
        .list_summaries(&ActivityQuery {
            action: Some(ActivityAction::Read),
            limit: 5,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(explicit
        .items
        .iter()
        .any(|row| row.outcome == ActivityOutcome::Succeeded));
    assert!(!commands
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .contains_key("hint"));
    db.drop().await.unwrap();
}
