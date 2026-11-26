use super::*;

async fn owner_sandbox_ids(store: &crate::store::MongoSandboxStore) -> BTreeSet<String> {
    store
        .sandboxes
        .find(doc! {"owner":"owner"})
        .projection(doc! {"_id":1})
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.get_str("_id").unwrap().to_owned())
        .collect()
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn active_subject_hash_is_unique() {
    let (_, db, store) = fixture().await;
    let owner = SubjectId::new("owner");
    let (a, b) = tokio::join!(
        store.request(&owner, "first", NOW),
        store.request(&owner, "second", NOW)
    );
    assert_eq!(a.unwrap().id, b.unwrap().id);
    assert_eq!(
        db.collection::<Document>(RECORDS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn completed_record_has_ttl_and_no_subject() {
    let (_, db, store) = fixture().await;
    let at = layer2_core::now_rfc3339();
    let retention = Duration::from_secs(37 * 86400);
    let original = store
        .request(&SubjectId::new("owner"), "first", &at)
        .await
        .unwrap();
    let mut claimed = store
        .claim_due("worker", &at, Duration::from_secs(90))
        .await
        .unwrap()
        .unwrap();
    claimed.step = ErasureStep::VerifyNoResidue;
    claimed.residue = Some(ResidueReport {
        keys: layer2_core::account_deletion::KeyReceipt {
            remaining: 0,
            empty_at: Some(at.clone()),
        },
        ..ResidueReport::default()
    });
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    let event = claimed.complete(fence, &at, retention).unwrap();
    // Historical wire fixture: production completion needs independent proof.
    let expiry = claimed.expire_at.as_deref().unwrap();
    store
        .records
        .replace_one(
            doc! {"_id":claimed.id.as_str()},
            record_document(&claimed, &[key_hash("first")]).unwrap(),
        )
        .await
        .unwrap();
    store
        .events
        .update_many(
            doc! {"deletion_id":claimed.id.as_str()},
            doc! {"$set":{"expire_at":records::date(expiry).unwrap()}},
        )
        .await
        .unwrap();
    store
        .events
        .insert_one(event_document(&event, Some(expiry)).unwrap())
        .await
        .unwrap();
    let row = db
        .collection::<Document>(RECORDS)
        .find_one(doc! {"_id":original.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert!(!row.contains_key("subject"));
    assert!(!row.contains_key("active_subject_hash"));
    let expiry_seconds = layer2_core::seconds_from_rfc3339(&at).unwrap() + retention.as_secs();
    assert_eq!(
        row.get_datetime("expire_at").unwrap().timestamp_millis(),
        i64::try_from(expiry_seconds * 1000).unwrap()
    );
    assert_eq!(
        store.get(&original.id).await.unwrap().unwrap().status,
        AccountDeletionStatus::Completed
    );
    assert!(db
        .collection::<Document>(EVENTS)
        .find_one(doc! {"deletion_id":original.id.as_str(),"expire_at":{"$type":"date"}})
        .await
        .unwrap()
        .is_some());
    let owner = SubjectId::new("owner");
    for marked in [true, false] {
        let update = if marked {
            doc! {"$set":{"active_subject_hash":subject_hash(&owner)}}
        } else {
            doc! {"$unset":{"active_subject_hash":""}}
        };
        db.collection::<Document>(RECORDS)
            .update_one(doc! {"_id":original.id.as_str()}, update)
            .await
            .unwrap();
        assert_eq!(
            store
                .active_hashes()
                .await
                .unwrap()
                .contains(&subject_hash(&owner)),
            marked
        );
        assert_eq!(store.erasing(&owner).await.unwrap(), marked);
        assert!(!store.erasing(&SubjectId::new("other")).await.unwrap());
        let mut session = db.client().start_session().await.unwrap();
        crate::store::sandbox_mongo::start_transaction(&mut session)
            .await
            .unwrap();
        assert_eq!(
            erasing_in_session(&db, &owner, &mut session).await.unwrap(),
            marked
        );
        session.abort_transaction().await.unwrap();
    }
    db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn deletion_and_admission_race_leaves_no_new_sandbox() {
    use crate::store::sandbox_admission_mongo_integration_tests::sandbox;
    use layer2_core::sandbox::store::{
        SandboxAdmission, SandboxAdmissionName, SandboxAdmissionStore, SandboxStoreError,
    };
    let (client, db, deletions) = fixture().await;
    let sandboxes = crate::store::MongoSandboxStore::open(&client, db.name())
        .await
        .unwrap();
    let admit = |index: usize| {
        let mut record = sandbox(
            &format!("sbx_deletion_{index}"),
            &format!("launch-deletion-{index}"),
        );
        record.name = "deletion".into();
        SandboxAdmission::new(
            record,
            SandboxAdmissionName::Allocate("deletion".into()),
            100,
            NOW.into(),
        )
        .unwrap()
    };
    let owner = SubjectId::new("owner");
    let (cutover, attempts) = tokio::join!(
        async {
            deletions.request(&owner, "delete", NOW).await.unwrap();
            owner_sandbox_ids(&sandboxes).await
        },
        futures_util::future::join_all((0..8).map(|index| sandboxes.admit_sandbox(admit(index))))
    );
    let mut admitted = 0;
    for attempt in attempts {
        match attempt {
            Ok(_) => admitted += 1,
            Err(SandboxStoreError::AccountDeletionInProgress) => {}
            Err(error) => panic!("unexpected admission error: {error}"),
        }
    }
    assert_eq!(
        sandboxes
            .sandboxes
            .count_documents(doc! {"owner":"owner"})
            .await
            .unwrap(),
        admitted
    );
    assert_eq!(owner_sandbox_ids(&sandboxes).await, cutover);
    assert_eq!(
        sandboxes.admit_sandbox(admit(100)).await,
        Err(SandboxStoreError::AccountDeletionInProgress)
    );
    assert_eq!(
        sandboxes
            .sandboxes
            .count_documents(doc! {"owner":"owner"})
            .await
            .unwrap(),
        admitted
    );
    assert_eq!(owner_sandbox_ids(&sandboxes).await, cutover);
    let mut other = admit(101).sandbox;
    other.owner = SubjectId::new("other-owner");
    let other = SandboxAdmission::new(
        other,
        SandboxAdmissionName::Allocate("deletion".into()),
        100,
        NOW.into(),
    )
    .unwrap();
    assert!(sandboxes.admit_sandbox(other).await.is_ok());
    db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn persisted_request_keys_allow_only_exact_owner_replays() {
    let (_, db, store) = fixture().await;
    let owner = SubjectId::new("owner");
    store.request(&owner, "first", NOW).await.unwrap();
    assert!(store.replay(&owner, "first").await.unwrap().is_some());
    assert!(store.replay(&owner, "different").await.unwrap().is_none());
    assert!(store
        .replay(&SubjectId::new("other"), "first")
        .await
        .unwrap()
        .is_none());
    let row = db
        .collection::<Document>(RECORDS)
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    assert!(!format!("{row:?}").contains("first"));
    db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn latest_prefers_active_over_same_second_completed_history() {
    use layer2_core::account_deletion::{AccountDeletion, AccountDeletionId, KeyReceipt};
    let (_, db, store) = fixture().await;
    let at = layer2_core::now_rfc3339();
    let owner = SubjectId::new("owner");
    let mut completed = AccountDeletion::request(
        AccountDeletionId(format!("acd_{}", "f".repeat(32))),
        owner.clone(),
        &at,
    )
    .unwrap();
    completed.step = ErasureStep::VerifyNoResidue;
    completed.residue = Some(ResidueReport {
        keys: KeyReceipt {
            remaining: 0,
            empty_at: Some(at.clone()),
        },
        ..ResidueReport::default()
    });
    let fence = completed
        .claim("worker", &at, Duration::from_secs(90))
        .unwrap();
    completed
        .complete(fence, &at, Duration::from_secs(37 * 86400))
        .unwrap();
    let active = AccountDeletion::request(
        AccountDeletionId(format!("acd_{}", "0".repeat(32))),
        owner.clone(),
        &at,
    )
    .unwrap();
    db.collection::<Document>(RECORDS)
        .insert_many([
            record_document(&completed, &[key_hash("completed")]).unwrap(),
            record_document(&active, &[key_hash("active")]).unwrap(),
        ])
        .await
        .unwrap();
    assert_eq!(store.latest(&owner).await.unwrap().unwrap().id, active.id);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn active_new_keys_return_existing_without_growing_persisted_keys() {
    let (_, db, store) = fixture().await;
    let owner = SubjectId::new("owner");
    let original = store.request(&owner, "original", NOW).await.unwrap();
    for index in 0..32 {
        let key = format!("new-{index}");
        assert_eq!(
            store.request(&owner, &key, NOW).await.unwrap().id,
            original.id
        );
        assert!(store.replay(&owner, &key).await.unwrap().is_none());
    }
    let row = db
        .collection::<Document>(RECORDS)
        .find_one(doc! {"_id":original.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get_array("request_keys").unwrap().len(), 1);
    assert_eq!(
        store.replay(&owner, "original").await.unwrap().unwrap().id,
        original.id
    );
    assert!(store
        .replay(&SubjectId::new("other"), "original")
        .await
        .unwrap()
        .is_none());
    db.drop().await.unwrap();
}
