//! Real owner admission races across Bridge stores and ordinary creators.
use super::*;
use layer2_core::sandbox::{
    store::{
        SandboxAdmission, SandboxAdmissionName, SandboxAdmissionResult, SandboxAdmissionStore,
    },
    SandboxOrigin,
};

#[path = "owner_reserve_contention.rs"]
mod contention;
#[path = "owner_reserve_limits.rs"]
mod limits;

async fn second_bridge(svc: &BridgeService, bridges: &MongoBridgeStore) -> BridgeId {
    let id = BridgeService::creation_id(&caller(), "create-second").unwrap();
    let mut configured = config();
    configured.partition = BridgePartitionPolicy::ConversationSender;
    configured.connector.bot_id = "second-existing-bot".into();
    configured.connector.key_id = "second-key".into();
    configured.connector.custody =
        BridgeSecretRef::new("brsec_00000000000000000000000000000002".into()).unwrap();
    svc.mutate(
        &caller(),
        None,
        change(
            &id,
            0,
            "create-second",
            BridgeChange::Create {
                owner: owner(),
                config: Box::new(configured),
            },
        ),
    )
    .await
    .unwrap();
    admit_custody(svc, bridges, &id, 1).await;
    svc.mutate(
        &caller(),
        None,
        change(&id, 1, "enable-second", BridgeChange::Enable),
    )
    .await
    .unwrap();
    id
}

fn ordinary(request: &ChannelSandboxReservation, name: &str, trigger: bool) -> SandboxAdmission {
    let mut sandbox = request.sandbox().clone();
    sandbox.id = layer2_core::SandboxId(format!("sbx_{name}"));
    sandbox.name = name.into();
    sandbox.work = SandboxWorkState::new(
        SandboxWorkStep::ProvisionRuntime,
        sandbox.created_at.clone(),
    );
    if trigger {
        sandbox.origin = SandboxOrigin::Trigger {
            launch_id: format!("launch-{name}"),
        };
    }
    let at = sandbox.created_at.clone();
    SandboxAdmission::new(
        sandbox,
        SandboxAdmissionName::Exact(name.into()),
        request.live_limit(),
        at,
    )
    .unwrap()
}

async fn untouched(
    store: &MongoBridgeAllocationStore,
    resources: &MongoSandboxStore,
    request: &ChannelSandboxReservation,
) {
    assert!(resources
        .get_sandbox(&request.sandbox().id)
        .await
        .unwrap()
        .is_none());
    assert!(
        store
            .generation(&request.previous().identity)
            .await
            .unwrap()
            .as_ref()
            == Some(request.previous())
    );
    assert_eq!(
        resources
            .resource_policies
            .count_documents(doc! {"_id": request.sandbox().id.as_str()})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        resources
            .sandbox_events
            .count_documents(doc! {"sandbox_id": request.sandbox().id.as_str()})
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn separate_bridges_race_for_owner_capacity_and_leave_one_ordinary_slot() {
    for trigger in [false, true] {
        let (client, database, bridges, svc, id, allocations) = setup().await;
        let resources = MongoSandboxStore::open(&client, &database).await.unwrap();
        let initial = request(&svc, &bridges, &id, &allocations, "initial").await;
        let original = allocations
            .admit_channel_sandbox(initial.clone())
            .await
            .unwrap();
        let second = second_bridge(&svc, &bridges).await;
        let other = MongoBridgeAllocationStore::open(bridges.clone(), None, &database)
            .await
            .unwrap();
        let a = request(&svc, &bridges, &id, &allocations, "a").await;
        let b = request(&svc, &bridges, &second, &other, "b").await;
        let (a_result, b_result) = tokio::join!(
            allocations.admit_channel_sandbox(a.clone()),
            other.admit_channel_sandbox(b.clone()),
        );
        assert_eq!(
            usize::from(a_result.is_ok()) + usize::from(b_result.is_ok()),
            1
        );
        let denied = if matches!(a_result, Err(AllocationError::Capacity)) {
            assert!(b_result.unwrap().created);
            &a
        } else {
            assert!(a_result.unwrap().created);
            assert!(matches!(b_result, Err(AllocationError::Capacity)));
            &b
        };
        untouched(&allocations, &resources, denied).await;
        assert_eq!(
            resources.sandboxes.count_documents(doc! {}).await.unwrap(),
            2
        );
        assert!(matches!(
            resources
                .admit_sandbox(ordinary(&initial, "ordinary-last", trigger))
                .await
                .unwrap(),
            SandboxAdmissionResult::Created(_)
        ));
        assert_eq!(
            resources.sandboxes.count_documents(doc! {}).await.unwrap(),
            3
        );
        // Quota exhaustion cannot invalidate an original successful receipt.
        let replay = other.admit_channel_sandbox(initial).await.unwrap();
        assert!(!replay.created);
        assert_eq!(replay.sandbox, original.sandbox);
        assert_eq!(replay.sidecar, original.sidecar);
        assert!(replay.allocation == original.allocation);
        client.database(&database).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn ordinary_admission_wins_reserved_slot_during_channel_replica_race() {
    for trigger in [false, true] {
        let (client, database, bridges, svc, id, allocations) = setup().await;
        let resources = MongoSandboxStore::open(&client, &database).await.unwrap();
        let initial = request(&svc, &bridges, &id, &allocations, "initial").await;
        allocations
            .admit_channel_sandbox(initial.clone())
            .await
            .unwrap();
        resources
            .admit_sandbox(ordinary(&initial, "ordinary-first", false))
            .await
            .unwrap();
        let candidate = request(&svc, &bridges, &id, &allocations, "candidate").await;
        let other = MongoBridgeAllocationStore::open(bridges.clone(), None, &database)
            .await
            .unwrap();
        let (a, b, regular) = tokio::join!(
            allocations.admit_channel_sandbox(candidate.clone()),
            other.admit_channel_sandbox(candidate.clone()),
            resources.admit_sandbox(ordinary(&initial, "ordinary-last", trigger)),
        );
        assert!(matches!(a, Err(AllocationError::Capacity)));
        assert!(matches!(b, Err(AllocationError::Capacity)));
        assert!(matches!(
            regular.unwrap(),
            SandboxAdmissionResult::Created(_)
        ));
        untouched(&allocations, &resources, &candidate).await;
        assert_eq!(
            resources.sandboxes.count_documents(doc! {}).await.unwrap(),
            3
        );
        client.database(&database).drop().await.unwrap();
    }
}
