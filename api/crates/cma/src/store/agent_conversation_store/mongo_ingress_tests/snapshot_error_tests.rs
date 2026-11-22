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

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn parent_read_preserves_missing_corrupt_and_unsupported_outcomes() {
    use crate::store::{
        agent_conversation_store::reopen::read_parent_admitted as read,
        ingress_reads::error::ReadFailure,
    };
    use layer2_core::sandbox::ingress_ownership::AdmittedError;
    let fixture = super::fixture::Fixture::new("parent_errors").await;
    let store = &fixture.store;
    let rows = &store.sandboxes;
    let original = rows
        .find_one(doc! {"_id":fixture.parent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let (root, policy) = super::policy();
    let idle = root.available_bytes();
    let mut session = store.client.start_session().await.unwrap();
    for fault in [
        "missing",
        "query",
        "version",
        "record",
        "admission",
        "finalization",
        "deep",
    ] {
        rows.delete_one(doc! {"_id":fixture.parent.id.as_str()})
            .await
            .unwrap();
        let mut row = original.clone();
        match fault {
            "query" => {
                row.insert("revision", 99_i64);
            }
            "version" => {
                row.insert("document_schema_version", 99_i64);
            }
            "record" => {
                row.get_document_mut("record")
                    .unwrap()
                    .insert("unexpected", "x".repeat(65536));
            }
            "admission" => {
                row.insert("admission_fingerprint", "a".repeat(64));
            }
            "finalization" => {
                row.insert("channel_retirement_finalization", doc! {"schema_version":1});
            }
            "deep" => {
                let mut deep = doc! {"leaf":true};
                for _ in 0..66 {
                    deep = doc! {"nested":deep};
                }
                row.insert("sidecar", deep);
            }
            _ => {}
        }
        if fault != "missing" {
            rows.insert_one(row.clone()).await.unwrap();
        }
        let result = read(
            store,
            &fixture.agent,
            &mut session,
            &policy,
            super::gate_plan(),
        )
        .unwrap()
        .await;
        if fault == "deep" {
            assert!(matches!(result, Err(AdmittedError::Unsupported(_))));
            assert!(crate::store::sandboxes::read_sandbox(&row).is_ok());
        } else {
            let error = result.unwrap().transpose().unwrap_err();
            assert!(error.belongs_to(&policy));
            if fault == "missing" {
                assert!(matches!(
                    &*error,
                    ReadFailure::Domain(SandboxStoreError::Conflict)
                ));
            } else {
                assert!(matches!(
                    &*error,
                    ReadFailure::Domain(SandboxStoreError::Corrupt(_))
                ));
                assert!(crate::store::sandboxes::read_sandbox(&row).is_err());
            }
            drop(error);
        }
        assert_eq!(root.available_bytes(), idle);
        if fault != "missing" {
            assert_eq!(
                rows.find_one(doc! {"_id":fixture.parent.id.as_str()})
                    .await
                    .unwrap(),
                Some(row)
            );
        }
    }
    drop(session);
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn parent_read_keeps_original_driver_error_and_transaction_label() {
    use crate::store::{
        agent_conversation_store::reopen::read_parent_admitted as read,
        ingress_reads::error::ReadFailure,
    };
    let mut options =
        mongodb::options::ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
    let app_name = "cma-ingress-parent-error";
    options.app_name = Some(app_name.into());
    let fixture = super::fixture::Fixture::with_client(
        mongodb::Client::with_options(options).unwrap(),
        "parent_driver",
    )
    .await;
    let store = &fixture.store;
    let admin = store.client.database("admin");
    let (root, policy) = super::policy();
    let idle = root.available_bytes();
    let mut session = store.client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    admin.run_command(doc! {"configureFailPoint":"failCommand", "mode":{"times":1}, "data":{
        "failCommands":["find"], "appName":app_name, "errorCode":112, "errorLabels":["TransientTransactionError"],
    }}).await.unwrap();
    let result = read(
        store,
        &fixture.agent,
        &mut session,
        &policy,
        super::gate_plan(),
    )
    .unwrap()
    .await;
    admin
        .run_command(doc! {"configureFailPoint":"failCommand", "mode":"off"})
        .await
        .unwrap();
    let error = result.unwrap().transpose().unwrap_err();
    let ReadFailure::Driver(driver) = &*error else {
        panic!("expected driver error")
    };
    assert!(driver.contains_label("TransientTransactionError"));
    let address = std::ptr::from_ref(driver.kind.as_ref());
    let error = error.map(|error| match error {
        ReadFailure::Driver(driver) => driver,
        _ => unreachable!(),
    });
    assert_eq!(std::ptr::from_ref(error.kind.as_ref()), address);
    assert!(error.belongs_to(&policy));
    assert!(root.available_bytes() < idle);
    drop(error);
    assert_eq!(root.available_bytes(), idle);
    let _ = session.abort_transaction().await;
    drop((session, admin));
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn integrity_point_keeps_original_driver_error_and_transient_label() {
    use crate::store::{ingress_reads::error::ReadFailure, sandbox_mongo::start_transaction};
    let app = "cma-ingress-point-error";
    let mut options =
        mongodb::options::ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
    options.app_name = Some(app.into());
    let fixture = fixture::Fixture::with_client(
        mongodb::Client::with_options(options).unwrap(),
        "point_driver",
    )
    .await;
    let store = &fixture.store;
    let admin = store.client.database("admin");
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = store.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    admin.run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":1},"data":{"failCommands":["find"],"appName":app,"errorCode":112,"errorLabels":["TransientTransactionError"]}}).await.unwrap();
    let result = store
        .integrity_point_read_admitted(
            &mut session,
            false,
            false,
            &policy,
            super::gate_plan(),
            || doc! {"_id":"point-missing"},
        )
        .unwrap()
        .await;
    admin
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    let error = result.unwrap().transpose().unwrap_err();
    let ReadFailure::Driver(driver) = &*error else {
        panic!("expected driver error")
    };
    assert!(driver.contains_label("TransientTransactionError"));
    let address = std::ptr::from_ref(driver.kind.as_ref());
    let error = error.map(|error| match error {
        ReadFailure::Driver(driver) => driver,
        _ => unreachable!(),
    });
    assert_eq!(std::ptr::from_ref(error.kind.as_ref()), address);
    assert!(error.belongs_to(&policy));
    assert!(root.available_bytes() < idle);
    drop(error);
    assert_eq!(root.available_bytes(), idle);
    let _ = session.abort_transaction().await;
    drop((session, admin));
    fixture.finish().await;
}
