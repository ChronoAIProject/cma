use super::*;

async fn setup_interval() -> (
    Client,
    String,
    Arc<MongoBridgeStore>,
    BridgeService,
    BridgeId,
    MongoBridgeAllocationStore,
) {
    let (client, database, bridges, svc, id, allocations) = setup().await;
    let current = bridges.bridge(&id).await.unwrap().unwrap();
    let mut config = current.current().unwrap().config.clone();
    config.limits.max_partitions = 20;
    config.limits.max_sandboxes = 20;
    svc.mutate(
        &caller(),
        None,
        change(
            &id,
            current.revision,
            "interval-capacity",
            BridgeChange::Update {
                config: Box::new(config),
                previous_work: Some(AcceptedWorkPolicy::DrainOriginal),
            },
        ),
    )
    .await
    .unwrap();
    (client, database, bridges, svc, id, allocations)
}

async fn reserve_sender(
    svc: &BridgeService,
    bridges: &MongoBridgeStore,
    id: &BridgeId,
    allocations: &MongoBridgeAllocationStore,
    sender: &str,
) -> AllocationRecord {
    let (admission, authority, _) =
        proposal_with_owner_limit(svc, bridges, id, sender, None, 20).await;
    allocations
        .reserve(AllocationReservation::new(admission, authority, now()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn allocation_interval_two_stores_share_last_slot_and_recover_receipts_at_capacity() {
    let (client, database, bridges, svc, id, allocations) = setup_interval().await;
    let first = reserve_sender(&svc, &bridges, &id, &allocations, "sender-0").await;
    for index in 1..MAX_NEW_ALLOCATIONS_PER_INTERVAL - 1 {
        reserve_sender(
            &svc,
            &bridges,
            &id,
            &allocations,
            &format!("sender-{index}"),
        )
        .await;
    }
    let reopened = MongoBridgeAllocationStore::open(bridges.clone(), None, &database)
        .await
        .unwrap();
    let (a, aa, _) = proposal_with_owner_limit(&svc, &bridges, &id, "racer-a", None, 20).await;
    let (b, ba, _) = proposal_with_owner_limit(&svc, &bridges, &id, "racer-b", None, 20).await;
    let (a, b) = tokio::join!(
        allocations.reserve(AllocationReservation::new(a, aa, now()).unwrap()),
        reopened.reserve(AllocationReservation::new(b, ba, now()).unwrap()),
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(
        matches!(a, Err(AllocationError::Capacity)) || matches!(b, Err(AllocationError::Capacity))
    );
    let (admission, authority, _) =
        proposal_with_owner_limit(&svc, &bridges, &id, "sender-0", None, 20).await;
    let recovered = reopened
        .reserve(AllocationReservation::new(admission, authority, now()).unwrap())
        .await
        .unwrap();
    assert_eq!(recovered.id, first.id);
    let records = client
        .database(&database)
        .collection::<Document>("bridge_allocations");
    assert_eq!(
        records.count_documents(doc! {}).await.unwrap(),
        MAX_NEW_ALLOCATIONS_PER_INTERVAL
    );
    assert_eq!(
        client
            .database(&database)
            .collection::<Document>("bridge_allocation_heads")
            .count_documents(doc! {})
            .await
            .unwrap(),
        MAX_NEW_ALLOCATIONS_PER_INTERVAL
    );
    assert_eq!(
        client
            .database(&database)
            .collection::<Document>("sandboxes")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn allocation_interval_reset_retries_preserve_budget_until_actual_admission_expires() {
    let (client, database, bridges, svc, id, allocations) = setup_interval().await;
    let first = reserve_sender(&svc, &bridges, &id, &allocations, "sender-0").await;
    for index in 1..MAX_NEW_ALLOCATIONS_PER_INTERVAL - 1 {
        reserve_sender(
            &svc,
            &bridges,
            &id,
            &allocations,
            &format!("sender-{index}"),
        )
        .await;
    }
    let (admission, authority, management) =
        proposal_with_owner_limit(&svc, &bridges, &id, "sender-0", Some(&first), 20).await;
    let reset = allocations
        .reserve(
            AllocationReservation::reset(admission, authority, first.clone(), management, now())
                .unwrap(),
        )
        .await
        .unwrap();
    let (admission, authority, management) =
        proposal_with_owner_limit(&svc, &bridges, &id, "sender-0", Some(&first), 20).await;
    let retry = allocations
        .reserve(
            AllocationReservation::reset(admission, authority, first, management, now()).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(retry.id, reset.id);
    let (admission, authority, management) =
        proposal_with_owner_limit(&svc, &bridges, &id, "sender-0", Some(&reset), 20).await;
    assert!(matches!(
        allocations
            .reserve(
                AllocationReservation::reset(
                    admission,
                    authority,
                    reset.clone(),
                    management,
                    now(),
                )
                .unwrap()
            )
            .await,
        Err(AllocationError::Capacity)
    ));
    assert_eq!(
        allocations
            .current(&reset.identity)
            .await
            .unwrap()
            .unwrap()
            .id,
        reset.id
    );
    // Expire only the actual admission stamps in this owned fixture. Recent
    // proposal timestamps must not keep those expired slots occupied.
    client
        .database(&database)
        .collection::<Document>("bridge_allocations")
        .update_many(
            doc! {},
            doc! {"$set": {"interval_admitted_at_ms": now() - ALLOCATION_INTERVAL_MS - 1}},
        )
        .await
        .unwrap();
    let (admission, authority, management) =
        proposal_with_owner_limit(&svc, &bridges, &id, "sender-0", Some(&reset), 20).await;
    let next = allocations
        .reserve(
            AllocationReservation::reset(admission, authority, reset, management, now()).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(next.identity.generation, 3);
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn allocation_interval_delayed_proposal_and_config_change_cannot_reset_budget() {
    let (client, database, bridges, svc, id, allocations) = setup_interval().await;
    let (admission, authority, _) =
        proposal_with_owner_limit(&svc, &bridges, &id, "delayed", None, 20).await;
    let first = allocations
        .reserve(
            AllocationReservation::new(admission, authority, now() - 2 * ALLOCATION_INTERVAL_MS)
                .unwrap(),
        )
        .await
        .unwrap();
    let records = client
        .database(&database)
        .collection::<Document>("bridge_allocations");
    let row = records
        .find_one(doc! {"_id": &first.id})
        .await
        .unwrap()
        .unwrap();
    assert!(
        row.get_i64("interval_admitted_at_ms").unwrap()
            > first.created_at_ms + ALLOCATION_INTERVAL_MS
    );
    for index in 1..MAX_NEW_ALLOCATIONS_PER_INTERVAL {
        reserve_sender(
            &svc,
            &bridges,
            &id,
            &allocations,
            &format!("sender-{index}"),
        )
        .await;
    }
    // A pre-upgrade row still occupies its recent slot using retained creation
    // time. Management replacement must not erase the other admission stamps.
    records
        .update_one(
            doc! {"_id": {"$ne": &first.id}},
            doc! {"$unset": {"interval_admitted_at_ms": ""}},
        )
        .await
        .unwrap();
    let current = bridges.bridge(&id).await.unwrap().unwrap();
    let mut config = current.current().unwrap().config.clone();
    config.display_name = "New configuration, same interval".into();
    svc.mutate(
        &caller(),
        None,
        change(
            &id,
            current.revision,
            "interval-config",
            BridgeChange::Update {
                config: Box::new(config),
                previous_work: Some(AcceptedWorkPolicy::DrainOriginal),
            },
        ),
    )
    .await
    .unwrap();
    let (admission, authority, _) =
        proposal_with_owner_limit(&svc, &bridges, &id, "after-config", None, 20).await;
    assert!(matches!(
        allocations
            .reserve(AllocationReservation::new(admission, authority, now()).unwrap())
            .await,
        Err(AllocationError::Capacity)
    ));
    assert_eq!(
        records.count_documents(doc! {}).await.unwrap(),
        MAX_NEW_ALLOCATIONS_PER_INTERVAL
    );
    client.database(&database).drop().await.unwrap();
}
