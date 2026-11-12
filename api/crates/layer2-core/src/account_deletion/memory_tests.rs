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
