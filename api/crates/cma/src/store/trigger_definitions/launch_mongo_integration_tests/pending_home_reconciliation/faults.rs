use super::*;

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
    let store = enabled(&client, &seed.database).await.unwrap();
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
    let store = enabled(&client, &seed.database).await.unwrap();
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
    let store = enabled(&client, &seed.database).await.unwrap();
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
    crate::store::test_visibility::majority_barrier(&admin, &seed.database).await;
    assert_eq!(pending(&seed).await, 0);
    assert_eq!(activity.count_documents(doc! {}).await.unwrap(), before + 1);
    assert_eq!(seed.client.database(&seed.database).collection::<Document>("platform_settings")
        .count_documents(doc! {"_id":{"$gte":"pending_launch_homes_change:","$lt":"pending_launch_homes_change;"}}).await.unwrap(),1);
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI disposable replica set"]
async fn unavailable_inspection_and_failed_census_never_report_healthy_zero_or_lose_usage() {
    use crate::store::{MongoAdmissionCeilingsStore, MongoRegistrationCapacityStore};
    use layer2_core::admission::PendingHomesObservation;
    let (seed, public, _) = setup("pending_read_fault").await;
    seed.store.admit_launch(request(&public, 0)).await.unwrap();
    let (admin, client, app, commands) = fault_client().await;
    let registration = MongoRegistrationCapacityStore::open(&client, &seed.database, false)
        .await
        .unwrap();
    let store = MongoAdmissionCeilingsStore::open(&client, &seed.database, 0)
        .await
        .unwrap()
        .with_activation(registration.activation_observation())
        .with_pending_home_reconciliation(true);
    for command in ["listIndexes", "aggregate"] {
        commands.lock().unwrap().clear();
        inject(
            &admin,
            &app,
            1,
            doc! {"failCommands":[command],"errorCode":13_i32},
        )
        .await;
        let result = store.read_with_history().await;
        clear_fault(&admin).await;
        let read = result.unwrap();
        assert_eq!(read.counts.pending_launch_homes, 1);
        assert_eq!(read.counts.retained_homes.unwrap().observed, 1);
        assert_eq!(
            read.counts.pending_homes,
            PendingHomesObservation::Unavailable
        );
        let trace = commands.lock().unwrap().clone();
        assert!(writes(&trace).is_empty());
        if command == "aggregate" {
            assert!(trace
                .iter()
                .any(|command| command.contains_key("abortTransaction")));
        }
    }
    assert_eq!(
        store
            .read_with_history()
            .await
            .unwrap()
            .counts
            .pending_homes,
        PendingHomesObservation::Verified {
            expected_pending_launch_homes: 1,
            pending_launch_homes_drift: 0
        }
    );
    seed.client.database(&seed.database).drop().await.unwrap();
}
