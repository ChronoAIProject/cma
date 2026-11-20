use super::super::ingress_snapshot::{decode, SnapshotError};
use super::*;

#[test]
fn snapshot_decode_error_keeps_its_work_reservation_until_the_error_drops() {
    let raw = RawDocumentBuf::try_from(&doc! {
        "document_schema_version": 1,
        "record": { "unexpected": "synthetic invalid record".repeat(1024) },
    })
    .unwrap();
    let (root, policy) = policy();
    let result = decode(&raw, &policy);
    assert!(
        result.is_err(),
        "the synthetic record cannot decode as an Agent"
    );
    let SnapshotError::Owned(error) = result.as_ref().unwrap_err() else {
        panic!("decode must retain its allocated diagnostic");
    };
    assert!(error.belongs_to(&policy));
    assert!(matches!(&**error, SandboxStoreError::Corrupt(message) if !message.is_empty()));
    let held = root.available_bytes();
    drop(raw);
    assert_eq!(root.available_bytes(), held);
    assert!(
        held < root.capacity_bytes() - root.metadata_bytes(),
        "returned snapshot error lost allocation custody"
    );
    drop(result);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}

#[test]
fn retained_decode_errors_have_independent_owners_and_return_all_capacity() {
    let raw = RawDocumentBuf::try_from(&doc! {
        "document_schema_version": 1,
        "record": { "unexpected": "synthetic invalid record".repeat(1024) },
    })
    .unwrap();
    let (root, policy) = policy();
    let mut errors = Vec::with_capacity(3);
    let mut previous = root.available_bytes();
    for _ in 0..3 {
        errors.push(decode(&raw, &policy).unwrap_err());
        let current = root.available_bytes();
        assert!(
            current < previous,
            "retained snapshot error is no longer charged"
        );
        previous = current;
    }
    drop(raw);
    let held = root.available_bytes();
    drop(errors.pop());
    assert!(root.available_bytes() > held);
    assert!(root.available_bytes() < root.capacity_bytes() - root.metadata_bytes());
    drop(errors);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}

fn malformed_large_key() -> RawDocumentBuf {
    let key_bytes = 64 * 1024;
    let mut bytes = Vec::with_capacity(key_bytes + 7);
    bytes.extend_from_slice(&((key_bytes + 7) as i32).to_le_bytes());
    bytes.push(2); // String tag, but no encoded string length/value follows.
    bytes.resize(key_bytes + 5, b'x');
    bytes.extend_from_slice(&[0, 0]);
    RawDocumentBuf::from_bytes(bytes).unwrap()
}

#[test]
fn malformed_large_key_returns_only_a_fixed_refusal_after_dropping_parser_scratch() {
    let raw = malformed_large_key();
    let (root, policy) = policy();
    let result = decode(&raw, &policy);
    assert!(matches!(
        result,
        Err(SnapshotError::Refused(SnapshotRefusal::Structure))
    ));
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
    assert!(
        matches!(SnapshotRefusal::Structure.into_error(), SandboxStoreError::Unavailable(message) if message.capacity() == 0)
    );
    assert!(matches!(
        SnapshotRefusal::Structure.into_admitted_error(),
        layer2_core::sandbox::ingress_ownership::AdmittedError::Unsupported(_)
    ));
}

#[test]
fn preflight_refuses_before_reading_a_malformed_key_when_scratch_cannot_be_admitted() {
    let raw = malformed_large_key();
    let (root, policy) = policy();
    let occupied = policy
        .admit(
            IngressClass::Transformation,
            AllocationPlan::checked(
                "occupied preflight scratch",
                &[policy.bounds().transformation_bytes()],
            )
            .unwrap(),
        )
        .unwrap();
    let held = root.available_bytes();
    assert!(matches!(
        decode(&raw, &policy),
        Err(SnapshotError::Refused(SnapshotRefusal::Capacity))
    ));
    assert_eq!(root.available_bytes(), held);
    assert!(matches!(
        SnapshotRefusal::Capacity.into_admitted_error(),
        layer2_core::sandbox::ingress_ownership::AdmittedError::Refused(
            layer2_core::sandbox::ingress_ownership::AdmissionRefusal::AtCapacity { .. }
        )
    ));
    drop(occupied);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
    assert!(matches!(
        decode(&raw, &policy),
        Err(SnapshotError::Refused(SnapshotRefusal::Structure))
    ));
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}

#[test]
fn oversized_raw_row_is_refused_before_preflight_or_typed_decode() {
    let size = super::super::ingress::MAX_DOCUMENT_BYTES + 1;
    let mut bytes = vec![0; size];
    bytes[..4].copy_from_slice(&(size as i32).to_le_bytes());
    let raw = RawDocumentBuf::from_bytes(bytes).unwrap();
    let (root, policy) = policy();
    let occupied = policy
        .admit(
            IngressClass::Transformation,
            AllocationPlan::checked(
                "occupied oversized preflight",
                &[policy.bounds().transformation_bytes()],
            )
            .unwrap(),
        )
        .unwrap();
    let held = root.available_bytes();
    assert!(matches!(
        decode(&raw, &policy),
        Err(SnapshotError::Refused(SnapshotRefusal::Structure))
    ));
    assert_eq!(root.available_bytes(), held);
    drop(occupied);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn driver_read_failure_keeps_its_diagnostic_charged_until_final_drop() {
    let mut options =
        mongodb::options::ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
    let app_name = "cma-ingress-snapshot-read-failure";
    options.app_name = Some(app_name.into());
    let fixture = fixture::Fixture::with_client(
        mongodb::Client::with_options(options).unwrap(),
        "read_failure",
    )
    .await;
    let admin = fixture.store.client.database("admin");
    admin
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": {"times": 1},
            "data": {"failCommands": ["find"], "appName": app_name, "errorCode": 2},
        })
        .await
        .unwrap();
    let (root, policy) = policy();
    let result = fixture
        .store
        .admitted_agent_message_snapshot(&fixture.agent.id, &policy)
        .await;
    admin
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();
    let error = result.unwrap_err();
    assert!(
        matches!(&*error, SandboxStoreError::Unavailable(message) if message == "conversation snapshot read failed")
    );
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
    drop(admin);
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn missing_snapshot_returns_all_read_capacity() {
    let fixture = fixture::Fixture::new("missing_snapshot").await;
    fixture
        .store
        .agents
        .delete_one(doc! {"_id": fixture.agent.id.as_str()})
        .await
        .unwrap();
    let (root, policy) = policy();
    let result = fixture
        .store
        .admitted_agent_message_snapshot(&fixture.agent.id, &policy)
        .await
        .unwrap();
    assert!(result.is_none());
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
    fixture.finish().await;
}
