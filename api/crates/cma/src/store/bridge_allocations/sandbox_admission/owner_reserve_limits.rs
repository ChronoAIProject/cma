//! The smallest usable owner limit reserves its only slot for ordinary work.
use super::*;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn owner_limit_one_refuses_fresh_channel_and_allows_manual_or_trigger_creation() {
    for trigger in [false, true] {
        let (client, database, bridges, svc, id, allocations) = setup().await;
        let resources = MongoSandboxStore::open(&client, &database).await.unwrap();
        let (admission, authority, _) =
            proposal_with_owner_limit(&svc, &bridges, &id, "limit-one", None, 1).await;
        let record = allocations
            .reserve(AllocationReservation::new(admission, authority.clone(), now()).unwrap())
            .await
            .unwrap();
        let candidate = ChannelSandboxReservation::prepare(authority, record).unwrap();
        assert_eq!(candidate.live_limit(), 1);
        assert_eq!(
            resources.sandboxes.count_documents(doc! {}).await.unwrap(),
            0
        );
        assert!(matches!(
            allocations.admit_channel_sandbox(candidate.clone()).await,
            Err(AllocationError::Capacity)
        ));
        untouched(&allocations, &resources, &candidate).await;
        assert!(matches!(
            resources
                .admit_sandbox(ordinary(&candidate, "only-ordinary", trigger))
                .await
                .unwrap(),
            SandboxAdmissionResult::Created(_)
        ));
        assert!(matches!(
            allocations.admit_channel_sandbox(candidate.clone()).await,
            Err(AllocationError::Capacity)
        ));
        untouched(&allocations, &resources, &candidate).await;
        assert_eq!(
            resources.sandboxes.count_documents(doc! {}).await.unwrap(),
            1
        );
        client.database(&database).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn retained_channel_replays_exact_receipt_under_fresh_owner_limit_one_authority() {
    let (client, database, bridges, svc, id, allocations) = setup().await;
    let resources = MongoSandboxStore::open(&client, &database).await.unwrap();
    let initial = request(&svc, &bridges, &id, &allocations, "retained").await;
    let original = allocations.admit_channel_sandbox(initial).await.unwrap();
    assert!(original.created);
    let authority = renew_retained_with_owner_limit(&svc, &bridges, &original.allocation, 1)
        .await
        .unwrap();
    let request =
        ChannelSandboxReservation::prepare(authority, original.allocation.clone()).unwrap();
    assert_eq!(request.live_limit(), 1);
    assert!(request.requires_existing());
    let reopened = MongoBridgeAllocationStore::open(bridges.clone(), None, &database)
        .await
        .unwrap();
    let replay = reopened.admit_channel_sandbox(request).await.unwrap();
    assert!(!replay.created);
    assert_eq!(replay.sandbox, original.sandbox);
    assert_eq!(replay.sidecar, original.sidecar);
    assert!(replay.allocation == original.allocation);
    for collection in [
        &resources.sandboxes,
        &resources.resource_policies,
        &resources.sandbox_events,
    ] {
        assert_eq!(collection.count_documents(doc! {}).await.unwrap(), 1);
    }
    client.database(&database).drop().await.unwrap();
}
