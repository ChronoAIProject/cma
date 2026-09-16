use super::super::super::reuse_command_fixture::connections::Connections;
use super::*;
use futures_util::TryStreamExt;
use std::collections::BTreeMap;

async fn inventory(f: &ReuseFixture) -> BTreeMap<&'static str, Vec<Document>> {
    let database = f.original.client.database(&f.original.database);
    let mut result = BTreeMap::new();
    for name in [
        "sandboxes",
        "workspaces",
        "worktrees",
        "agents",
        "agent_creation_provenance",
        "agent_log",
        "sandbox_events",
        "agent_reuse_bindings",
        "bridge_allocations",
        "bridge_allocation_heads",
    ] {
        result.insert(
            name,
            database
                .collection::<Document>(name)
                .find(doc! {})
                .sort(doc! {"_id":1})
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap(),
        );
    }
    result
}

fn quota_refused(result: Result<OrdinaryChild, SandboxStoreError>) {
    assert!(
        matches!(
            result,
            Err(SandboxStoreError::WorkspaceLimitReached { .. })
                | Err(SandboxStoreError::AgentLimitReached { .. })
        ),
        "the actual shared physical limit must refuse fresh ordinary admission"
    );
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_owner_replay_at_full_capacity_and_quota_refusal_preserve_all_artifacts() {
    for shared_workspace in [false, true] {
        for trigger in [false, true] {
            let f = ReuseFixture::new(shared_workspace).await;
            let (one, two) = reservations(&f).await;
            let request = request(&f, shared_workspace, trigger, "ordinary-original");
            let original = admit(&f, &request).await.unwrap();
            let one = f.drive_ready(one).await;
            let two = f.drive_ready(two).await;
            assert_eq!(one.state, AllocationState::Ready);
            assert_eq!(two.state, AllocationState::Ready);
            // Materialized and reserved identities occupy each slot once.
            if !shared_workspace && !trigger {
                // Manual Workspace creation has no replay contract: its
                // existing same-name refusal must remain ahead of quota.
                assert!(matches!(
                    admit(&f, &request).await,
                    Err(SandboxStoreError::WorkspaceNameConflict)
                ));
            } else {
                let replay = admit(&f, &request).await.unwrap();
                assert_eq!(replay, original);
            }
            let before = inventory(&f).await;
            quota_refused(ordinary(&f, shared_workspace, trigger, "ordinary-over-limit").await);
            assert_eq!(
                inventory(&f).await,
                before,
                "quota refusal must roll back every lifecycle, provenance and log write"
            );
            retained(&f, &original).await;
            f.original.close().await;
        }
    }
}

async fn other_bridge(f: &ReuseFixture) -> BridgeId {
    let svc = BridgeService::new(
        f.original.bridges.clone(),
        Arc::new(Authority),
        Arc::new(Connections(f.original.resources.clone())),
    );
    let id = BridgeService::creation_id(&caller(), "shared-capacity-second").unwrap();
    let original = f
        .original
        .bridges
        .bridge(&f.original.record.identity.bridge_id)
        .await
        .unwrap()
        .unwrap();
    let mut configured = original.current().unwrap().config.clone();
    configured.connector.bot_id = "other-shared-bot".into();
    configured.connector.key_id = "other-shared-key".into();
    configured.connector.custody =
        BridgeSecretRef::new("brsec_00000000000000000000000000000002".into()).unwrap();
    svc.mutate(
        &caller(),
        None,
        change(
            &id,
            0,
            "shared-capacity-second",
            BridgeChange::Create {
                owner: owner(),
                config: Box::new(configured),
            },
        ),
    )
    .await
    .unwrap();
    admit_custody(&svc, &f.original.bridges, &id, 1).await;
    svc.mutate(
        &caller(),
        None,
        change(
            &id,
            1,
            "enable-shared-capacity-second",
            BridgeChange::Enable,
        ),
    )
    .await
    .unwrap();
    id
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_owner_admission_counts_pending_children_across_distinct_bridges() {
    for shared_workspace in [false, true] {
        let f = ReuseFixture::new(shared_workspace).await;
        let first = f.reserve("first-bridge-sender").await;
        let other = other_bridge(&f).await;
        let (admission, authority) = f.proposal_on(&other, "other-bridge-sender").await;
        let second = f
            .original
            .allocations
            .reserve(AllocationReservation::new(admission, authority, now()).unwrap())
            .await
            .unwrap();
        assert_ne!(first.identity.bridge_id, second.identity.bridge_id);
        let owner_child = ordinary(&f, shared_workspace, false, "cross-bridge-spare")
            .await
            .unwrap();
        let before = inventory(&f).await;
        quota_refused(ordinary(&f, shared_workspace, false, "cross-bridge-over-limit").await);
        assert_eq!(inventory(&f).await, before);
        assert!(f.current(&first).await == first);
        assert!(f
            .original
            .allocations
            .current(&second.identity)
            .await
            .unwrap()
            .is_some_and(|record| record == second));
        retained(&f, &owner_child).await;
        f.original.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_owner_admission_refuses_corrupt_pending_reservation_without_artifacts() {
    for shared_workspace in [false, true] {
        let f = ReuseFixture::new(shared_workspace).await;
        let held = f.reserve("corrupt-reservation").await;
        f.original
            .client
            .database(&f.original.database)
            .collection::<Document>("bridge_allocations")
            .update_one(doc! {"_id":&held.id}, doc! {"$inc":{"revision":1_i64}})
            .await
            .unwrap();
        let before = inventory(&f).await;
        assert!(matches!(
            ordinary(&f, shared_workspace, false, "corrupt-capacity-owner").await,
            Err(SandboxStoreError::Corrupt(_))
        ));
        assert_eq!(inventory(&f).await, before);
        f.original.close().await;
    }
}

async fn reserve_other(
    f: &ReuseFixture,
    bridge: &BridgeId,
) -> Result<AllocationRecord, AllocationError> {
    for _ in 0..32 {
        let (admission, authority) = f.proposal_on(bridge, "racing-other-bridge").await;
        match f
            .original
            .allocations
            .reserve(AllocationReservation::new(admission, authority, now()).unwrap())
            .await
        {
            Err(AllocationError::Conflict | AllocationError::Unavailable) => {
                tokio::task::yield_now().await
            }
            result => return result,
        }
    }
    Err(AllocationError::Unavailable)
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_manual_trigger_and_other_bridge_race_preserves_combined_capacity() {
    for shared_workspace in [false, true] {
        let f = ReuseFixture::new(shared_workspace).await;
        let first = f.reserve("original-bridge-reservation").await;
        let other = other_bridge(&f).await;
        let (channel, manual, trigger) = tokio::join!(
            reserve_other(&f, &other),
            ordinary(&f, shared_workspace, false, "racing-manual"),
            ordinary(&f, shared_workspace, true, "racing-trigger"),
        );
        let channel_created = match channel {
            Ok(record) => {
                assert_ne!(record.identity.bridge_id, first.identity.bridge_id);
                true
            }
            Err(AllocationError::Capacity) => false,
            Err(error) => panic!("unexpected Channel race result: {error}"),
        };
        let mut owner_created = 0;
        for result in [manual, trigger] {
            match result {
                Ok(child) => {
                    owner_created += 1;
                    retained(&f, &child).await;
                }
                refused => quota_refused(refused),
            }
        }
        assert_eq!(owner_created, if channel_created { 1 } else { 2 });
        assert_eq!(
            2 + owner_created + usize::from(channel_created),
            4,
            "one original ordinary child plus every accepted reservation must fit"
        );
        assert!(f.current(&first).await == first);
        f.original.close().await;
    }
}
