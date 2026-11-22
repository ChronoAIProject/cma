use super::*;
use layer2_core::account_deletion::{ErasureEventKind, KeyReceipt, StepProgress};

async fn snapshot(db: &Database) -> (Vec<Document>, Vec<Document>) {
    let records = db
        .collection::<Document>(RECORDS)
        .find(doc! {})
        .sort(doc! {"_id":1})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let events = db
        .collection::<Document>(EVENTS)
        .find(doc! {})
        .sort(doc! {"_id":1})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    (records, events)
}

async fn claimed(store: &MongoAccountDeletionStore, at: &str) -> AccountDeletion {
    store
        .request(&SubjectId::new("owner"), "accepted", at)
        .await
        .unwrap();
    let mut record = store
        .claim_due("worker", at, Duration::from_secs(90))
        .await
        .unwrap()
        .unwrap();
    let revision = record.revision;
    record.inventory.sandboxes.push("synthetic-sandbox".into());
    record
        .inventory
        .pending_key_ids
        .push("synthetic-key-id".into());
    record.revision += 1;
    store
        .commit(
            record.clone(),
            vec![],
            revision,
            Some(&record.work.lease.as_ref().unwrap().token),
        )
        .await
        .unwrap();
    record
}

fn completed(mut record: AccountDeletion, at: &str) -> (AccountDeletion, ErasureEvent) {
    record.step = ErasureStep::VerifyNoResidue;
    record.residue = Some(ResidueReport {
        keys: KeyReceipt {
            remaining: 0,
            empty_at: Some(at.into()),
        },
        ..Default::default()
    });
    assert!(record.residue.as_ref().unwrap().is_zero());
    let fence = record.work.lease.as_ref().unwrap().fence;
    let event = record
        .complete(fence, at, Duration::from_secs(37 * 86400))
        .unwrap();
    (record, event)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn unverified_completion_preserves_record_audit_replay_and_replica_standing() {
    let (_, db, store) = fixture().await;
    let at = layer2_core::now_rfc3339();
    let original = claimed(&store, &at).await;
    store
        .request(&SubjectId::new("foreign-owner"), "foreign", &at)
        .await
        .unwrap();
    let before = snapshot(&db).await;
    let (finished, event) = completed(original.clone(), &at);
    let port: &dyn AccountDeletionStore = &store;
    let result = port
        .commit(
            finished,
            vec![event],
            original.revision,
            Some(&original.work.lease.as_ref().unwrap().token),
        )
        .await;
    assert_eq!(result, Err(AccountDeletionError::VerificationRequired));
    assert_eq!(snapshot(&db).await, before);
    let second_client =
        mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
    let reopened = MongoAccountDeletionStore::from_database(&second_client.database(db.name()));
    assert_eq!(reopened.get(&original.id).await.unwrap().unwrap(), original);
    assert_eq!(
        reopened
            .replay(&SubjectId::new("owner"), "accepted")
            .await
            .unwrap()
            .unwrap(),
        original
    );
    assert!(reopened.erasing(&SubjectId::new("owner")).await.unwrap());
    assert!(reopened
        .active_hashes()
        .await
        .unwrap()
        .contains(&original.subject_hash));
    assert!(!reopened
        .erasing(&SubjectId::new("unrelated-owner"))
        .await
        .unwrap());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn partial_completion_fields_and_events_cannot_bypass_the_proof_gate() {
    let (_, db, store) = fixture().await;
    let at = layer2_core::now_rfc3339();
    let original = claimed(&store, &at).await;
    let before = snapshot(&db).await;
    for case in [
        "status",
        "completed_at",
        "expire_at",
        "reverify_at",
        "verified_step",
        "completed_event",
    ] {
        let mut next = original.clone();
        next.revision += 1;
        let mut events = vec![];
        match case {
            "status" => next.status = AccountDeletionStatus::Completed,
            "completed_at" => next.completed_at = Some(at.clone()),
            "expire_at" => next.expire_at = Some("2099-01-01T00:00:00Z".into()),
            "reverify_at" => next.reverify_at = Some(at.clone()),
            "verified_step" => next.steps.push(StepProgress {
                step: ErasureStep::VerifyNoResidue,
                completed_at: at.clone(),
                counts: Default::default(),
            }),
            "completed_event" => {
                next.step = ErasureStep::VerifyNoResidue;
                next.event_seq += 1;
                events.push(next.current_event(
                    &at,
                    ErasureEventKind::Completed {
                        counts: Default::default(),
                    },
                ));
            }
            _ => unreachable!(),
        }
        let result = store
            .commit(
                next,
                events,
                original.revision,
                Some(&original.work.lease.as_ref().unwrap().token),
            )
            .await;
        assert_eq!(
            result,
            Err(AccountDeletionError::VerificationRequired),
            "{case}"
        );
        assert_eq!(snapshot(&db).await, before, "{case}");
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn pending_commit_cannot_scrub_or_reassign_accepted_identity_or_reset_deadline() {
    let (_, db, store) = fixture().await;
    let at = layer2_core::now_rfc3339();
    let original = claimed(&store, &at).await;
    let before = snapshot(&db).await;
    for case in [
        "subject",
        "active_marker",
        "retarget",
        "request_time",
        "deadline",
    ] {
        let mut next = original.clone();
        next.revision += 1;
        match case {
            "subject" => next.subject = None,
            "active_marker" => next.active_subject_hash = None,
            "retarget" => {
                let other = SubjectId::new("unrelated-owner");
                next.subject_hash = subject_hash(&other);
                next.active_subject_hash = Some(next.subject_hash.clone());
                next.subject = Some(other);
            }
            "request_time" => next.requested_at = "2099-01-01T00:00:00Z".into(),
            "deadline" => next.deadline_at = "2099-01-08T00:00:00Z".into(),
            _ => unreachable!(),
        }
        let result = store
            .commit(
                next,
                vec![],
                original.revision,
                Some(&original.work.lease.as_ref().unwrap().token),
            )
            .await;
        assert_eq!(result, Err(AccountDeletionError::Invalid), "{case}");
        assert_eq!(snapshot(&db).await, before, "{case}");
        assert!(store.erasing(&SubjectId::new("owner")).await.unwrap());
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn unprepared_store_refuses_completion_before_any_database_command() {
    use mongodb::{event::command::CommandEvent, event::EventHandler, options::ClientOptions};
    use std::sync::{Arc, Mutex};
    let name = format!("cma_acd_proof_absent_{}", uuid::Uuid::new_v4().simple());
    let observed = Arc::new(Mutex::new(Vec::new()));
    let commands = observed.clone();
    let observed_name = name.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.db == observed_name {
                commands.lock().unwrap().push(event.command_name);
            }
        }
    }));
    let client = mongodb::Client::with_options(options).unwrap();
    let db = client.database(&name);
    let store = MongoAccountDeletionStore::from_database(&db);
    let at = layer2_core::now_rfc3339();
    let mut request =
        AccountDeletion::request(AccountDeletionId::generate(), SubjectId::new("owner"), &at)
            .unwrap();
    request
        .claim("worker", &at, Duration::from_secs(90))
        .unwrap();
    let lease = request.work.lease.clone().unwrap();
    let revision = request.revision;
    let (finished, event) = completed(request, &at);
    assert_eq!(
        store
            .commit(finished, vec![event], revision, Some(&lease.token))
            .await,
        Err(AccountDeletionError::VerificationRequired)
    );
    assert!(observed.lock().unwrap().is_empty());
    assert!(db.list_collection_names().await.unwrap().is_empty());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn historical_completed_records_are_readable_but_general_writer_cannot_reactivate_them() {
    let (_, db, store) = fixture().await;
    let at = layer2_core::now_rfc3339();
    let mut record =
        AccountDeletion::request(AccountDeletionId::generate(), SubjectId::new("owner"), &at)
            .unwrap();
    record
        .claim("historical-worker", &at, Duration::from_secs(90))
        .unwrap();
    let (historical, event) = completed(record, &at);
    store
        .records
        .insert_one(record_document(&historical, &[key_hash("historical")]).unwrap())
        .await
        .unwrap();
    store
        .events
        .insert_one(event_document(&event, historical.expire_at.as_deref()).unwrap())
        .await
        .unwrap();
    let before = snapshot(&db).await;
    assert_eq!(
        store.get(&historical.id).await.unwrap().unwrap(),
        historical
    );
    assert_eq!(
        store
            .replay(&SubjectId::new("owner"), "historical")
            .await
            .unwrap()
            .unwrap(),
        historical
    );
    let mut replacement =
        AccountDeletion::request(historical.id.clone(), SubjectId::new("owner"), &at).unwrap();
    replacement.revision = historical.revision + 1;
    replacement.event_seq = historical.event_seq;
    replacement.work.next_fence = historical.work.next_fence;
    assert_eq!(
        store
            .commit(replacement, vec![], historical.revision, None)
            .await,
        Err(AccountDeletionError::VerificationRequired)
    );
    assert_eq!(snapshot(&db).await, before);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn verification_pending_work_can_block_rearm_and_defer_without_losing_authority() {
    let (_, db, store) = fixture().await;
    let at = layer2_core::now_rfc3339();
    let mut record = claimed(&store, &at).await;
    let original = record.clone();
    let lease = record.work.lease.clone().unwrap();
    record.step = ErasureStep::VerifyNoResidue;
    let event = record
        .block(lease.fence, &at, ErasureBlock::OwnerAuthorityUnavailable)
        .unwrap();
    store
        .commit(
            record.clone(),
            vec![event],
            original.revision,
            Some(&lease.token),
        )
        .await
        .unwrap();
    let revision = record.revision;
    let event = record.rearm(&at).unwrap();
    store
        .commit(record.clone(), vec![event], revision, None)
        .await
        .unwrap();
    let mut record = store
        .claim_due("successor", &at, Duration::from_secs(90))
        .await
        .unwrap()
        .unwrap();
    let lease = record.work.lease.clone().unwrap();
    let revision = record.revision;
    record.defer(lease.fence, &at).unwrap();
    store
        .commit(record.clone(), vec![], revision, Some(&lease.token))
        .await
        .unwrap();
    let reopened = MongoAccountDeletionStore::from_database(&db);
    let persisted = reopened.get(&record.id).await.unwrap().unwrap();
    assert_eq!(persisted, record);
    assert_eq!(persisted.inventory, original.inventory);
    assert_eq!(persisted.requested_at, original.requested_at);
    assert_eq!(persisted.deadline_at, original.deadline_at);
    assert_eq!(persisted.subject, original.subject);
    assert_eq!(persisted.active_subject_hash, original.active_subject_hash);
    assert!(persisted.completed_at.is_none() && persisted.expire_at.is_none());
    assert!(reopened.erasing(&SubjectId::new("owner")).await.unwrap());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn intermediate_completed_step_events_remain_writable_until_final_verification() {
    let (_, db, store) = fixture().await;
    let at = layer2_core::now_rfc3339();
    let mut record = claimed(&store, &at).await;
    let original = record.clone();
    for step in ERASURE_ORDER.into_iter().take(6) {
        assert_eq!(record.step, step);
        let lease = record.work.lease.clone().unwrap();
        let revision = record.revision;
        let event = record
            .advance(lease.fence, &at, Default::default())
            .unwrap();
        assert_eq!(event.step, step);
        assert!(matches!(event.kind, ErasureEventKind::Completed { .. }));
        store
            .commit(record, vec![event], revision, Some(&lease.token))
            .await
            .unwrap();
        record = store
            .claim_due("successor", &at, Duration::from_secs(90))
            .await
            .unwrap()
            .unwrap();
    }
    assert_eq!(record.step, ErasureStep::VerifyNoResidue);
    assert_eq!(record.status, AccountDeletionStatus::Running);
    assert_eq!(record.subject, original.subject);
    assert_eq!(record.deadline_at, original.deadline_at);
    assert_eq!(record.steps.len(), 6);
    let before = snapshot(&db).await;
    let revision = record.revision;
    let token = record.work.lease.as_ref().unwrap().token.clone();
    let (finished, event) = completed(record, &at);
    assert_eq!(
        store
            .commit(finished, vec![event], revision, Some(&token))
            .await,
        Err(AccountDeletionError::VerificationRequired)
    );
    assert_eq!(snapshot(&db).await, before);
    db.drop().await.unwrap();
}
