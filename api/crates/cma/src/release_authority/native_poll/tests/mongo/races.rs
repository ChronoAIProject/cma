use super::*;

#[tokio::test]
#[ignore = "requires owned CHRONO_TEST_MONGODB_URI replica set"]
async fn concurrent_same_command_has_only_one_fresh_admission() {
    let (db, store) = fixture().await;
    let command = prepare(&store, None).await;
    let (a, b) = tokio::join!(
        store.execute_native_poll(&command),
        store.execute_native_poll(&command)
    );
    assert_eq!(
        [&a, &b]
            .iter()
            .filter(|r| matches!(r, Ok(NativePollExecution::Fresh(_))))
            .count(),
        1
    );
    let receipt = store
        .reconcile_native_poll(&command)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(receipt.outcome(), NativePollOutcome::Empty));
    assert_eq!(
        store
            .store_commands
            .count_documents(doc! {"kind": "native_target_poll_receipt"})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .load("test-deployment")
            .await
            .unwrap()
            .unwrap()
            .admission_sequence,
        1
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires owned CHRONO_TEST_MONGODB_URI replica set"]
async fn freeze_serializes_with_empty_poll_and_closes_the_next_poll() {
    let (db, store) = fixture().await;
    let command = prepare(&store, None).await;
    let head = store.load("test-deployment").await.unwrap().unwrap();
    let request = transition(&head);
    let (freeze, poll) = tokio::join!(
        store.begin(&request, &TestEvidence),
        store.execute_native_poll(&command)
    );
    if freeze.is_err() {
        store.begin(&request, &TestEvidence).await.unwrap();
    }
    let head = store.load("test-deployment").await.unwrap().unwrap();
    assert_eq!(head.phase, Phase::Freezing);
    let receipt = store.reconcile_native_poll(&command).await.unwrap();
    assert_eq!(head.admission_sequence, i64::from(receipt.is_some()));
    assert_eq!(head.active_effects, 0);
    if let Ok(NativePollExecution::Fresh(fresh)) = poll {
        assert_eq!(receipt.as_ref().unwrap().sha256(), fresh.sha256());
    }
    assert!(matches!(
        store
            .prepare_native_poll(&writer(1, 1), NativePollLane::Background, receipt.as_ref())
            .await,
        Err(AuthorityError::Closed)
    ));
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires owned CHRONO_TEST_MONGODB_URI replica set"]
async fn cancellation_after_commit_before_caller_observation_only_reconciles() {
    let (db, store) = fixture().await;
    store
        .native_targets
        .insert_one(testing::target())
        .await
        .unwrap();
    let command = prepare(&store, None).await;
    let pending = command.clone(); // Actual caller must save before mutating await.
    let (committed_tx, committed_rx) = tokio::sync::oneshot::channel();
    let task_store = store.clone();
    let task = tokio::spawn(async move {
        let result = task_store.execute_native_poll(&command).await.unwrap();
        assert!(matches!(result, NativePollExecution::Fresh(_)));
        committed_tx.send(()).unwrap();
        // Deterministic boundary: transaction ACKed, caller has not seen result.
        std::future::pending::<()>().await;
        result
    });
    committed_rx.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let receipt = store
        .reconcile_native_poll(&pending)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(receipt.outcome(), NativePollOutcome::Claimed(_)));
    assert!(matches!(
        store.execute_native_poll(&pending).await.unwrap(),
        NativePollExecution::Replay(_)
    ));
    assert_eq!(
        store
            .load("test-deployment")
            .await
            .unwrap()
            .unwrap()
            .admission_sequence,
        1
    );
    assert_eq!(store.effects.count_documents(doc! {}).await.unwrap(), 0);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires owned CHRONO_TEST_MONGODB_URI replica set"]
async fn caller_database_cannot_supply_a_foreign_client_session() {
    let (db, store) = fixture().await;
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let foreign_client = Client::with_uri_str(uri).await.unwrap();
    // Public constructor uses only the supplied database name; all handles and
    // the transaction derive from store.client, never foreign_client.
    let reopened = AuthorityStore::open(store.client.clone(), foreign_client.database(db.name()))
        .await
        .unwrap();
    let command = prepare(&reopened, None).await;
    assert!(matches!(
        fresh(reopened.execute_native_poll(&command).await.unwrap()).outcome(),
        NativePollOutcome::Empty
    ));
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires owned CHRONO_TEST_MONGODB_URI replica set"]
async fn fixed_lanes_cannot_both_claim_the_same_original_target() {
    let (db, store) = fixture().await;
    store
        .native_targets
        .insert_one(testing::bound_target())
        .await
        .unwrap();
    let command = store
        .prepare_native_poll(&writer(1, 1), NativePollLane::StartupAgent, None)
        .await
        .unwrap();
    let receipt = fresh(store.execute_native_poll(&command).await.unwrap());
    let NativePollOutcome::Claimed(target) = receipt.outcome() else {
        panic!("startup target")
    };
    assert_eq!(
        target.binding.as_ref().unwrap().uid,
        bootstrap().pair.agent.pool_uid
    );
    // Expiry is deliberately forced while the original positive remains held.
    store
        .native_targets
        .update_one(
            doc! {"_id": &target.id},
            doc! {"$set": {"lease_until": DateTime::from_millis(1)}},
        )
        .await
        .unwrap();
    let background = prepare(&store, None).await;
    assert!(matches!(
        fresh(store.execute_native_poll(&background).await.unwrap()).outcome(),
        NativePollOutcome::Empty
    ));
    let retained = store
        .native_targets
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.lease, target.lease);
    db.drop().await.unwrap();
}
