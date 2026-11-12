use super::*;
const NOW: &str = "2026-11-14T00:00:00Z";
#[tokio::test]
async fn latest_prefers_active_when_requests_share_the_same_second() {
    let owner = SubjectId::new("owner");
    let mut completed = AccountDeletion::request(
        AccountDeletionId(format!("acd_{}", "f".repeat(32))),
        owner.clone(),
        NOW,
    )
    .unwrap();
    completed.step = ErasureStep::VerifyNoResidue;
    completed.residue = Some(ResidueReport {
        keys: KeyReceipt {
            remaining: 0,
            empty_at: Some(NOW.into()),
        },
        ..ResidueReport::default()
    });
    let fence = completed
        .claim("worker", NOW, Duration::from_secs(90))
        .unwrap();
    completed
        .complete(fence, NOW, Duration::from_secs(37 * 86400))
        .unwrap();
    let active = AccountDeletion::request(
        AccountDeletionId(format!("acd_{}", "0".repeat(32))),
        owner.clone(),
        NOW,
    )
    .unwrap();
    let records = BTreeMap::from([
        (completed.id.clone(), completed),
        (active.id.clone(), active.clone()),
    ]);
    let store = MemoryAccountDeletionStore(Mutex::new(State {
        records,
        ..State::default()
    }));
    assert_eq!(store.latest(&owner).await.unwrap().unwrap().id, active.id);
    assert!(store
        .latest(&SubjectId::new("other"))
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn active_request_key_storage_is_bounded_and_owner_scoped() {
    let store = MemoryAccountDeletionStore::default();
    let owner = SubjectId::new("owner");
    let first = store.request(&owner, "original", NOW).await.unwrap();
    for index in 0..128 {
        let key = format!("later-{index}");
        assert_eq!(store.request(&owner, &key, NOW).await.unwrap().id, first.id);
        assert!(store.replay(&owner, &key).await.unwrap().is_none());
    }
    assert_eq!(store.0.lock().unwrap().keys.len(), 1);
    assert_eq!(
        store.replay(&owner, "original").await.unwrap().unwrap().id,
        first.id
    );
    assert!(store
        .replay(&SubjectId::new("other"), "original")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn revision_only_commits_preserve_contiguous_event_sequences() {
    let store = MemoryAccountDeletionStore::default();
    let owner = SubjectId::new("owner");
    let requested = store.request(&owner, "original", NOW).await.unwrap();
    let mut record = store
        .claim_due("worker", NOW, Duration::from_secs(90))
        .await
        .unwrap()
        .unwrap();
    let lease = record.work.lease.clone().unwrap();
    let revision = record.revision;
    record
        .renew(lease.fence, "2026-11-14T00:01:00Z", Duration::from_secs(90))
        .unwrap();
    store
        .commit(record.clone(), vec![], revision, Some(&lease.token))
        .await
        .unwrap();
    let revision = record.revision;
    record.defer(lease.fence, "2026-11-14T00:03:00Z").unwrap();
    store
        .commit(record, vec![], revision, Some(&lease.token))
        .await
        .unwrap();
    assert!(store
        .claim_due("other", "2026-11-14T00:02:00Z", Duration::from_secs(90))
        .await
        .unwrap()
        .is_none());
    let mut record = store
        .claim_due("other", "2026-11-14T00:03:00Z", Duration::from_secs(90))
        .await
        .unwrap()
        .unwrap();
    let lease = record.work.lease.clone().unwrap();
    let revision = record.revision;
    let event = record
        .advance(lease.fence, "2026-11-14T00:03:00Z", BTreeMap::new())
        .unwrap();
    store
        .commit(record, vec![event], revision, Some(&lease.token))
        .await
        .unwrap();
    assert_eq!(
        store.0.lock().unwrap().events[&requested.id]
            .iter()
            .map(|event| event.seq)
            .collect::<Vec<_>>(),
        [1, 2, 3, 4]
    );
}

#[tokio::test]
async fn caller_lease_token_is_required_even_with_current_revision() {
    let store = MemoryAccountDeletionStore::default();
    let owner = SubjectId::new("owner");
    store.request(&owner, "original", NOW).await.unwrap();
    let mut record = store
        .claim_due("worker", NOW, Duration::from_secs(90))
        .await
        .unwrap()
        .unwrap();
    let lease = record.work.lease.clone().unwrap();
    let revision = record.revision;
    record
        .renew(lease.fence, NOW, Duration::from_secs(90))
        .unwrap();
    assert_eq!(
        store
            .commit(record.clone(), vec![], revision, Some("wrong-lease-token"))
            .await,
        Err(AccountDeletionError::StaleFence)
    );
    store
        .commit(record, vec![], revision, Some(&lease.token))
        .await
        .unwrap();
}
