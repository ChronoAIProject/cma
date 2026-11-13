use super::*;
use crate::store::admission_control::history::{change_key, read_change};
use layer2_core::admission::ADMISSION_HISTORY_LIMIT;

async fn history_count(client: &Client, name: &str) -> u64 {
    client
        .database(name)
        .collection::<Document>("platform_settings")
        .count_documents(
            doc! {"_id": {"$gte": "admission_control_change:", "$lt": "admission_control_change;"}},
        )
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn history_is_immutable_complete_and_read_as_twenty_newest_changes() {
    let (client, name, store) = fixture().await;
    let settings = client
        .database(&name)
        .collection::<Document>("platform_settings");
    let mut first = None;
    for expected in 0..26 {
        let stopped = expected % 2 == 0;
        let state = store
            .change(
                expected,
                &SubjectId::new(format!("operator-{expected}")),
                &format!("reason-{expected}"),
                stopped,
                100 + expected,
            )
            .await
            .unwrap();
        let raw = settings
            .find_one(doc! {"_id": change_key(state.revision)})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            read_change(raw.clone()).unwrap(),
            state.last_change.unwrap()
        );
        if expected == 0 {
            first = Some(raw);
        }
        store
            .change(
                if stopped { 0 } else { state.revision },
                &SubjectId::new("other"),
                "no-op",
                stopped,
                200 + expected,
            )
            .await
            .unwrap();
    }
    let reopened = MongoAdmissionControlStore::open(&client, &name)
        .await
        .unwrap();
    let read = reopened.read_with_history().await.unwrap();
    assert_eq!(read.control.revision, 26);
    assert_eq!(read.history.len(), ADMISSION_HISTORY_LIMIT);
    assert_eq!(
        read.history
            .iter()
            .map(|row| row.revision)
            .collect::<Vec<_>>(),
        (7..=26).rev().collect::<Vec<_>>()
    );
    assert_eq!(read.history[0].reason, "reason-25");
    assert_eq!(read.history[19].actor, SubjectId::new("operator-6"));
    assert_eq!(
        settings
            .find_one(doc! {"_id": change_key(1)})
            .await
            .unwrap(),
        first
    );
    assert_eq!(history_count(&client, &name).await, 26);
    assert_eq!(audit_count(&client, &name).await, 26);
    let ordinary = crate::store::settings::MongoPlatformSettingsStore::open(&client, &name)
        .await
        .unwrap();
    assert_eq!(ordinary.read().await.unwrap().revision, 0);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn history_insert_failure_aborts_the_singleton_and_activity() {
    let (client, name, store) = fixture().await;
    client
        .database(&name)
        .run_command(doc! {
            "collMod": "platform_settings", "validator": {"_id": "platform"},
            "validationLevel": "strict", "validationAction": "error",
        })
        .await
        .unwrap();
    assert!(store
        .change(0, &SubjectId::new("operator-a"), "incident", true, 100)
        .await
        .is_err());
    assert_eq!(store.read().await.unwrap(), AdmissionControl::default());
    assert_eq!(history_count(&client, &name).await, 0);
    assert_eq!(audit_count(&client, &name).await, 0);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn history_reads_refuse_missing_malformed_and_future_change_rows() {
    let (client, name, store) = fixture().await;
    store
        .change(0, &SubjectId::new("operator-a"), "incident", true, 100)
        .await
        .unwrap();
    let settings = client
        .database(&name)
        .collection::<Document>("platform_settings");
    let original = settings
        .find_one(doc! {"_id": change_key(1)})
        .await
        .unwrap()
        .unwrap();
    for kind in ["outer", "record", "schema", "revision", "reason"] {
        let mut invalid = original.clone();
        match kind {
            "outer" => {
                invalid.insert("unknown", true);
            }
            "record" => {
                invalid
                    .get_document_mut("record")
                    .unwrap()
                    .insert("unknown", true);
            }
            "schema" => {
                invalid.insert("document_schema_version", 2_i64);
            }
            "revision" => {
                invalid
                    .get_document_mut("record")
                    .unwrap()
                    .insert("revision", 2_i64);
            }
            "reason" => {
                invalid
                    .get_document_mut("record")
                    .unwrap()
                    .insert("reason", "rewritten");
            }
            _ => unreachable!(),
        }
        settings
            .replace_one(doc! {"_id": change_key(1)}, invalid)
            .await
            .unwrap();
        assert!(
            matches!(
                store.read_with_history().await,
                Err(StoreError::Malformed(_))
            ),
            "accepted {kind}"
        );
    }
    settings
        .delete_one(doc! {"_id": change_key(1)})
        .await
        .unwrap();
    assert!(matches!(
        store.read_with_history().await,
        Err(StoreError::Malformed(_))
    ));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn control_and_history_reads_share_one_snapshot_during_concurrent_changes() {
    let (client, name, writer) = fixture().await;
    let reader = MongoAdmissionControlStore::open(&client, &name)
        .await
        .unwrap();
    let ((), ()) = tokio::join!(
        async {
            for revision in 0..26 {
                writer
                    .change(
                        revision,
                        &SubjectId::new("operator-a"),
                        "incident transition",
                        revision % 2 == 0,
                        100 + revision,
                    )
                    .await
                    .unwrap();
                tokio::task::yield_now().await;
            }
        },
        async {
            for _ in 0..32 {
                let snapshot = reader.read_with_history().await.unwrap();
                snapshot.validate().unwrap();
                assert_eq!(
                    snapshot.history.first(),
                    snapshot.control.last_change.as_ref()
                );
                tokio::task::yield_now().await;
            }
        }
    );
    assert_eq!(
        reader.read_with_history().await.unwrap().control.revision,
        26
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn unknown_commit_never_duplicates_the_change_history_or_activity() {
    use mongodb::event::{command::CommandEvent, EventHandler};
    use mongodb::options::ClientOptions;
    use std::sync::{Arc, Mutex};
    #[derive(Default)]
    struct Trace {
        starts: usize,
        commits: usize,
        sessions: Vec<Document>,
    }
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let app = format!("cma-admission-history-{suffix}");
    let name = format!("cma_admission_history_{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let trace = Arc::new(Mutex::new(Trace::default()));
    let observed = trace.clone();
    let mut options = ClientOptions::parse(uri).await.unwrap();
    options.app_name = Some(app.clone());
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            let mut trace = observed.lock().unwrap();
            if matches!(event.command.get_bool("startTransaction"), Ok(true)) {
                trace.starts += 1;
                let session = event.command.get_document("lsid").unwrap().clone();
                if !trace.sessions.contains(&session) {
                    trace.sessions.push(session);
                }
            }
            if event.command_name == "commitTransaction" {
                trace.commits += 1;
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = MongoAdmissionControlStore::open(&client, &name)
        .await
        .unwrap();
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": {"times": 5_i32},
            "data": {"failCommands": ["commitTransaction"], "appName": &app,
                "writeConcernError": {"code": 50_i32, "codeName": "MaxTimeMSExpired",
                    "errmsg": "injected admission history ambiguity"}},
        })
        .await
        .unwrap();
    let result = store
        .change(0, &SubjectId::new("operator-a"), "incident", true, 100)
        .await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();
    let (starts, commits, sessions) = {
        let trace = trace.lock().unwrap();
        (trace.starts, trace.commits, trace.sessions.clone())
    };
    if !sessions.is_empty() {
        admin
            .database("admin")
            .run_command(doc! {"killSessions": sessions})
            .await
            .unwrap();
    }
    assert!(matches!(result, Err(StoreError::Unavailable(_))));
    assert_eq!(starts, 1, "the unknown transaction body must never rerun");
    assert_eq!(commits, 5, "only the same commit is retried");
    let snapshot = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let read = store.read_with_history().await.unwrap();
            if read.control.revision == 1 {
                break read;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        snapshot.history,
        vec![snapshot.control.last_change.clone().unwrap()]
    );
    let repeated = store
        .change(0, &SubjectId::new("operator-b"), "duplicate", true, 101)
        .await
        .unwrap();
    assert_eq!(repeated, snapshot.control);
    assert_eq!(history_count(&client, &name).await, 1);
    assert_eq!(audit_count(&client, &name).await, 1);
    client.database(&name).drop().await.unwrap();
}
