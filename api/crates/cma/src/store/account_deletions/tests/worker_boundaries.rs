use super::worker_fixture::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn distinct_competing_claims_issue_one_authority_and_retry_preserves_pending_identity() {
    let (db, _, journal, checkpoint) = enrolled().await;
    let second = reopened(&db, None).await;
    let request = |id: &str| {
        JournalRequest::new(
            checkpoint.binding.clone(),
            JournalCommand::Claim {
                request_id: id.into(),
                holder: "worker".into(),
                lease_seconds: 3,
            },
        )
        .unwrap()
    };
    let a = request("first");
    let b = request("second");
    let ba = budget();
    let bb = budget();
    let (a, b) = tokio::join!(journal.claim_due(&a, &ba), second.claim_due(&b, &bb));
    let mut claims = Vec::new();
    for outcome in [a.unwrap(), b.unwrap()] {
        match outcome {
            JournalOutcome::Recorded(recorded) => {
                claims.push(recorded.claim.expect("new claim authority"))
            }
            JournalOutcome::Idle => {}
            other => panic!("unexpected claim outcome: {other:?}"),
        }
    }
    assert_eq!(claims.len(), 1);
    let old = claims.pop().unwrap();
    let pending = plan(&old, "original-pending-identity");
    wait_until(&db, &old.lease().expires_at).await;
    let next = acquire(&second, old.checkpoint().clone(), 90).await;
    assert!(next.lease().fence > old.lease().fence);
    let JournalResolution::NotCommittedAfterFence { claim, operation } = second
        .resolve(&pending, Some(&next), &budget())
        .await
        .unwrap()
    else {
        panic!("successor fence required")
    };
    assert_eq!(operation, pending.key());
    let (receipt, _) = recorded(second.commit(&claim, &pending, &budget()).await.unwrap());
    assert_eq!(receipt.operation, pending.key());
    assert_eq!(receipt.checkpoint.binding.sequence, 1);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn cross_plan_observation_and_corrupt_receipts_refuse_without_debt_loss() {
    let (db, _, journal, claim) = claimed().await;
    let first = plan(&claim, "first");
    let (_, next) = recorded(journal.commit(&claim, &first, &budget()).await.unwrap());
    let next = next.unwrap();
    let second = plan(&next, "second");
    let (_, next) = recorded(journal.commit(&next, &second, &budget()).await.unwrap());
    let next = next.unwrap();
    let observation = JournalRequest::new(
        next.checkpoint().binding.clone(),
        JournalCommand::Observe {
            plan: first.key().into(),
            evidence: "d".repeat(64).try_into().unwrap(),
            state: ObservationState::Observed,
        },
    )
    .unwrap();
    let before = snapshot(&db).await;
    assert!(matches!(
        journal.commit(&next, &observation, &budget()).await,
        Err(JournalError::BindingMismatch)
    ));
    assert_eq!(snapshot(&db).await, before);
    let saved = journal
        .operations
        .find_one(doc! {"_id":second.key()})
        .await
        .unwrap()
        .unwrap();
    for (path, value) in [
        ("receipt.checkpoint.digest", "e".repeat(64)),
        ("receipt.lease_expires_at", LATER.into()),
        ("receipt.checkpoint.latest_plan", first.key().into()),
    ] {
        journal
            .operations
            .update_one(doc! {"_id":second.key()}, doc! {"$set":{path:value}})
            .await
            .unwrap();
        let before = snapshot(&db).await;
        assert!(matches!(
            journal.resolve(&second, None, &budget()).await,
            Err(JournalError::Incompatible)
        ));
        assert_eq!(snapshot(&db).await, before);
        journal
            .operations
            .replace_one(doc! {"_id":second.key()}, saved.clone())
            .await
            .unwrap();
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn encoded_capacity_and_cursor_bounds_leave_all_existing_debt_intact() {
    let (db, _, journal, claim) = claimed().await;
    let request = plan(&claim, "saved");
    let (_, next) = recorded(journal.commit(&claim, &request, &budget()).await.unwrap());
    let next = next.unwrap();
    let base = plan(&next, "oversized");
    let before = snapshot(&db).await;
    for cursor in ["x".repeat(4097), "invalid\npage".into()] {
        let mut changed = serde_json::to_value(&base).unwrap();
        changed["command"]["page"]["cursor"] = cursor.into();
        let request: JournalRequest = serde_json::from_value(changed).unwrap();
        assert!(matches!(
            journal.commit(&next, &request, &budget()).await,
            Err(JournalError::Capacity)
        ));
    }
    let mut changed = serde_json::to_value(&base).unwrap();
    let targets:Vec<_>=(0..500).map(|i|serde_json::json!({"class":"delegation","namespace":"\"".repeat(64),"identity":format!("{i:03}{}","\\".repeat(509)),"incarnation":"\"".repeat(512)})).collect();
    changed["command"]["page"]["targets"] = targets.into();
    let request: JournalRequest = serde_json::from_value(changed).unwrap();
    assert!(matches!(
        journal.commit(&next, &request, &budget()).await,
        Err(JournalError::Capacity)
    ));
    assert_eq!(snapshot(&db).await, before);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn revision_overflow_refuses_enrollment_without_partial_writes() {
    let (_, db, store) = fixture().await;
    crate::store::migrations::account_erasure_work::prepare_for_test(&db).await;
    let parent = store
        .request(
            &SubjectId::new("owner"),
            "accepted",
            &database_now(&db).await,
        )
        .await
        .unwrap();
    db.collection::<Document>(RECORDS)
        .update_one(
            doc! {"_id":parent.id.as_str()},
            doc! {"$set":{"revision":i64::MAX}},
        )
        .await
        .unwrap();
    let journal = MongoErasureJournal::from_database(&db);
    let request = JournalRequest::new(binding(parent.id), JournalCommand::Enroll).unwrap();
    let before = snapshot(&db).await;
    assert!(matches!(
        journal.enroll(&request, &budget()).await,
        Err(JournalError::Capacity)
    ));
    assert_eq!(snapshot(&db).await, before);
    db.drop().await.unwrap();
}
