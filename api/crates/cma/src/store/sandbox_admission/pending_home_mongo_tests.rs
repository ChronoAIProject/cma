//! Pending Launches and ordinary Sandbox creation spend the same fleet slots.
use super::ceilings::{configure, fresh};
use super::*;
use crate::store::sandbox_mongo::DOCUMENT_SCHEMA_VERSION;
use layer2_core::admission::AdmissionCeilings;

async fn reserve(store: &MongoSandboxStore, index: usize, state: &str) {
    store
        .sandbox_admissions
        .update_one(
            doc! {"_id": {"scope": "global_sandbox_capacity"}},
            doc! {"$inc": {"fence": 1_i64, "pending_launch_homes": i64::from(state == "reserved")},
            "$setOnInsert": {"document_schema_version": DOCUMENT_SCHEMA_VERSION}},
        )
        .upsert(true)
        .await
        .unwrap();
    store
        .sandbox_admissions
        .insert_one(doc! {
            "_id": {"scope": "launch_home_capacity", "launch": format!("reserved-{index}")},
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "owner": fresh(index).sandbox.owner.as_str(), "state": state,
        })
        .await
        .unwrap();
}

fn continued(index: usize) -> SandboxAdmission {
    let mut sandbox = fresh(index).sandbox;
    sandbox.origin = SandboxOrigin::Trigger {
        launch_id: format!("reserved-{index}"),
    };
    SandboxAdmission::new(
        sandbox,
        SandboxAdmissionName::Allocate("capacity".into()),
        8,
        NOW.into(),
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn pending_launch_homes_block_ordinary_creates_and_transfer_once_after_lowering() {
    let (client, name) = database("home_pending").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    configure(
        &store,
        AdmissionCeilings {
            retained_homes: Some(1),
            ..Default::default()
        },
    )
    .await;
    reserve(&store, 0, "reserved").await;
    assert_eq!(
        store.admit_sandbox(fresh(1)).await.unwrap_err(),
        SandboxStoreError::GlobalCapacityReached
    );
    configure(
        &store,
        AdmissionCeilings {
            retained_homes: Some(0),
            ..Default::default()
        },
    )
    .await;
    let created = store.admit_sandbox(continued(0)).await.unwrap();
    assert!(matches!(created, SandboxAdmissionResult::Created(_)));
    assert!(matches!(
        store.admit_sandbox(continued(0)).await.unwrap(),
        SandboxAdmissionResult::Existing(_)
    ));
    let fence = store
        .sandbox_admissions
        .find_one(doc! {"_id": {"scope": "global_sandbox_capacity"}})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fence.get_i64("pending_launch_homes").unwrap(), 0);
    let receipt = store
        .sandbox_admissions
        .find_one(doc! {"_id": {"scope": "launch_home_capacity", "launch": "reserved-0"}})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.get_str("state").unwrap(), "consumed");
    assert_eq!(
        receipt.get_str("sandbox_id").unwrap(),
        fresh(0).sandbox.id.as_str()
    );
    assert_eq!(store.sandboxes.count_documents(doc! {}).await.unwrap(), 1);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn reused_launch_replacement_obeys_capacity_and_failed_transfer_retains_reservation() {
    let (client, name) = database("home_transfer").await;
    let store = MongoSandboxStore::open(&client, &name)
        .await
        .unwrap()
        .with_global_sandbox_limit(1);
    configure(
        &store,
        AdmissionCeilings {
            retained_homes: Some(1),
            ..Default::default()
        },
    )
    .await;
    store.admit_sandbox(fresh(0)).await.unwrap();
    reserve(&store, 1, "reused").await;
    assert_eq!(
        store.admit_sandbox(continued(1)).await.unwrap_err(),
        SandboxStoreError::GlobalCapacityReached
    );
    // A failed resource transaction cannot release the Launch's durable slot.
    reserve(&store, 2, "reserved").await;
    assert_eq!(
        store.admit_sandbox(continued(2)).await.unwrap_err(),
        SandboxStoreError::GlobalCapacityReached
    );
    let row = store
        .sandbox_admissions
        .find_one(doc! {"_id": {"scope": "global_sandbox_capacity"}})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get_i64("pending_launch_homes").unwrap(), 1);
    let receipt = store
        .sandbox_admissions
        .find_one(doc! {"_id": {"scope": "launch_home_capacity", "launch": "reserved-2"}})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.get_str("state").unwrap(), "reserved");
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn reused_marker_continuation_survives_unreadable_policy_and_pending_transfers_when_unset() {
    let (client, name) = database("home_continue").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    configure(
        &store,
        AdmissionCeilings {
            retained_homes: Some(1),
            ..Default::default()
        },
    )
    .await;
    reserve(&store, 0, "reused").await;
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$set": {"admission_ceilings": null}},
        )
        .await
        .unwrap();
    store.admit_sandbox(continued(0)).await.unwrap();
    reserve(&store, 1, "reserved").await;
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$unset": {"admission_ceilings": ""}},
        )
        .await
        .unwrap();
    store.admit_sandbox(continued(1)).await.unwrap();
    let fence = store
        .sandbox_admissions
        .find_one(doc! {"_id": {"scope": "global_sandbox_capacity"}})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fence.get_i64("pending_launch_homes").unwrap(), 0);
    assert_eq!(store.sandboxes.count_documents(doc! {}).await.unwrap(), 2);
    client.database(&name).drop().await.unwrap();
}
