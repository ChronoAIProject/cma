use layer2_core::sandbox::ingress_ownership::{
    AllocationPlan, ClassBounds, IngressBudget, IngressClass, IngressOwnershipPolicy,
};
use layer2_core::sandbox::store::{AgentConversationStore, SandboxStoreError};
use mongodb::bson::{doc, RawDocumentBuf};

use super::ingress::{SnapshotLayout, SnapshotRefusal};

mod fixture;
mod ownership;
mod read_port_compatibility;
mod read_port_custody;
mod read_port_races;
mod read_ports;
mod snapshot_error_tests;
mod snapshot_operation_tests;
mod snapshot_pressure;
mod snapshot_refusal_tests;
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
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
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
        assert!(root.available_bytes() < root.capacity_bytes() - root.metadata_bytes());
        assert_eq!(
            policy
                .budget(IngressClass::Transformation)
                .available_bytes(),
            96 * 1024 * 1024
        );
        drop(snapshot);
        assert_eq!(
            root.available_bytes(),
            root.capacity_bytes() - root.metadata_bytes()
        );
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
    assert!(root.available_bytes() < root.capacity_bytes() - root.metadata_bytes());
    assert_eq!(
        policy.budget(IngressClass::Data).available_bytes(),
        policy.bounds().data_bytes()
    );
    drop(error);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn seal_read_preserves_missing_and_corrupt_domains_without_quarantining_preflight() {
    use crate::store::{agent_log_segments::Seal, ingress_reads::error::ReadFailure};
    use layer2_core::sandbox::ingress_ownership::AdmittedError;
    use mongodb::bson::{self, Bson};
    let fixture = fixture::Fixture::new("seal_errors").await;
    let store = &fixture.store;
    let (head, valid) = read_ports::seal_fixture(1);
    let valid = bson::serialize_to_document(&valid).unwrap();
    let rows = &store.segment_logs.seals;
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let plan = || AllocationPlan::checked("seal error read", &[50 * 1024 * 1024]).unwrap();
    let mut session = store.client.start_session().await.unwrap();
    let absent = Seal::read_at_admitted(store, &head, 0, &mut session, false, &policy, plan())
        .unwrap()
        .await
        .unwrap()
        .transpose()
        .unwrap_err();
    assert!(matches!(
        &*absent,
        ReadFailure::Domain(SandboxStoreError::Corrupt(_))
    ));
    drop(absent);
    for (field, replacement) in [
        ("format_version", Bson::Int64(2)),
        ("entry_count", Bson::Int32(1)),
        ("previous_seal", Bson::Null),
        ("digest_version", Bson::String("unknown".into())),
        ("unexpected", Bson::String("x".repeat(64 * 1024))),
        ("unexpected", Bson::String("x".repeat(1024 * 1024))),
    ] {
        let mut row = valid.clone();
        if field == "previous_seal" {
            row.remove(field);
        } else {
            row.insert(field, replacement);
        }
        let expected = Seal::read(row.clone()).unwrap_err();
        rows.replace_one(doc! {"_id": valid.get_str("_id").unwrap()}, row.clone())
            .upsert(true)
            .await
            .unwrap();
        let error = Seal::read_at_admitted(store, &head, 0, &mut session, false, &policy, plan())
            .unwrap()
            .await
            .unwrap()
            .transpose()
            .unwrap_err();
        let ReadFailure::Domain(actual) = &*error else {
            panic!("domain classification changed")
        };
        assert_eq!(actual, &expected);
        assert!(error.belongs_to(&policy));
        assert!(root.available_bytes() < idle);
        assert_eq!(
            policy.budget(IngressClass::Data).available_bytes(),
            policy.bounds().data_bytes()
        );
        drop(error);
        assert_eq!(root.available_bytes(), idle);
        assert_eq!(
            rows.find_one(doc! {"_id":valid.get_str("_id").unwrap()})
                .await
                .unwrap(),
            Some(row)
        );
    }
    let mut row = valid.clone();
    let mut nested = doc! {"value":true};
    for _ in 0..66 {
        nested = doc! {"nested":nested};
    }
    row.insert("unsupported", nested);
    rows.replace_one(doc! {"_id":valid.get_str("_id").unwrap()}, row.clone())
        .await
        .unwrap();
    assert!(matches!(
        Seal::read_at_admitted(store, &head, 0, &mut session, false, &policy, plan())
            .unwrap()
            .await,
        Err(AdmittedError::Unsupported(_))
    ));
    assert_eq!(
        rows.find_one(doc! {"_id":valid.get_str("_id").unwrap()})
            .await
            .unwrap(),
        Some(row)
    );
    assert_eq!(root.available_bytes(), idle);
    drop(session);
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn seal_read_keeps_driver_error_identity_and_transaction_label() {
    use crate::store::{agent_log_segments::Seal, ingress_reads::error::ReadFailure};
    let mut options =
        mongodb::options::ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
    let app_name = "cma-ingress-seal-error";
    options.app_name = Some(app_name.into());
    let fixture = fixture::Fixture::with_client(
        mongodb::Client::with_options(options).unwrap(),
        "seal_driver",
    )
    .await;
    let store = &fixture.store;
    let admin = store.client.database("admin");
    let (head, _) = read_ports::seal_fixture(1);
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = store.client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    admin
        .run_command(
            doc! {"configureFailPoint":"failCommand", "mode":{"times":1}, "data":{
                "failCommands":["find"], "appName":app_name, "errorCode":112,
                "errorLabels":["TransientTransactionError"],
            }},
        )
        .await
        .unwrap();
    let plan = AllocationPlan::checked("seal driver read", &[50 * 1024 * 1024]).unwrap();
    let result = Seal::read_at_admitted(store, &head, 0, &mut session, false, &policy, plan)
        .unwrap()
        .await;
    admin
        .run_command(doc! {"configureFailPoint":"failCommand", "mode":"off"})
        .await
        .unwrap();
    let error = result.unwrap().transpose().unwrap_err();
    let ReadFailure::Driver(driver) = &*error else {
        panic!("driver classification changed")
    };
    assert!(driver.contains_label("TransientTransactionError"));
    let pointer = std::ptr::from_ref(driver.kind.as_ref());
    let moved = error.map(|error| match error {
        ReadFailure::Driver(driver) => driver,
        _ => unreachable!(),
    });
    assert_eq!(std::ptr::from_ref(moved.kind.as_ref()), pointer);
    assert!(moved.belongs_to(&policy));
    assert!(root.available_bytes() < idle);
    drop(moved);
    assert_eq!(root.available_bytes(), idle);
    let _ = session.abort_transaction().await;
    drop((session, admin));
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn containing_seal_sizes_large_predecessor_independently_and_releases_refusal() {
    use crate::store::agent_log_segments::Seal;
    use layer2_core::sandbox::ingress_ownership::AdmittedError;
    use mongodb::bson;
    let fixture = fixture::Fixture::new("containing_pressure").await;
    let store = &fixture.store;
    let (head, seals) = read_port_custody::seal_chain(10_128);
    store
        .segment_logs
        .seals
        .insert_many(
            seals
                .iter()
                .map(|s| bson::serialize_to_document(s).unwrap()),
        )
        .await
        .unwrap();
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = store.client.start_session().snapshot(true).await.unwrap();
    let pressure = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked(
                "small candidate allowance",
                &[policy.bounds().data_bytes() - 8192],
            )
            .unwrap(),
        )
        .unwrap();
    let held = root.available_bytes();
    let small = Seal::read_at_admitted(
        store,
        &head,
        1,
        &mut session,
        true,
        &policy,
        AllocationPlan::checked("neighbor read", &[50 * 1024 * 1024]).unwrap(),
    )
    .unwrap()
    .await
    .unwrap()
    .transpose()
    .unwrap();
    assert_eq!(**small, seals[1]);
    drop(small);
    let timestamp = session.snapshot_time();
    assert!(matches!(
        Seal::containing_admitted(
            store,
            &head,
            10_129,
            &mut session,
            true,
            &policy,
            AllocationPlan::checked("neighbor read", &[50 * 1024 * 1024]).unwrap()
        )
        .unwrap()
        .await,
        Err(AdmittedError::Refused(_))
    ));
    assert_eq!(root.available_bytes(), held);
    drop(pressure);
    let result = Seal::containing_admitted(
        store,
        &head,
        10_129,
        &mut session,
        true,
        &policy,
        AllocationPlan::checked("neighbor read", &[50 * 1024 * 1024]).unwrap(),
    )
    .unwrap()
    .await
    .unwrap()
    .transpose()
    .unwrap();
    assert_eq!(**result, seals[1]);
    assert_eq!(session.snapshot_time(), timestamp);
    drop((result, session));
    assert_eq!(root.available_bytes(), idle);
    fixture.finish().await;
}

use crate::store::agent_log_segments::writer_gate::{GateName, GateRecord, Policy};
use mongodb::bson::{self, Document};
pub(super) fn gate_row(name: GateName, count: usize) -> Document {
    bson::serialize_to_document(&GateRecord {
        name,
        format_version: 1,
        revision: 1,
        admission_fence: 0,
        policy: Policy {
            enabled: true,
            agents: (0..count).map(|n| format!("{n:0512}")).collect(),
            ..Policy::default()
        },
        slots: Vec::new(),
    })
    .unwrap()
}

pub(super) fn gate_plan() -> AllocationPlan {
    AllocationPlan::checked("gate read", &[50 * 1024 * 1024]).unwrap()
}
