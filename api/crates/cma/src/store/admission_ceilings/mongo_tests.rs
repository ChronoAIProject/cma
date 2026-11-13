use super::*;
use layer2_core::admission::{AdmissionCeilings, AdmissionCeilingsStore};

async fn fixture() -> (Client, String, MongoAdmissionCeilingsStore) {
    let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let name = format!("cma_admission_ceilings_{}", uuid::Uuid::new_v4().simple());
    let store = MongoAdmissionCeilingsStore::open(&client, &name, 0)
        .await
        .unwrap();
    (client, name, store)
}
fn policy(limit: u32) -> AdmissionCeilings {
    AdmissionCeilings {
        registrations: Some(limit),
        ..Default::default()
    }
}

#[tokio::test]
#[ignore = "requires a disposable Mongo replica set"]
async fn pending_home_observation_survives_unset_retained_home_ceiling() {
    use mongodb::event::{command::CommandEvent, EventHandler};
    use mongodb::options::ClientOptions;
    use std::sync::{Arc, Mutex};
    let observed = Arc::new(Mutex::new(Vec::new()));
    let trace = observed.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.command.get_str("find").ok() == Some("sandbox_admissions") {
                trace.lock().unwrap().push(event.command);
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let name = format!("cma_pending_observation_{}", uuid::Uuid::new_v4().simple());
    crate::store::MongoRegistrationCapacityStore::open(&client, &name, true)
        .await
        .unwrap();
    let store = MongoAdmissionCeilingsStore::open(&client, &name, 0)
        .await
        .unwrap();
    let _resources = crate::store::MongoSandboxStore::open(&client, &name)
        .await
        .unwrap();
    client
        .database(&name)
        .collection::<Document>("sandbox_admissions")
        .insert_one(doc! {"_id":{"scope":"global_sandbox_capacity"},
        "document_schema_version":DOCUMENT_SCHEMA_VERSION,"fence":1_i64,
        "pending_launch_homes":9_i64})
        .await
        .unwrap();
    for (revision, limit) in [(0, Some(4)), (1, None)] {
        store
            .replace(
                revision,
                &SubjectId::new("operator"),
                "observe retained pending homes",
                AdmissionCeilings {
                    retained_homes: limit,
                    ..Default::default()
                },
                100 + revision,
            )
            .await
            .unwrap();
        observed.lock().unwrap().clear();
        let read = store.read_with_history().await.unwrap();
        assert_eq!(read.effective_limits.retained_homes, limit);
        assert_eq!(read.counts.pending_launch_homes, 9);
        if limit.is_none() {
            assert!(read.counts.retained_homes.is_none());
        }
        let commands = observed.lock().unwrap();
        assert_eq!(
            commands.len(),
            2,
            "activation and pending homes each require exactly one point read"
        );
        assert_eq!(
            commands[1].get_document("filter").unwrap(),
            &doc! {"_id":{"scope":"global_sandbox_capacity"}}
        );
        assert!(commands[0].contains_key("maxTimeMS"));
    }
    client.database(&name).drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires a disposable Mongo replica set"]
async fn ceilings_cas_history_audit_are_atomic_and_independent_of_control_and_settings() {
    use layer2_core::{admission::AdmissionControlStore, PlatformSettingsStore};
    let (client, name, first) = fixture().await;
    let second = MongoAdmissionCeilingsStore::open(&client, &name, 0)
        .await
        .unwrap();
    let settings = crate::store::MongoPlatformSettingsStore::open(&client, &name)
        .await
        .unwrap();
    let initial = settings.read().await.unwrap();
    let control = crate::store::MongoAdmissionControlStore::open(&client, &name)
        .await
        .unwrap();
    let actor = SubjectId::new("operator");
    let (a, b) = tokio::join!(
        first.replace(0, &actor, "first bound", policy(5), 100),
        second.replace(0, &actor, "second bound", policy(8), 101)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(matches!(
        a.as_ref().err().or(b.as_ref().err()),
        Some(StoreError::RevisionConflict { .. })
    ));
    assert_eq!(settings.read().await.unwrap(), initial);
    assert_eq!(control.read().await.unwrap().revision, 0);
    for revision in 1..25 {
        first
            .replace(
                revision,
                &actor,
                "adjust bound",
                policy(revision as u32 + 10),
                100 + revision,
            )
            .await
            .unwrap();
    }
    let read = first.read_with_history().await.unwrap();
    assert_eq!(read.history.len(), 20);
    assert_eq!(read.history[0].revision, 25);
    assert_eq!(read.history[19].revision, 6);
    let db = client.database(&name);
    assert_eq!(
        db.collection::<Document>("activity")
            .count_documents(doc! {"record.target.id":"admission_ceilings"})
            .await
            .unwrap(),
        25
    );
    assert_eq!(db.collection::<Document>(SETTINGS).count_documents(doc! {"_id":{"$gte":"admission_ceilings_change:","$lt":"admission_ceilings_change;"}}).await.unwrap(), 25);
    let saved = first.read().await.unwrap();
    first
        .replace(25, &actor, "same policy", saved.ceilings.clone(), 200)
        .await
        .unwrap();
    assert_eq!(first.read().await.unwrap(), saved);
    control
        .change(0, &actor, "contain incident", true, 200)
        .await
        .unwrap();
    assert_eq!(first.read().await.unwrap(), saved);
    db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires a disposable Mongo replica set"]
async fn unreadable_policy_and_missing_history_are_never_repaired() {
    let (client, name, store) = fixture().await;
    let settings = client.database(&name).collection::<Document>(SETTINGS);
    store
        .replace(
            0,
            &SubjectId::new("operator"),
            "bound registrations",
            policy(2),
            100,
        )
        .await
        .unwrap();
    settings
        .delete_one(doc! {"_id":history::change_key(1)})
        .await
        .unwrap();
    assert!(store.read_with_history().await.is_err());
    settings
        .update_one(
            doc! {"_id":"platform"},
            doc! {"$set":{"admission_ceilings":"unreadable"}},
        )
        .await
        .unwrap();
    assert!(store.read().await.is_err());
    assert!(store
        .replace(
            1,
            &SubjectId::new("operator"),
            "do not repair",
            policy(3),
            101
        )
        .await
        .is_err());
    assert_eq!(
        settings
            .find_one(doc! {"_id":"platform"})
            .await
            .unwrap()
            .unwrap()
            .get_str("admission_ceilings")
            .unwrap(),
        "unreadable"
    );
    client.database(&name).drop().await.unwrap();
}
#[test]
fn ceilings_are_strict_and_separate_from_predecessor_control() {
    let document = doc! {"document_schema_version":1_i64,"admission_ceilings":"invalid"};
    assert!(from_document(&document).is_err());
    assert_eq!(
        crate::store::admission_control::from_document(&document).unwrap(),
        layer2_core::admission::AdmissionControl::default()
    );
    assert_eq!(
        from_document(&doc! {"document_schema_version":1_i64}).unwrap(),
        AdmissionCeilingsState::default()
    );
}

#[tokio::test]
#[ignore = "requires a disposable Mongo replica set"]
async fn audit_or_history_write_failure_rolls_back_the_entire_change() {
    for collection in ["activity", "platform_settings"] {
        let (client, name, store) = fixture().await;
        let validator = if collection == "activity" {
            doc! {"audit_failure_fixture":true}
        } else {
            doc! {"_id":"platform"}
        };
        client
            .database(&name)
            .run_command(doc! {"collMod":collection,"validator":validator,
            "validationLevel":"strict","validationAction":"error"})
            .await
            .unwrap();
        assert!(store
            .replace(
                0,
                &SubjectId::new("operator"),
                "atomic evidence",
                policy(1),
                100
            )
            .await
            .is_err());
        assert_eq!(
            store.read().await.unwrap(),
            AdmissionCeilingsState::default()
        );
        assert!(store.read_with_history().await.unwrap().history.is_empty());
        assert_eq!(
            client
                .database(&name)
                .collection::<Document>("activity")
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires a disposable Mongo replica set"]
async fn advisory_counts_use_effective_limits_and_bounded_lower_bounds() {
    let (client, name, _) = fixture().await;
    // Existing initialization supplies exactly the indexes enforcement uses.
    let _sandboxes = crate::store::sandbox_mongo::MongoSandboxStore::open(&client, &name)
        .await
        .unwrap();
    let db = client.database(&name);
    for n in 0..5 {
        db.collection::<Document>("sandboxes")
            .insert_one(doc! {"_id":n,"owner":format!("owner-{n}"),"name":"capacity","retired_at":Bson::Null})
            .await
            .unwrap();
        db.collection::<Document>("agents")
            .insert_one(doc! {"_id":n,"owner":format!("owner-{n}"),"creation_key":format!("create-{n}"),"worktree_id":format!("tree-{n}"),"status":"working"})
            .await
            .unwrap();
    }
    let store = MongoAdmissionCeilingsStore::open(&client, &name, 2)
        .await
        .unwrap();
    store
        .replace(
            0,
            &SubjectId::new("operator"),
            "bounded observation",
            AdmissionCeilings {
                retained_homes: Some(4),
                active_sessions: Some(3),
                ..Default::default()
            },
            100,
        )
        .await
        .unwrap();
    let read = store.read_with_history().await.unwrap();
    assert_eq!(read.effective_limits.retained_homes, Some(2));
    assert_eq!(read.counts.retained_homes.unwrap().observed, 2);
    assert_eq!(read.counts.active_sessions.unwrap().observed, 3);
    assert!(read.counts.registrations.is_none());
    db.drop().await.unwrap();
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
    let app = format!("cma-ceilings-history-{suffix}");
    let name = format!("cma_ceilings_history_{suffix}");
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
    let store = MongoAdmissionCeilingsStore::open(&client, &name, 0)
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
        .replace(
            0,
            &SubjectId::new("operator-a"),
            "bound capacity",
            policy(2),
            100,
        )
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
            if read.state.revision == 1 {
                break read;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        snapshot.history,
        vec![snapshot.state.last_change.clone().unwrap()]
    );
    let repeated = store
        .replace(
            1,
            &SubjectId::new("operator-b"),
            "same capacity",
            policy(2),
            101,
        )
        .await
        .unwrap();
    assert_eq!(repeated, snapshot.state);
    let db = client.database(&name);
    assert_eq!(db.collection::<Document>(SETTINGS).count_documents(doc! {"_id":{"$gte":"admission_ceilings_change:","$lt":"admission_ceilings_change;"}}).await.unwrap(),1);
    assert_eq!(
        db.collection::<Document>("activity")
            .count_documents(doc! {"record.target.id":"admission_ceilings"})
            .await
            .unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}
