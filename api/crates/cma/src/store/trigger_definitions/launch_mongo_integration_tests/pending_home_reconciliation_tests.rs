//! Repair only provable terminal leaks; preserve live and missing Launch receipts.
use super::home_capacity::{configure, home, pending, receipt, request, resources};
use super::*;
use crate::store::{
    launch_home_capacity::{fence_key, receipt_key},
    pending_launch_homes::{reserved_filter, PendingLaunchHomes, BATCH_SIZE, INDEX},
    sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome},
};
use futures_util::TryStreamExt;
use layer2_core::admission::{AdmissionCeilings, AdmissionCeilingsStore};
use layer2_core::sandbox::store::SandboxAdmissionStore;
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use std::sync::Mutex;

async fn setup(prefix: &str) -> (Seeded, PublicTrigger, PendingLaunchHomes) {
    let seed = seeded(prefix).await;
    let _resources = resources(&seed).await;
    let public = published(&seed, prefix).await;
    configure(
        &seed,
        AdmissionCeilings {
            retained_homes: Some(1000),
            ..Default::default()
        },
    )
    .await;
    let reconciler = PendingLaunchHomes::open(&seed.client, &seed.database)
        .await
        .unwrap();
    (seed, public, reconciler)
}
fn admissions(seed: &Seeded) -> mongodb::Collection<Document> {
    seed.client
        .database(&seed.database)
        .collection("sandbox_admissions")
}
fn cancel_commit(mut launch: Launch) -> LaunchCommit {
    let expected_revision = launch.revision;
    let from = launch.state;
    launch.cancel(LATER.into()).unwrap();
    LaunchCommit {
        expected_revision,
        launch,
        event_at: LATER.into(),
        events: vec![
            LaunchEventKind::StateChanged {
                from,
                to: LaunchState::Cancelled,
            },
            LaunchEventKind::Cancelled,
        ],
    }
}
async fn leak(seed: &Seeded, public: &PublicTrigger, index: usize) -> Launch {
    let launch = seed
        .store
        .admit_launch(request(public, index))
        .await
        .unwrap()
        .launch()
        .clone();
    // Reproduce a predecessor whole-record replacement discarding this marker.
    seed.client
        .database(&seed.database)
        .collection::<Document>(LAUNCHES)
        .update_one(
            doc! {"_id":launch.id.as_str()},
            doc! {"$unset":{"home_capacity_admission":""}},
        )
        .await
        .unwrap();
    seed.store
        .commit_launch(cancel_commit(launch))
        .await
        .unwrap()
}
async fn monitored_client() -> (Client, Arc<Mutex<Vec<Document>>>) {
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let commands = Arc::new(Mutex::new(Vec::new()));
    let trace = commands.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            trace.lock().unwrap().push(event.command);
        }
    }));
    (Client::with_options(options).unwrap(), commands)
}
fn writes(commands: &[Document]) -> Vec<&Document> {
    commands
        .iter()
        .filter(|command| {
            ["insert", "update", "delete", "findAndModify"]
                .iter()
                .any(|name| command.contains_key(*name))
        })
        .collect()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn terminal_leaks_release_once_on_two_replicas_without_touching_missing_or_live_launches() {
    let (seed, public, first) = setup("pending_repair").await;
    let lost = leak(&seed, &public, 0).await;
    let live = seed
        .store
        .admit_launch(request(&public, 1))
        .await
        .unwrap()
        .launch()
        .clone();
    let missing = "lch_missing_pending";
    admissions(&seed)
        .insert_one(
            doc! {"_id":{"scope":"launch_home_capacity","launch":missing},
            "document_schema_version":1_i64,"owner":"missing-owner","state":"reserved"},
        )
        .await
        .unwrap();
    admissions(&seed)
        .update_one(fence_key(), doc! {"$inc":{"pending_launch_homes":1_i64}})
        .await
        .unwrap();
    let second = PendingLaunchHomes::open(&seed.client, &seed.database)
        .await
        .unwrap();
    let (a, b) = tokio::join!(first.sweep(None), second.sweep(None));
    assert_eq!(a.unwrap().released + b.unwrap().released, 1);
    assert_eq!(pending(&seed).await, 2);
    assert_eq!(
        receipt(&seed, &lost).await.get_str("state").unwrap(),
        "released"
    );
    assert_eq!(
        receipt(&seed, &live).await.get_str("state").unwrap(),
        "reserved"
    );
    assert_eq!(
        admissions(&seed)
            .find_one(receipt_key(missing))
            .await
            .unwrap()
            .unwrap()
            .get_str("state")
            .unwrap(),
        "reserved"
    );
    let report = first.sweep(None).await.unwrap();
    assert_eq!(report.released, 0);
    assert_eq!(report.missing_launches, 1);
    assert_eq!(first.observe().await.unwrap().drift, 0);
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn repair_races_terminal_release_reservation_and_sandbox_transfer_without_drift() {
    let (seed, public, first) = setup("pending_races").await;
    let lost = leak(&seed, &public, 0).await;
    let normal = seed
        .store
        .admit_launch(request(&public, 1))
        .await
        .unwrap()
        .launch()
        .clone();
    let transfer = seed
        .store
        .admit_launch(request(&public, 2))
        .await
        .unwrap()
        .launch()
        .clone();
    let other =
        MongoTriggerDefinitionStore::open(&seed.client, &seed.database, seed.profiles.clone())
            .await
            .unwrap();
    let resource = resources(&seed).await;
    let second = PendingLaunchHomes::open(&seed.client, &seed.database)
        .await
        .unwrap();
    let home = home(&seed, &transfer, "sbx_pending_transfer", false);
    let new = request(&public, 3);
    let (a, b, c, d, e) = tokio::join!(
        first.sweep(None),
        second.sweep(None),
        other.commit_launch(cancel_commit(normal.clone())),
        other.admit_launch(new),
        resource.admit_sandbox(home)
    );
    assert_eq!(a.unwrap().released + b.unwrap().released, 1);
    c.unwrap();
    d.unwrap();
    e.unwrap();
    assert_eq!(
        receipt(&seed, &lost).await.get_str("state").unwrap(),
        "released"
    );
    assert_eq!(
        receipt(&seed, &normal).await.get_str("state").unwrap(),
        "released"
    );
    assert_eq!(
        receipt(&seed, &transfer).await.get_str("state").unwrap(),
        "consumed"
    );
    let counts = first.observe().await.unwrap();
    assert_eq!((counts.pending, counts.expected, counts.drift), (1, 1, 0));
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn detection_uses_one_read_only_snapshot_during_concurrent_reservations() {
    let (seed, public, first) = setup("pending_detect").await;
    let other =
        MongoTriggerDefinitionStore::open(&seed.client, &seed.database, seed.profiles.clone())
            .await
            .unwrap();
    let reserve = async {
        for index in 0..24 {
            other.admit_launch(request(&public, index)).await.unwrap();
        }
    };
    let observe = async {
        for _ in 0..48 {
            assert_eq!(first.observe().await.unwrap().drift, 0);
        }
    };
    tokio::join!(reserve, observe);
    assert_eq!(first.observe().await.unwrap().expected, 24);
    admissions(&seed)
        .update_one(fence_key(), doc! {"$inc":{"pending_launch_homes":5_i64}})
        .await
        .unwrap();
    let counts = first.observe().await.unwrap();
    assert_eq!((counts.pending, counts.expected, counts.drift), (29, 24, 5));
    // Pin an older read snapshot, commit a new reservation, then read the counter.
    let mut session = seed.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let before = first
        .observe_in(&mut session)
        .await
        .unwrap_or_else(|_| panic!("snapshot read failed"));
    other.admit_launch(request(&public, 25)).await.unwrap();
    let after = first
        .observe_in(&mut session)
        .await
        .unwrap_or_else(|_| panic!("snapshot reread failed"));
    assert_eq!(before, after);
    assert!(matches!(
        commit_transaction(&mut session).await,
        CommitOutcome::Committed
    ));
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn inactive_reconciliation_adds_zero_reads_and_writes_and_steady_state_adds_zero_writes() {
    let (client, commands) = monitored_client().await;
    let database = format!("cma_pending_inactive_{}", uuid::Uuid::new_v4().simple());
    let inactive = PendingLaunchHomes::open(&client, &database).await.unwrap();
    commands.lock().unwrap().clear();
    assert_eq!(inactive.observe().await.unwrap().pending, 0);
    assert_eq!(inactive.sweep(None).await.unwrap().scanned, 0);
    inactive
        .correct(
            9,
            &SubjectId::new("operator"),
            "inactive correction",
            false,
            100,
        )
        .await
        .unwrap();
    assert!(
        commands.lock().unwrap().is_empty(),
        "pre-activation reconciliation must issue no commands"
    );
    client.database(&database).drop().await.unwrap();

    let (seed, public, _) = setup("pending_steady").await;
    seed.store.admit_launch(request(&public, 0)).await.unwrap();
    let active = PendingLaunchHomes::open(&client, &seed.database)
        .await
        .unwrap();
    commands.lock().unwrap().clear();
    let report = active.sweep(None).await.unwrap();
    assert_eq!((report.scanned, report.released), (1, 0));
    let dry = active
        .correct(999, &SubjectId::new("operator"), "inspect only", true, 100)
        .await
        .unwrap();
    assert_eq!(
        (dry.pending_launch_homes, dry.expected_pending_launch_homes),
        (1, 1)
    );
    let trace = commands.lock().unwrap().clone();
    assert!(
        writes(&trace).is_empty(),
        "steady state and dry runs cannot write the fence or evidence"
    );
    let transaction_reads: Vec<_> = trace
        .iter()
        .filter(|command| command.get_bool("startTransaction").ok() == Some(true))
        .collect();
    assert!(!transaction_reads.is_empty());
    for command in transaction_reads {
        assert_eq!(
            command
                .get_document("readConcern")
                .unwrap()
                .get_str("level")
                .unwrap(),
            "snapshot"
        );
    }
    for command in trace
        .iter()
        .filter(|command| command.get_str("aggregate").ok() == Some("sandbox_admissions"))
    {
        assert_eq!(command.get_str("hint").unwrap(), INDEX);
    }
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn partial_index_excludes_settled_history_and_bounds_receipt_pages() {
    let (seed, _, reconciler) = setup("pending_index").await;
    let rows:Vec<_>=(0..1000).map(|index|doc! {"_id":{"scope":"launch_home_capacity","launch":format!("history-{index:04}")},
        "document_schema_version":1_i64,"owner":"owner","state":if index%2==0 {"released"} else {"consumed"}}).collect();
    admissions(&seed).insert_many(rows).await.unwrap();
    let rows: Vec<_> = (0..70)
        .map(|index| {
            doc! {"_id":{"scope":"launch_home_capacity","launch":format!("missing-{index:04}")},
            "document_schema_version":1_i64,"owner":"owner","state":"reserved"}
        })
        .collect();
    admissions(&seed).insert_many(rows).await.unwrap();
    admissions(&seed)
        .insert_one(doc! {"_id":{"scope":"another_scope","launch":"unrelated"},"state":"reserved"})
        .await
        .unwrap();
    let db = seed.client.database(&seed.database);
    let explained = db
        .run_command(
            doc! {"explain":{"find":"sandbox_admissions","filter":reserved_filter(),"hint":INDEX,
            "sort":{"state":1,"_id":1}},"verbosity":"executionStats"},
        )
        .await
        .unwrap();
    let stats = explained.get_document("executionStats").unwrap();
    assert_eq!(stats.get_i32("nReturned").unwrap(), 70);
    assert!(stats.get_i32("totalDocsExamined").unwrap() <= 70);
    assert!(stats.get_i32("totalKeysExamined").unwrap() <= 71);
    let plan = explained.get_document("queryPlanner").unwrap().to_string();
    assert!(plan.contains(INDEX));
    assert!(!plan.contains("COLLSCAN"));
    eprintln!("partial-index reserved=70 historical=1000 unrelated=1 executionStats={stats}");
    crate::store::pending_launch_homes::ensure_index(&admissions(&seed))
        .await
        .unwrap();
    let a = reconciler.sweep(None).await.unwrap();
    assert_eq!(a.scanned, BATCH_SIZE as u64);
    let b = reconciler.sweep(a.next).await.unwrap();
    assert_eq!(b.scanned, 6);
    assert!(b.next.is_none());
    assert_eq!(a.released + b.released, 0);
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn operator_correction_is_read_only_in_dry_run_refuses_stale_cas_and_audits_atomically() {
    let (seed, public, _) = setup("pending_operator").await;
    for index in 0..2 {
        seed.store
            .admit_launch(request(&public, index))
            .await
            .unwrap();
    }
    admissions(&seed)
        .update_one(fence_key(), doc! {"$set":{"pending_launch_homes":9_i64}})
        .await
        .unwrap();
    let (client, commands) = monitored_client().await;
    let store = crate::store::MongoAdmissionCeilingsStore::open(&client, &seed.database, 0)
        .await
        .unwrap();
    let settings = client
        .database(&seed.database)
        .collection::<Document>("platform_settings");
    let activity = client
        .database(&seed.database)
        .collection::<Document>("activity");
    let old_settings = settings.find_one(doc! {"_id":"platform"}).await.unwrap();
    let before = activity.count_documents(doc! {}).await.unwrap();
    let actor = SubjectId::new("reconciliation-operator");
    commands.lock().unwrap().clear();
    let dry = store
        .reconcile_pending_homes(8, &actor, "verify pending drift", true, 100)
        .await
        .unwrap();
    assert_eq!(
        (
            dry.previous_pending_launch_homes,
            dry.expected_pending_launch_homes,
            dry.pending_launch_homes
        ),
        (9, 2, 9)
    );
    assert!(!dry.corrected);
    assert!(writes(&commands.lock().unwrap()).is_empty());
    assert!(matches!(
        store
            .reconcile_pending_homes(8, &actor, "stale pending drift", false, 101)
            .await,
        Err(StoreError::RevisionConflict {
            expected: 8,
            actual: 9
        })
    ));
    assert!(writes(&commands.lock().unwrap()).is_empty());
    commands.lock().unwrap().clear();
    let fixed = store
        .reconcile_pending_homes(9, &actor, "reconcile verified receipts", false, 102)
        .await
        .unwrap();
    assert_eq!((fixed.pending_launch_homes, fixed.corrected), (2, true));
    assert_eq!(pending(&seed).await, 2);
    assert_eq!(activity.count_documents(doc! {}).await.unwrap(), before + 1);
    let evidence:Vec<Document>=settings.find(doc! {"_id":{"$gte":"pending_launch_homes_change:","$lt":"pending_launch_homes_change;"}})
        .await.unwrap().try_collect().await.unwrap();
    assert_eq!(evidence.len(), 1);
    let change = evidence[0].get_document("record").unwrap();
    assert_eq!(change.get_str("actor").unwrap(), actor.as_str());
    assert_eq!(
        change.get_str("reason").unwrap(),
        "reconcile verified receipts"
    );
    assert_eq!(change.get_i64("at").unwrap(), 102);
    assert_eq!(change.get_i64("old_pending_launch_homes").unwrap(), 9);
    assert_eq!(change.get_i64("new_pending_launch_homes").unwrap(), 2);
    assert_eq!(
        settings.find_one(doc! {"_id":"platform"}).await.unwrap(),
        old_settings
    );
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn unreadable_state_and_counter_underflow_never_release_a_receipt() {
    let (seed, public, reconciler) = setup("pending_corrupt").await;
    let lost = leak(&seed, &public, 0).await;
    admissions(&seed)
        .update_one(fence_key(), doc! {"$set":{"pending_launch_homes":0_i64}})
        .await
        .unwrap();
    assert_eq!(reconciler.sweep(None).await.unwrap().released, 0);
    assert_eq!(
        receipt(&seed, &lost).await.get_str("state").unwrap(),
        "reserved"
    );
    assert_eq!(pending(&seed).await, 0);
    let (client, commands) = monitored_client().await;
    let guarded = PendingLaunchHomes::open(&client, &seed.database)
        .await
        .unwrap();
    admissions(&seed)
        .update_one(
            doc! {"_id":{"scope":"registration_capacity"}},
            doc! {"$set":{"activated_at":"invalid"}},
        )
        .await
        .unwrap();
    commands.lock().unwrap().clear();
    assert!(guarded.sweep(None).await.is_err());
    assert!(guarded
        .correct(
            0,
            &SubjectId::new("operator"),
            "unreadable activation",
            false,
            100
        )
        .await
        .is_err());
    assert!(writes(&commands.lock().unwrap()).is_empty());
    assert_eq!(
        receipt(&seed, &lost).await.get_str("state").unwrap(),
        "reserved"
    );
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn callback_activation_and_new_startup_observations_enable_the_same_reconciler() {
    use crate::auth::registration::RegistrationCapacity;
    let (client, commands) = monitored_client().await;
    let name = format!("cma_pending_activation_{}", uuid::Uuid::new_v4().simple());
    let registration = crate::store::MongoRegistrationCapacityStore::open(&client, &name, false)
        .await
        .unwrap();
    let pending = PendingLaunchHomes::open(&client, &name)
        .await
        .unwrap()
        .with_activation(registration.activation_observation());
    commands.lock().unwrap().clear();
    assert!(!pending.active());
    pending.sweep(None).await.unwrap();
    assert!(commands.lock().unwrap().is_empty());
    crate::store::MongoRegistrationCapacityStore::open(&client, &name, true)
        .await
        .unwrap();
    assert!(!pending.active());
    assert!(registration.tracking_active().await.unwrap());
    assert!(pending.active());
    assert_eq!(pending.observe().await.unwrap().drift, 0);
    client.database(&name).drop().await.unwrap();

    let name = format!(
        "cma_pending_startup_activation_{}",
        uuid::Uuid::new_v4().simple()
    );
    let stale = crate::store::MongoRegistrationCapacityStore::open(&client, &name, false)
        .await
        .unwrap();
    crate::store::MongoRegistrationCapacityStore::open(&client, &name, true)
        .await
        .unwrap();
    let pending = PendingLaunchHomes::open(&client, &name)
        .await
        .unwrap()
        .with_activation(stale.activation_observation());
    assert!(
        pending.active(),
        "a positive startup read cannot be replaced by an older false observation"
    );
    assert!(stale
        .activation_observation()
        .load(std::sync::atomic::Ordering::Relaxed));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn repairing_the_same_receipt_as_a_normal_terminal_commit_decrements_once() {
    let (seed, public, reconciler) = setup("pending_same_release").await;
    let mut terminal = leak(&seed, &public, 0).await;
    seed.client
        .database(&seed.database)
        .collection::<Document>(LAUNCHES)
        .update_one(
            doc! {"_id":terminal.id.as_str()},
            doc! {"$set":{"home_capacity_admission":true}},
        )
        .await
        .unwrap();
    let expected_revision = terminal.revision;
    terminal.revision += 1;
    terminal.updated_at = LATEST.into();
    let other =
        MongoTriggerDefinitionStore::open(&seed.client, &seed.database, seed.profiles.clone())
            .await
            .unwrap();
    let (repair, normal) = tokio::join!(
        reconciler.sweep(None),
        other.commit_launch(LaunchCommit {
            expected_revision,
            launch: terminal.clone(),
            event_at: LATEST.into(),
            events: Vec::new()
        })
    );
    assert!(repair.unwrap().released <= 1);
    normal.unwrap();
    assert_eq!(pending(&seed).await, 0);
    assert_eq!(
        receipt(&seed, &terminal).await.get_str("state").unwrap(),
        "released"
    );
    assert_eq!(reconciler.observe().await.unwrap().drift, 0);
    seed.client.database(&seed.database).drop().await.unwrap();
}

async fn fault_client() -> (Client, Client, String, Arc<Mutex<Vec<Document>>>) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(uri).await.unwrap();
    let app = format!("cma-pending-fault-{}", uuid::Uuid::new_v4().simple());
    options.app_name = Some(app.clone());
    let commands = Arc::new(Mutex::new(Vec::new()));
    let trace = commands.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            trace.lock().unwrap().push(event.command);
        }
    }));
    (admin, Client::with_options(options).unwrap(), app, commands)
}
async fn inject(admin: &Client, app: &str, times: i32, mut fault: Document) {
    fault.insert("appName", app);
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":times},"data":fault})
        .await
        .unwrap();
}
async fn clear_fault(admin: &Client) {
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI disposable replica set"]
async fn correction_rolls_back_counter_and_evidence_when_audit_insertion_fails() {
    let (seed, _, _) = setup("pending_audit_fault").await;
    admissions(&seed)
        .insert_one(
            doc! {"_id":{"scope":"global_sandbox_capacity"},"document_schema_version":1_i64,
            "fence":0_i64,"pending_launch_homes":9_i64},
        )
        .await
        .unwrap();
    let (admin, client, app, _) = fault_client().await;
    let store = PendingLaunchHomes::open(&client, &seed.database)
        .await
        .unwrap();
    let db = seed.client.database(&seed.database);
    let before = db
        .collection::<Document>("activity")
        .count_documents(doc! {})
        .await
        .unwrap();
    for target in ["evidence", "activity"] {
        if target == "activity" {
            admin
                .database("admin")
                .run_command(
                    doc! {"configureFailPoint":"failCommand","mode":{"skip":1_i32},
                    "data":{"appName":&app,"failCommands":["insert"],"errorCode":121_i32}},
                )
                .await
                .unwrap();
        } else {
            inject(
                &admin,
                &app,
                1,
                doc! {"failCommands":["insert"],"errorCode":121_i32},
            )
            .await;
        }
        let failed = store
            .correct(
                9,
                &SubjectId::new("operator"),
                "faulted audit pair",
                false,
                100,
            )
            .await;
        clear_fault(&admin).await;
        assert!(
            matches!(failed, Err(StoreError::Unavailable(_))),
            "{target} insertion must fail"
        );
        assert_eq!(pending(&seed).await, 9);
        assert_eq!(
            db.collection::<Document>("activity")
                .count_documents(doc! {})
                .await
                .unwrap(),
            before
        );
        assert_eq!(db.collection::<Document>("platform_settings").count_documents(doc! {"_id":{"$gte":"pending_launch_homes_change:","$lt":"pending_launch_homes_change;"}}).await.unwrap(),0);
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI disposable replica set"]
async fn correction_exhausts_a_bounded_retry_budget_without_counter_or_audit_writes() {
    let (seed, _, _) = setup("pending_retry_fault").await;
    admissions(&seed)
        .insert_one(
            doc! {"_id":{"scope":"global_sandbox_capacity"},"document_schema_version":1_i64,
            "fence":0_i64,"pending_launch_homes":9_i64},
        )
        .await
        .unwrap();
    let (admin, client, app, commands) = fault_client().await;
    let store = PendingLaunchHomes::open(&client, &seed.database)
        .await
        .unwrap();
    commands.lock().unwrap().clear();
    inject(&admin,&app,5,doc! {"failCommands":["update"],"errorCode":112_i32,"errorLabels":["TransientTransactionError"]}).await;
    let failed = store
        .correct(
            9,
            &SubjectId::new("operator"),
            "retry exhaustion",
            false,
            100,
        )
        .await;
    clear_fault(&admin).await;
    assert!(
        matches!(failed,Err(StoreError::Unavailable(message)) if message.contains("exhausted"))
    );
    let starts = commands
        .lock()
        .unwrap()
        .iter()
        .filter(|command| command.get_bool("startTransaction").ok() == Some(true))
        .count();
    assert_eq!(starts, crate::store::sandbox_mongo::TRANSACTION_ATTEMPTS);
    assert_eq!(pending(&seed).await, 9);
    assert_eq!(seed.client.database(&seed.database).collection::<Document>("platform_settings")
        .count_documents(doc! {"_id":{"$gte":"pending_launch_homes_change:","$lt":"pending_launch_homes_change;"}}).await.unwrap(),0);
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI disposable replica set"]
async fn correction_unknown_commit_never_restarts_the_body_or_duplicates_the_audit_pair() {
    let (seed, _, _) = setup("pending_unknown_fault").await;
    admissions(&seed)
        .insert_one(
            doc! {"_id":{"scope":"global_sandbox_capacity"},"document_schema_version":1_i64,
            "fence":0_i64,"pending_launch_homes":9_i64},
        )
        .await
        .unwrap();
    let (admin, client, app, commands) = fault_client().await;
    let store = PendingLaunchHomes::open(&client, &seed.database)
        .await
        .unwrap();
    let activity = seed
        .client
        .database(&seed.database)
        .collection::<Document>("activity");
    let before = activity.count_documents(doc! {}).await.unwrap();
    commands.lock().unwrap().clear();
    inject(&admin,&app,5,doc! {"failCommands":["commitTransaction"],
        "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected pending home commit ambiguity"}}).await;
    let failed = store
        .correct(
            9,
            &SubjectId::new("operator"),
            "uncertain correction",
            false,
            100,
        )
        .await;
    clear_fault(&admin).await;
    assert!(matches!(failed, Err(StoreError::Unavailable(_))));
    let trace = commands.lock().unwrap().clone();
    assert_eq!(
        trace
            .iter()
            .filter(|command| command.get_bool("startTransaction").ok() == Some(true))
            .count(),
        1
    );
    assert_eq!(
        trace
            .iter()
            .filter(|command| command.contains_key("commitTransaction"))
            .count(),
        5
    );
    assert_eq!(pending(&seed).await, 0);
    assert_eq!(activity.count_documents(doc! {}).await.unwrap(), before + 1);
    assert_eq!(seed.client.database(&seed.database).collection::<Document>("platform_settings")
        .count_documents(doc! {"_id":{"$gte":"pending_launch_homes_change:","$lt":"pending_launch_homes_change;"}}).await.unwrap(),1);
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn completed_and_failed_terminal_leaks_are_repaired_while_retryable_failure_is_retained() {
    use layer2_core::trigger_domain::{LaunchFailure, LaunchStepStatus};
    let (seed, public, reconciler) = setup("pending_terminal_states").await;
    let mut launches = Vec::new();
    for (index, state) in [
        LaunchState::Completed,
        LaunchState::FailedTerminal,
        LaunchState::FailedRetryable,
    ]
    .into_iter()
    .enumerate()
    {
        let mut current = seed
            .store
            .admit_launch(request(&public, index))
            .await
            .unwrap()
            .launch()
            .clone();
        seed.client
            .database(&seed.database)
            .collection::<Document>(LAUNCHES)
            .update_one(
                doc! {"_id":current.id.as_str()},
                doc! {"$unset":{"home_capacity_admission":""}},
            )
            .await
            .unwrap();
        let revision = current.revision;
        current
            .start(LaunchConfirmation {
                surface: LaunchConsentSurface::Landing,
                trust_confirmed_digest: None,
                acknowledgements: vec![],
                confirmed_at: NOW.into(),
            })
            .unwrap();
        current = seed
            .store
            .commit_launch(LaunchCommit {
                expected_revision: revision,
                launch: current,
                event_at: NOW.into(),
                events: vec![
                    LaunchEventKind::ConfirmationRecorded,
                    LaunchEventKind::StateChanged {
                        from: LaunchState::Draft,
                        to: LaunchState::Resolving,
                    },
                ],
            })
            .await
            .unwrap();
        let revision = current.revision;
        if state == LaunchState::Completed {
            current.state = LaunchState::Ready;
            current.ready_at = Some(LATER.into());
            current.updated_at = LATER.into();
            current.revision += 1;
            current.first_message = FirstMessageDelivery::Completed;
            for step in &mut current.steps {
                step.status = LaunchStepStatus::Completed;
                step.started_at.get_or_insert_with(|| LATER.into());
                step.completed_at.get_or_insert_with(|| LATER.into());
            }
            current.work = layer2_core::trigger_domain::LaunchWorkState::new(
                layer2_core::trigger_domain::LaunchWorkStep::Finish,
                LATER.into(),
            );
            current = seed
                .store
                .commit_launch(LaunchCommit {
                    expected_revision: revision,
                    launch: current,
                    event_at: LATER.into(),
                    events: vec![],
                })
                .await
                .unwrap();
            let revision = current.revision;
            current
                .transition(LaunchState::Completed, LATEST.into())
                .unwrap();
            current = seed
                .store
                .commit_launch(LaunchCommit {
                    expected_revision: revision,
                    launch: current,
                    event_at: LATEST.into(),
                    events: vec![LaunchEventKind::Completed],
                })
                .await
                .unwrap();
        } else {
            let failure = LaunchFailure {
                code: "fixture_capacity_failure".into(),
                safe_detail: "A fixture resource step failed.".into(),
                retryable: state == LaunchState::FailedRetryable,
                occurred_at: LATER.into(),
            };
            current.failure = Some(failure.clone());
            current.transition(state, LATER.into()).unwrap();
            current = seed
                .store
                .commit_launch(LaunchCommit {
                    expected_revision: revision,
                    launch: current,
                    event_at: LATER.into(),
                    events: vec![
                        LaunchEventKind::StateChanged {
                            from: LaunchState::Resolving,
                            to: state,
                        },
                        LaunchEventKind::Failed { failure },
                    ],
                })
                .await
                .unwrap();
        }
        launches.push(current);
    }
    assert_eq!(reconciler.sweep(None).await.unwrap().released, 2);
    for (index, launch) in launches.iter().enumerate() {
        assert_eq!(
            receipt(&seed, launch).await.get_str("state").unwrap(),
            if index < 2 { "released" } else { "reserved" }
        );
    }
    assert_eq!(reconciler.observe().await.unwrap().pending, 1);
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[derive(Clone, Default)]
struct Events(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Events {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Events {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn drift_and_missing_launches_emit_structured_operations_events() {
    use tracing::instrument::WithSubscriber;
    let (seed, _, reconciler) = setup("pending_events").await;
    admissions(&seed)
        .insert_one(
            doc! {"_id":{"scope":"launch_home_capacity","launch":"missing-event-launch"},
            "document_schema_version":1_i64,"owner":"owner","state":"reserved"},
        )
        .await
        .unwrap();
    let output = Events::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_ansi(false)
        .with_writer(output.clone())
        .finish();
    let report = reconciler
        .sweep(None)
        .with_subscriber(subscriber)
        .await
        .unwrap();
    assert_eq!(report.missing_launches, 1);
    let text = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    let events: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let drift = events
        .iter()
        .find(|event| event["fields"]["event"] == "pending_launch_homes_drift")
        .unwrap();
    assert_eq!(drift["fields"]["pending_launch_homes"], 0);
    assert_eq!(drift["fields"]["expected_pending_launch_homes"], 1);
    assert_eq!(drift["fields"]["pending_launch_homes_drift"], -1);
    assert!(events
        .iter()
        .any(|event| event["fields"]["event"] == "pending_launch_home_missing_launch"));
    seed.client.database(&seed.database).drop().await.unwrap();
}
