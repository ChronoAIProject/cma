use super::*;
use crate::store::MongoSandboxStore;
use layer2_core::sandbox::{
    store::{SandboxStore, SandboxWorkerStore},
    SandboxWorkState, SandboxWorkStep,
};

#[path = "rollback_tests.rs"]
mod rollback;

#[path = "owner_reserve_tests.rs"]
mod owner_reserve;

async fn request(
    svc: &BridgeService,
    bridges: &MongoBridgeStore,
    id: &BridgeId,
    store: &MongoBridgeAllocationStore,
    sender: &str,
) -> ChannelSandboxReservation {
    let (admission, authority, _) = proposal(svc, bridges, id, sender, None).await;
    let record = store
        .reserve(AllocationReservation::new(admission, authority.clone(), now()).unwrap())
        .await
        .unwrap();
    ChannelSandboxReservation::prepare(authority, record).unwrap()
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn held_sandbox_race_reopen_and_old_worker_selection_preserve_one_real_creation() {
    let (client, database, bridges, svc, id, allocations) = setup().await;
    let resources = MongoSandboxStore::open(&client, &database).await.unwrap();
    let request = request(&svc, &bridges, &id, &allocations, "sender").await;
    let results = futures_util::future::join_all(
        (0..4).map(|_| allocations.admit_channel_sandbox(request.clone())),
    )
    .await;
    let receipts = results.into_iter().collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(receipts.iter().filter(|r| r.created).count(), 1);
    let receipt = &receipts[0];
    assert_eq!(
        resources
            .get_sandbox(&receipt.sandbox.id)
            .await
            .unwrap()
            .as_ref(),
        Some(&receipt.sandbox)
    );
    assert!(receipt.allocation.effect_started);
    assert_eq!(receipt.allocation.state, AllocationState::NeedsConnection);
    assert!(receipt.allocation.receipt.is_none());
    let (reset, authority, management) =
        proposal(&svc, &bridges, &id, "sender", Some(&receipt.allocation)).await;
    assert!(matches!(
        AllocationReservation::reset(
            reset,
            authority,
            receipt.allocation.clone(),
            management,
            now()
        ),
        Err(AllocationError::ResetRequired)
    ));
    assert!(resources
        .due_sandbox_work("2999-01-01T00:00:00Z", 10)
        .await
        .unwrap()
        .is_empty());
    let mut ordinary = receipt.sandbox.clone();
    ordinary.id = layer2_core::SandboxId("sbx_ordinary".into());
    ordinary.name = "ordinary".into();
    ordinary.work = SandboxWorkState::new(
        SandboxWorkStep::ProvisionRuntime,
        ordinary.created_at.clone(),
    );
    resources.create_sandbox(ordinary.clone()).await.unwrap();
    assert_eq!(
        resources
            .due_sandbox_work("2999-01-01T00:00:00Z", 10)
            .await
            .unwrap(),
        vec![ordinary.id]
    );
    let reopened = MongoBridgeAllocationStore::open(bridges.clone(), None, &database)
        .await
        .unwrap();
    let authority = renew_retained(&svc, &bridges, &receipt.allocation)
        .await
        .unwrap();
    let replay = reopened
        .admit_channel_sandbox(
            ChannelSandboxReservation::prepare(authority, receipt.allocation.clone()).unwrap(),
        )
        .await
        .unwrap();
    assert!(!replay.created);
    assert_eq!(replay.sidecar, receipt.sidecar);
    for collection in ["workspaces", "worktrees", "agents"] {
        assert_eq!(
            client
                .database(&database)
                .collection::<mongodb::bson::Document>(collection)
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
    }
    assert_eq!(
        resources
            .sandbox_events
            .count_documents(doc! {"sandbox_id":receipt.sandbox.id.as_str()})
            .await
            .unwrap(),
        1
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn held_sandbox_never_retrofits_existing_rows_or_recovers_missing_creation_evidence() {
    for field in [
        "existing",
        "channel_creation",
        "sandbox_admission",
        "policy",
    ] {
        let (client, database, bridges, svc, id, allocations) = setup().await;
        let resources = MongoSandboxStore::open(&client, &database).await.unwrap();
        let request = request(&svc, &bridges, &id, &allocations, "sender").await;
        if field == "existing" {
            resources
                .create_sandbox(request.sandbox().clone())
                .await
                .unwrap();
        } else {
            allocations
                .admit_channel_sandbox(request.clone())
                .await
                .unwrap();
            match field {
                "channel_creation" => {
                    resources
                        .sandboxes
                        .update_one(
                            doc! {"_id":request.sandbox().id.as_str()},
                            doc! {"$unset":{"channel_creation":""}},
                        )
                        .await
                        .unwrap();
                }
                "sandbox_admission" => {
                    client
                        .database(&database)
                        .collection::<mongodb::bson::Document>("bridge_allocations")
                        .update_one(
                            doc! {"_id":&request.previous().id},
                            doc! {"$unset":{"sandbox_admission":""}},
                        )
                        .await
                        .unwrap();
                }
                _ => {
                    resources
                        .resource_policies
                        .delete_one(doc! {"_id":request.sandbox().id.as_str()})
                        .await
                        .unwrap();
                }
            }
        }
        assert!(
            allocations.admit_channel_sandbox(request).await.is_err(),
            "{field}"
        );
        assert_eq!(
            resources.sandboxes.count_documents(doc! {}).await.unwrap(),
            1
        );
        client.database(&database).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn held_sandbox_replay_requires_current_bridge_custody_head_and_owner() {
    for field in ["bridge", "custody", "head", "owner"] {
        let (client, database, bridges, svc, id, allocations) = setup().await;
        let resources = MongoSandboxStore::open(&client, &database).await.unwrap();
        let request = request(&svc, &bridges, &id, &allocations, "sender").await;
        allocations
            .admit_channel_sandbox(request.clone())
            .await
            .unwrap();
        let db = client.database(&database);
        match field {
            "bridge" => {
                svc.mutate(
                    &caller(),
                    None,
                    change(
                        &id,
                        2,
                        "disable",
                        BridgeChange::Disable {
                            accepted_work: AcceptedWorkPolicy::SuspendOriginal,
                        },
                    ),
                )
                .await
                .unwrap();
            }
            "custody" => {
                db.collection::<mongodb::bson::Document>("bridge_credentials")
                    .delete_many(doc! {})
                    .await
                    .unwrap();
            }
            "head" => {
                db.collection::<mongodb::bson::Document>("bridge_allocation_heads")
                    .delete_many(doc! {})
                    .await
                    .unwrap();
            }
            _ => {
                resources
                    .resource_policies
                    .update_one(
                        doc! {"_id":request.sandbox().id.as_str()},
                        doc! {"$set":{"record.owner.kind":"organization"}},
                    )
                    .await
                    .unwrap();
            }
        }
        assert!(
            allocations.admit_channel_sandbox(request).await.is_err(),
            "{field}"
        );
        assert_eq!(
            resources.sandboxes.count_documents(doc! {}).await.unwrap(),
            1
        );
        assert_eq!(
            resources
                .sandbox_events
                .count_documents(doc! {})
                .await
                .unwrap(),
            1
        );
        db.drop().await.unwrap();
    }
}
