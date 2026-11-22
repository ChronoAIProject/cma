use super::{doc, fixture::Fixture, policy, AllocationPlan, IngressClass, SandboxStoreError};
use layer2_core::sandbox::ingress_ownership::{AdmissionRefusal, AdmittedError};
use layer2_core::sandbox::store::AgentConversationStore;
use mongodb::bson::Bson;

fn ignored_sidecars() -> [Bson; 2] {
    let mut nested = doc! {"leaf": "synthetic"};
    for _ in 0..66 {
        nested = doc! {"nested": nested};
    }
    let pointer = serde_json::from_value(serde_json::json!({
        "$dbPointer": {
            "$ref": "synthetic.collection",
            "$id": {"$oid": "000000000000000000000001"}
        }
    }))
    .unwrap();
    assert!(matches!(pointer, Bson::DbPointer(_)));
    [Bson::Document(nested), pointer]
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn valid_ignored_sidecars_refuse_without_reporting_snapshot_corruption() {
    let mut fixture = Fixture::new("snapshot_unsupported").await;
    let checkpoint = fixture.retain_prefix(128).await;
    let baseline = fixture
        .store
        .agent_message_snapshot(&fixture.agent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(baseline.checkpoint.as_ref(), Some(&checkpoint));
    let (root, policy) = policy();
    for sidecar in ignored_sidecars() {
        fixture
            .store
            .agents
            .update_one(
                doc! {"_id": fixture.agent.id.as_str()},
                doc! {"$set": {"ignored_sidecar": sidecar}},
            )
            .await
            .unwrap();
        let before = fixture
            .store
            .agents
            .find_one(doc! {"_id": fixture.agent.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            fixture
                .store
                .agent_message_snapshot(&fixture.agent.id)
                .await
                .unwrap()
                .unwrap(),
            baseline
        );
        let result = fixture
            .store
            .agent_message_snapshot_operation(&fixture.agent.id, &policy)
            .unwrap()
            .await;
        assert!(
            matches!(result, Err(AdmittedError::Unsupported(_))),
            "{result:?}"
        );
        assert_eq!(
            root.available_bytes(),
            root.capacity_bytes() - root.metadata_bytes()
        );
        let error = fixture
            .store
            .admitted_agent_message_snapshot(&fixture.agent.id, &policy)
            .await
            .unwrap_err();
        assert!(
            matches!(&*error, SandboxStoreError::Unavailable(message) if message.capacity() == 0)
        );
        drop(error);
        assert_eq!(
            root.available_bytes(),
            root.capacity_bytes() - root.metadata_bytes()
        );
        // Refusal supplies no snapshot for success/ACK and does not mutate or
        // quarantine the original Agent, checkpoint, receipt or ignored sidecar.
        let after = fixture
            .store
            .agents
            .find_one(doc! {"_id": fixture.agent.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after, before);
    }
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn snapshot_operation_keeps_data_and_read_pressure_outside_domain_results() {
    let fixture = Fixture::new("snapshot_refusal_class").await;
    let (root, policy) = policy();
    for class in [IngressClass::Data, IngressClass::Transformation] {
        let budget = policy.budget(class);
        let leave = if class == IngressClass::Transformation {
            4096
        } else {
            0
        };
        let pressure = policy
            .admit(
                class,
                AllocationPlan::checked(
                    "snapshot class pressure",
                    &[budget.available_bytes() - leave],
                )
                .unwrap(),
            )
            .unwrap();
        let held = root.available_bytes();
        let result = fixture
            .store
            .agent_message_snapshot_operation(&fixture.agent.id, &policy)
            .unwrap()
            .await;
        assert!(
            matches!(
                result,
                Err(AdmittedError::Refused(AdmissionRefusal::AtCapacity { .. }))
            ),
            "{result:?}"
        );
        assert_eq!(root.available_bytes(), held);
        drop(pressure);
        assert_eq!(
            root.available_bytes(),
            root.capacity_bytes() - root.metadata_bytes()
        );
    }
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
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn snapshot_operation_keeps_genuine_domain_corruption_owned() {
    let fixture = Fixture::new("snapshot_domain_error").await;
    let (root, policy) = policy();
    for invalid in [
        doc! {super::super::checkpoint::FIELD: {"version": 99}},
        doc! {"record": {"unexpected": "synthetic invalid Agent"}},
    ] {
        fixture
            .store
            .agents
            .update_one(
                doc! {"_id": fixture.agent.id.as_str()},
                doc! {"$set": invalid},
            )
            .await
            .unwrap();
        assert!(matches!(
            fixture
                .store
                .agent_message_snapshot(&fixture.agent.id)
                .await,
            Err(SandboxStoreError::Corrupt(_))
        ));
        let error = fixture
            .store
            .agent_message_snapshot_operation(&fixture.agent.id, &policy)
            .unwrap()
            .await
            .unwrap()
            .transpose()
            .unwrap_err();
        assert!(matches!(&*error, SandboxStoreError::Corrupt(message) if !message.is_empty()));
        assert!(error.belongs_to(&policy));
        let held = root.available_bytes();
        assert!(held < root.capacity_bytes() - root.metadata_bytes());
        let address = match &*error {
            SandboxStoreError::Corrupt(message) => message.as_ptr(),
            _ => unreachable!(),
        };
        let mapped = error.map(|error| match error {
            SandboxStoreError::Corrupt(message) => message,
            _ => unreachable!(),
        });
        assert_eq!(mapped.as_ptr(), address);
        assert_eq!(root.available_bytes(), held);
        drop(mapped);
        assert_eq!(
            root.available_bytes(),
            root.capacity_bytes() - root.metadata_bytes()
        );
    }
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn publication_read_keeps_absence_corruption_and_preflight_refusal_distinct() {
    use super::fixture::receipt_fixture;
    use crate::store::{
        agent_log_segments::publication::Publication, ingress_reads::error::ReadFailure,
    };
    let fixture = Fixture::new("receipt_errors").await;
    let store = &fixture.store;
    let rows = &store.segment_logs.publications;
    let receipt = receipt_fixture(&fixture.agent, 1);
    let original = mongodb::bson::serialize_to_document(&receipt).unwrap();
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = store.client.start_session().await.unwrap();
    for fault in [
        "missing",
        "width",
        "version",
        "nullable",
        "v1_witness",
        "identity",
        "digest",
        "unknown",
        "deep",
    ] {
        rows.delete_one(doc! {"_id":&receipt.id}).await.unwrap();
        let mut row = original.clone();
        match fault {
            "width" => {
                row.insert("head_revision", 3_i32);
            }
            "version" => {
                row.insert("format_version", 99_i64);
            }
            "nullable" => {
                row.remove("previous_seal");
            }
            "v1_witness" => {
                row.insert("agent_digest", Bson::Null);
            }
            "identity" => {
                row.insert("publication_key", "other-key");
            }
            "digest" => {
                row.insert(
                    "fingerprint",
                    mongodb::bson::Binary {
                        subtype: mongodb::bson::spec::BinarySubtype::Generic,
                        bytes: vec![1; 65536],
                    },
                );
            }
            "unknown" => {
                row.insert("unexpected", "x".repeat(65536));
            }
            "deep" => {
                row.insert("unexpected", ignored_sidecars()[0].clone());
            }
            _ => {}
        }
        if fault != "missing" {
            rows.insert_one(row.clone()).await.unwrap();
        }
        let result = Publication::read_at_admitted(
            store,
            &receipt.id,
            &mut session,
            false,
            &policy,
            super::gate_plan(),
        )
        .unwrap()
        .await;
        if fault == "deep" {
            assert!(matches!(result, Err(AdmittedError::Unsupported(_))));
        } else if fault == "missing" {
            assert!(result.unwrap().transpose().unwrap().is_none());
        } else {
            assert!(Publication::read(row.clone()).is_err());
            let error = result.unwrap().transpose().unwrap_err();
            assert!(
                matches!(&*error, ReadFailure::Domain(SandboxStoreError::Corrupt(_))),
                "{fault}"
            );
            assert!(error.belongs_to(&policy));
            drop(error);
        }
        assert_eq!(root.available_bytes(), idle);
        if fault != "missing" {
            assert_eq!(
                rows.find_one(doc! {"_id":&receipt.id}).await.unwrap(),
                Some(row)
            );
        }
    }
    drop(session);
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn publication_read_keeps_original_driver_error_and_transient_label() {
    use crate::store::{
        agent_log_segments::publication::Publication, ingress_reads::error::ReadFailure,
        sandbox_mongo::start_transaction,
    };
    let app = "cma-ingress-receipt-error";
    let mut options =
        mongodb::options::ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
    options.app_name = Some(app.into());
    let fixture = Fixture::with_client(
        mongodb::Client::with_options(options).unwrap(),
        "receipt_driver",
    )
    .await;
    let store = &fixture.store;
    let admin = store.client.database("admin");
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = store.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    admin.run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":1},"data":{"failCommands":["find"],"appName":app,"errorCode":112,"errorLabels":["TransientTransactionError"]}}).await.unwrap();
    let result = Publication::read_at_admitted(
        store,
        "synthetic-missing",
        &mut session,
        false,
        &policy,
        super::gate_plan(),
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

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn integrity_point_refuses_before_filter_or_query_and_cancels_unpolled() {
    use crate::store::mongo_read_telemetry::{isolated_test, snapshot};
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    let fixture = Fixture::new("point_refusal").await;
    let store = &fixture.store;
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = store.client.start_session().await.unwrap();
    let calls = AtomicUsize::new(0);
    let filter = || {
        calls.fetch_add(1, SeqCst);
        doc! {"_id":"point-missing"}
    };
    isolated_test(async {
        let full =
            AllocationPlan::checked("point pressure", &[policy.bounds().transformation_bytes()])
                .unwrap();
        let pressure = policy.admit(IngressClass::Transformation, full).unwrap();
        assert!(matches!(
            store.integrity_point_read_admitted(
                &mut session,
                false,
                false,
                &policy,
                super::gate_plan(),
                filter
            ),
            Err(AdmittedError::Refused(_))
        ));
        drop(pressure);
        drop(
            store
                .integrity_point_read_admitted(
                    &mut session,
                    false,
                    false,
                    &policy,
                    super::gate_plan(),
                    filter,
                )
                .unwrap(),
        );
        assert!(matches!(
            store
                .integrity_point_read_admitted(&mut session, false, false, &policy, full, filter)
                .unwrap()
                .await,
            Err(AdmittedError::Refused(_))
        ));
        assert_eq!(calls.load(SeqCst), 0);
        assert_eq!(snapshot(store.agent_log.name()), [0; 4]);
        assert_eq!(root.available_bytes(), idle);
        let missing = store
            .integrity_point_read_admitted(
                &mut session,
                false,
                false,
                &policy,
                super::gate_plan(),
                filter,
            )
            .unwrap()
            .await
            .unwrap()
            .transpose()
            .unwrap();
        assert!(missing.is_none());
        drop(missing);
        assert_eq!(calls.load(SeqCst), 1);
        assert_eq!(snapshot(store.agent_log.name()), [1, 0, 0, 0]);
    })
    .await;
    drop(session);
    assert_eq!(root.available_bytes(), idle);
    fixture.finish().await;
}
