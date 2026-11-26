use super::worker_fixture::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn empty_observed_plan_cannot_complete_or_release_account_standing() {
    let (db, store, journal, claim) = claimed().await;
    let request = JournalRequest::new(
        claim.checkpoint().binding.clone(),
        JournalCommand::Prepare {
            page: PlanPage {
                targets: vec![],
                cursor: None,
            },
        },
    )
    .unwrap();
    let (_, next) = recorded(journal.commit(&claim, &request, &budget()).await.unwrap());
    let claim = next.unwrap();
    let observation = JournalRequest::new(
        claim.checkpoint().binding.clone(),
        JournalCommand::Observe {
            plan: request.key().into(),
            evidence: "d".repeat(64).try_into().unwrap(),
            state: ObservationState::Observed,
        },
    )
    .unwrap();
    let (receipt, _) = recorded(
        journal
            .commit(&claim, &observation, &budget())
            .await
            .unwrap(),
    );
    let parent = store
        .get(&receipt.checkpoint.binding.deletion_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(parent.status, AccountDeletionStatus::Running);
    assert_eq!(parent.step, ErasureStep::Inventory);
    assert!(
        parent.expire_at.is_none() && parent.completed_at.is_none() && parent.residue.is_none()
    );
    assert!(store.erasing(&SubjectId::new("owner")).await.unwrap());
    let mut changed = parent.clone();
    changed.revision += 1;
    changed.status = AccountDeletionStatus::Completed;
    let before = snapshot(&db).await;
    assert_eq!(
        store
            .commit(
                changed,
                vec![],
                parent.revision,
                Some(&parent.work.lease.as_ref().unwrap().token)
            )
            .await,
        Err(AccountDeletionError::VerificationRequired)
    );
    assert_eq!(snapshot(&db).await, before);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn journal_pages_keep_exact_debt_and_replay_without_new_authority() {
    let (db, store, journal, claim) = claimed().await;
    let original = store
        .get(&claim.checkpoint().binding.deletion_id)
        .await
        .unwrap()
        .unwrap();
    let foreign = store
        .latest(&SubjectId::new("foreign-owner"))
        .await
        .unwrap();
    let request = plan(&claim, "one");
    let (first, next) = recorded(journal.commit(&claim, &request, &budget()).await.unwrap());
    let second_client = reopened(&db, None).await;
    let before = snapshot(&db).await;
    let (replay, no_authority) = recorded(
        second_client
            .commit(&claim, &request, &budget())
            .await
            .unwrap(),
    );
    assert_eq!(replay, first);
    assert!(no_authority.is_none());
    assert_eq!(snapshot(&db).await, before);
    let claim = next.unwrap();
    let second = plan(&claim, "two");
    let (second_receipt, next) =
        recorded(journal.commit(&claim, &second, &budget()).await.unwrap());
    assert_eq!(second_receipt.checkpoint.binding.sequence, 2);
    assert_eq!(second_receipt.checkpoint.binding.inventory_generation, 2);
    let claim = next.unwrap();
    let observed = JournalRequest::new(
        claim.checkpoint().binding.clone(),
        JournalCommand::Observe {
            plan: second.key().into(),
            evidence: "d".repeat(64).try_into().unwrap(),
            state: ObservationState::Unresolved,
        },
    )
    .unwrap();
    let (receipt, _) = recorded(journal.commit(&claim, &observed, &budget()).await.unwrap());
    assert_eq!(
        receipt.checkpoint.latest_plan.as_deref(),
        Some(second.key())
    );
    assert_eq!(
        receipt.checkpoint.last_observation.as_deref(),
        Some(observed.key())
    );
    let retained = journal
        .operations
        .find(doc! {"kind":"operation"})
        .sort(doc! {"journal_sequence":1})
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    assert_eq!(retained.len(), 3);
    for (row, expected) in retained[..2].iter().zip([&request, &second]) {
        let row = crate::store::account_deletions::worker_records::operation(row.clone()).unwrap();
        assert_eq!(&row.request, expected);
    }
    let parent = store.get(&original.id).await.unwrap().unwrap();
    assert_eq!(parent.inventory, original.inventory);
    assert_eq!(parent.step, original.step);
    assert_eq!(parent.deadline_at, original.deadline_at);
    assert_eq!(parent.subject, original.subject);
    assert_eq!(
        store
            .latest(&SubjectId::new("foreign-owner"))
            .await
            .unwrap(),
        foreign
    );
    assert!(store.erasing(&SubjectId::new("owner")).await.unwrap());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn changed_binding_and_oversized_input_refuse_without_partial_progress() {
    let (db, _, journal, claim) = claimed().await;
    let request = plan(&claim, "original");
    let before = snapshot(&db).await;
    let mut changed = serde_json::to_value(&request).unwrap();
    changed["command"]["page"]["targets"][0]["incarnation"] = "replacement".into();
    let changed: JournalRequest = serde_json::from_value(changed).unwrap();
    assert!(matches!(
        journal.commit(&claim, &changed, &budget()).await,
        Err(JournalError::BindingMismatch)
    ));
    let mut changed = serde_json::to_value(&request).unwrap();
    let target = changed["command"]["page"]["targets"][0].clone();
    changed["command"]["page"]["targets"] = vec![target; 501].into();
    let changed: JournalRequest = serde_json::from_value(changed).unwrap();
    assert!(matches!(
        journal.commit(&claim, &changed, &budget()).await,
        Err(JournalError::Capacity)
    ));
    assert_eq!(snapshot(&db).await, before);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn claim_receipt_replay_cannot_grant_a_second_lease() {
    let (db, _, journal, checkpoint) = enrolled().await;
    let request = JournalRequest::new(
        checkpoint.binding,
        JournalCommand::Claim {
            request_id: "stable-claim-request".into(),
            holder: "worker".into(),
            lease_seconds: 90,
        },
    )
    .unwrap();
    let second = reopened(&db, None).await;
    let first_budget = budget();
    let second_budget = budget();
    let (a, b) = tokio::join!(
        journal.claim_due(&request, &first_budget),
        second.claim_due(&request, &second_budget)
    );
    let a = a.unwrap();
    let b = b.unwrap();
    let (a, ac) = recorded(a);
    let (b, bc) = recorded(b);
    assert_eq!(a, b);
    assert_ne!(ac.is_some(), bc.is_some());
    let before = snapshot(&db).await;
    let (receipt, authority) = recorded(second.claim_due(&request, &budget()).await.unwrap());
    assert_eq!(receipt, a);
    assert!(authority.is_none());
    assert_eq!(snapshot(&db).await, before);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn renew_defer_and_block_keep_the_original_request_deadline() {
    let (db, store, journal, claim) = claimed().await;
    let id = claim.checkpoint().binding.deletion_id.clone();
    let original = store.get(&id).await.unwrap().unwrap();
    let request = JournalRequest::new(
        claim.checkpoint().binding.clone(),
        JournalCommand::Renew { lease_seconds: 120 },
    )
    .unwrap();
    let (_, next) = recorded(journal.commit(&claim, &request, &budget()).await.unwrap());
    let claim = next.unwrap();
    let request = JournalRequest::new(
        claim.checkpoint().binding.clone(),
        JournalCommand::Defer { delay_seconds: 1 },
    )
    .unwrap();
    let (receipt, next) = recorded(journal.commit(&claim, &request, &budget()).await.unwrap());
    assert!(next.is_none());
    let parent = store.get(&id).await.unwrap().unwrap();
    wait_until(&db, &parent.work.ready_at).await;
    let claim = acquire(&journal, receipt.checkpoint, 90).await;
    let request = JournalRequest::new(
        claim.checkpoint().binding.clone(),
        JournalCommand::Block {
            reason: JournalBlock::AuthorityUnavailable,
        },
    )
    .unwrap();
    let (_, next) = recorded(journal.commit(&claim, &request, &budget()).await.unwrap());
    assert!(next.is_none());
    let parent = store.get(&id).await.unwrap().unwrap();
    assert_eq!(parent.status, AccountDeletionStatus::Blocked);
    assert_eq!(parent.deadline_at, original.deadline_at);
    assert_eq!(parent.active_subject_hash, original.active_subject_hash);
    assert_eq!(parent.step, original.step);
    db.drop().await.unwrap();
}
