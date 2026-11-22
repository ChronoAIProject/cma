use super::*;

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
    let inactive = enabled(&client, &database).await.unwrap();
    commands.lock().unwrap().clear();
    assert_eq!(
        inactive.readiness().await,
        Err(layer2_core::admission::PendingHomesObservation::CapacityInactive)
    );
    assert!(inactive.observe().await.is_err());
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
        .unwrap_err();
    assert!(
        commands.lock().unwrap().is_empty(),
        "pre-activation reconciliation must issue no commands"
    );
    client.database(&database).drop().await.unwrap();

    let (seed, public, _) = setup("pending_steady").await;
    seed.store.admit_launch(request(&public, 0)).await.unwrap();
    let active = enabled(&client, &seed.database).await.unwrap();
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
    prepare_index(&seed.client, &seed.database).await;
    let a = reconciler.sweep(None).await.unwrap();
    assert_eq!(a.scanned, BATCH_SIZE as u64);
    let b = reconciler.sweep(a.next).await.unwrap();
    assert_eq!(b.scanned, 6);
    assert!(b.next.is_none());
    assert_eq!(a.released + b.released, 0);
    seed.client.database(&seed.database).drop().await.unwrap();
}
