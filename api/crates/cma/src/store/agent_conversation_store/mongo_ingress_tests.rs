use layer2_core::sandbox::ingress_ownership::{
    AllocationPlan, ClassBounds, IngressBudget, IngressClass, IngressOwnershipPolicy,
};
use layer2_core::sandbox::store::{AgentConversationStore, SandboxStoreError};
use mongodb::bson::{doc, RawDocumentBuf};

use super::ingress::{SnapshotLayout, SnapshotRefusal};

mod fixture;
mod ownership;
mod snapshot_error_tests;
mod snapshot_operation_tests;
mod snapshot_pressure;
mod uncertainty;

fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(128 * 1024 * 1024);
    let plan = |bytes| AllocationPlan::new("snapshot fixture", &[bytes]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(128 * 1024 * 1024, plan(1024 * 1024), plan(96 * 1024 * 1024)).unwrap(),
    )
    .unwrap();
    (root, policy)
}

#[test]
fn raw_preflight_counts_container_layout_separately_from_wire_length() {
    let scalar = RawDocumentBuf::try_from(&doc! {"payload": "x".repeat(8192)}).unwrap();
    let containers = RawDocumentBuf::try_from(&doc! {
        "payload": (0..256).map(|_| doc! {"v": 0}).collect::<Vec<_>>()
    })
    .unwrap();
    let scalar = SnapshotLayout::inspect(&scalar, &policy().1).unwrap();
    let containers = SnapshotLayout::inspect(&containers, &policy().1).unwrap();
    assert!(containers.wire_bytes < scalar.wire_bytes);
    assert!(containers.retained_bytes().unwrap() > scalar.retained_bytes().unwrap());
    assert!(containers.decode_bytes().unwrap() > containers.retained_bytes().unwrap());
}

#[test]
fn raw_preflight_refuses_malformed_and_excessively_nested_rows() {
    // A valid outer BSON header does not validate its contained elements.
    let malformed = RawDocumentBuf::from_bytes(vec![8, 0, 0, 0, 2, b'x', 0, 0]).unwrap();
    assert!(matches!(
        SnapshotLayout::inspect(&malformed, &policy().1),
        Err(SnapshotRefusal::Structure)
    ));
    let mut nested = doc! {"leaf": "synthetic"};
    for _ in 0..66 {
        nested = doc! {"nested": nested};
    }
    let nested = RawDocumentBuf::try_from(&nested).unwrap();
    assert!(matches!(
        SnapshotLayout::inspect(&nested, &policy().1),
        Err(SnapshotRefusal::Structure)
    ));
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn admitted_snapshot_matches_the_legacy_atomic_snapshot_and_retains_custody() {
    let fixture = fixture::Fixture::new("snapshot_custody").await;
    let (root, policy) = policy();
    let before = root.available_bytes();
    let legacy = fixture
        .store
        .agent_message_snapshot(&fixture.agent.id)
        .await
        .unwrap()
        .unwrap();
    let admitted = fixture
        .store
        .admitted_agent_message_snapshot(&fixture.agent.id, &policy)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(*admitted, legacy);
    assert!(root.available_bytes() < before);
    assert_eq!(
        policy
            .budget(IngressClass::Transformation)
            .available_bytes(),
        96 * 1024 * 1024
    );
    let remaining = root.available_bytes();
    let shared = admitted.clone();
    assert!(admitted.ptr_eq(&shared));
    drop(admitted);
    assert_eq!(root.available_bytes(), remaining);
    assert_eq!(shared.agent, legacy.agent);
    drop(shared);
    assert_eq!(root.available_bytes(), before);
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn refused_snapshot_keeps_existing_records_and_returns_all_partial_admission() {
    let fixture = fixture::Fixture::new("snapshot_pressure").await;
    let (root, policy) = policy();
    let data = policy.budget(IngressClass::Data);
    let data_bytes = data.available_bytes();
    let payload_bytes = data_bytes
        - layer2_core::sandbox::ingress_ownership::sizing::shared_owner_bytes::<Vec<u8>>();
    let occupied = policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new("synthetic retained pressure", &[data_bytes]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![7_u8; payload_bytes]);
    let before = root.available_bytes();
    assert!(matches!(
        fixture
            .store
            .admitted_agent_message_snapshot(&fixture.agent.id, &policy)
            .await,
        Err(error) if matches!(&*error, SandboxStoreError::Unavailable(_))
    ));
    assert_eq!(root.available_bytes(), before);
    assert_eq!(
        fixture
            .store
            .agent_message_snapshot(&fixture.agent.id)
            .await
            .unwrap()
            .unwrap()
            .agent,
        fixture.agent
    );
    let control = policy
        .admit(
            IngressClass::Control,
            AllocationPlan::checked("snapshot refusal control", &[4096]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![11_u8; 1024]);
    assert!(control.iter().all(|byte| *byte == 11));
    drop(control);
    assert_eq!(root.available_bytes(), before);
    assert_eq!(occupied.len(), payload_bytes);
    assert!(occupied.iter().all(|byte| *byte == 7));
    drop(occupied);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn near_limit_prefix_and_exact_receipt_survive_owned_snapshot_retries() {
    let mut fixture = fixture::Fixture::new("snapshot_prefix").await;
    let checkpoint = fixture
        .retain_prefix(layer2_core::sandbox::MAX_DURABLE_AGENT_MESSAGE_BYTES - 1)
        .await;
    let (root, policy) = policy();
    for _ in 0..3 {
        let snapshot = fixture
            .store
            .admitted_agent_message_snapshot(&fixture.agent.id, &policy)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.checkpoint.as_ref(), Some(&checkpoint));
        assert!(root.available_bytes() < root.capacity_bytes());
        assert_eq!(
            policy
                .budget(IngressClass::Transformation)
                .available_bytes(),
            96 * 1024 * 1024
        );
        drop(snapshot);
        assert_eq!(root.available_bytes(), root.capacity_bytes());
    }
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn corrupt_raw_checkpoint_retains_error_custody_until_final_drop() {
    let fixture = fixture::Fixture::new("snapshot_corruption").await;
    fixture
        .store
        .agents
        .update_one(
            doc! {"_id": fixture.agent.id.as_str()},
            doc! {"$set": {super::checkpoint::FIELD: {"version": 99}}},
        )
        .await
        .unwrap();
    let (root, policy) = policy();
    let error = fixture
        .store
        .admitted_agent_message_snapshot(&fixture.agent.id, &policy)
        .await
        .unwrap_err();
    assert!(matches!(&*error, SandboxStoreError::Corrupt(_)));
    assert!(error.belongs_to(&policy));
    assert!(root.available_bytes() < root.capacity_bytes());
    assert_eq!(
        policy.budget(IngressClass::Data).available_bytes(),
        policy.bounds().data_bytes()
    );
    drop(error);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
    fixture.finish().await;
}
