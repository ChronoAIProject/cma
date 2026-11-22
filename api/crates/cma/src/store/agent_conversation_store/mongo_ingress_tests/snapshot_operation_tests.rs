use super::*;
use layer2_core::sandbox::ingress_ownership::{AdmissionRefusal, AdmittedError};
use layer2_core::sandbox::AgentConversationError;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn synchronous_snapshot_admits_before_poll_and_transfers_independent_retention() {
    let fixture = fixture::Fixture::new("operation_success").await;
    let (root, policy) = policy();
    let occupied = policy
        .admit(
            IngressClass::Transformation,
            AllocationPlan::checked(
                "occupied snapshot operation",
                &[policy.bounds().transformation_bytes()],
            )
            .unwrap(),
        )
        .unwrap();
    let held = root.available_bytes();
    assert!(matches!(
        fixture
            .store
            .agent_message_snapshot_operation(&fixture.agent.id, &policy),
        Err(AdmittedError::Refused(AdmissionRefusal::AtCapacity { .. }))
    ));
    assert_eq!(root.available_bytes(), held);
    drop(occupied);

    let unpolled = fixture
        .store
        .agent_message_snapshot_operation(&fixture.agent.id, &policy)
        .unwrap();
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes() - unpolled.requested_bytes()
    );
    drop(unpolled);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );

    let legacy = fixture
        .store
        .agent_message_snapshot(&fixture.agent.id)
        .await
        .unwrap()
        .unwrap();
    let mut operation = fixture
        .store
        .agent_message_snapshot_operation(&fixture.agent.id, &policy)
        .unwrap();
    let result = (&mut operation).await.unwrap();
    assert!(result.belongs_to(&policy));
    let snapshot = result.as_ref().unwrap().as_ref().unwrap().clone();
    assert_eq!(*snapshot, legacy);
    drop(operation);
    let retained = root.available_bytes();
    assert!(retained < root.capacity_bytes() - root.metadata_bytes());
    drop(result);
    assert!(root.available_bytes() > retained);
    assert_eq!(
        policy
            .budget(IngressClass::Transformation)
            .available_bytes(),
        policy.bounds().transformation_bytes()
    );
    assert!(root.available_bytes() < root.capacity_bytes() - root.metadata_bytes());
    assert_eq!(*snapshot, legacy);
    drop(snapshot);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn synchronous_snapshot_preserves_corruption_and_missing_row_outcomes() {
    let fixture = fixture::Fixture::new("operation_corruption").await;
    let (root, policy) = policy();
    fixture
        .store
        .agents
        .update_one(
            doc! {"_id": fixture.agent.id.as_str()},
            doc! {"$set": {"record": {"unexpected": "invalid snapshot".repeat(1024)}}},
        )
        .await
        .unwrap();
    let result = fixture
        .store
        .agent_message_snapshot_operation(&fixture.agent.id, &policy)
        .unwrap()
        .await
        .unwrap();
    let error = result.transpose().unwrap_err();
    let SandboxStoreError::Corrupt(detail) = &*error else {
        panic!("expected snapshot corruption");
    };
    assert!(!detail.is_empty());
    let address = detail.as_ptr();
    let error = error.map(|error| match error {
        SandboxStoreError::Corrupt(detail) => AgentConversationError::Corrupt(detail),
        _ => panic!("snapshot error category changed"),
    });
    assert!(
        matches!(&*error, AgentConversationError::Corrupt(detail) if detail.as_ptr() == address)
    );
    assert!(error.belongs_to(&policy));
    assert!(root.available_bytes() < root.capacity_bytes() - root.metadata_bytes());
    drop(error);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );

    fixture
        .store
        .agents
        .delete_one(doc! {"_id": fixture.agent.id.as_str()})
        .await
        .unwrap();
    let missing = fixture
        .store
        .agent_message_snapshot_operation(&fixture.agent.id, &policy)
        .unwrap()
        .await
        .unwrap();
    assert!(missing.as_ref().unwrap().is_none());
    drop(missing);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn synchronous_snapshot_keeps_the_original_unavailable_error_owned() {
    let mut options =
        mongodb::options::ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
    let app_name = "cma-ingress-snapshot-operation-failure";
    options.app_name = Some(app_name.into());
    let fixture = fixture::Fixture::with_client(
        mongodb::Client::with_options(options).unwrap(),
        "operation_failure",
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
        .agent_message_snapshot_operation(&fixture.agent.id, &policy)
        .unwrap()
        .await
        .unwrap();
    admin
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();
    let error = result.transpose().unwrap_err();
    let SandboxStoreError::Unavailable(message) = &*error else {
        panic!("expected snapshot unavailability");
    };
    assert_eq!(message, "conversation snapshot read failed");
    let address = message.as_ptr();
    let error = error.map(|error| match error {
        SandboxStoreError::Unavailable(message) => AgentConversationError::Unavailable(message),
        _ => panic!("snapshot error category changed"),
    });
    assert!(
        matches!(&*error, AgentConversationError::Unavailable(message) if message.as_ptr() == address)
    );
    assert!(error.belongs_to(&policy));
    assert!(root.available_bytes() < root.capacity_bytes() - root.metadata_bytes());
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
async fn projection_operation_refuses_before_queries_and_retains_original_driver_error() {
    use crate::store::{
        agent_log_segments::{projection::read_admitted as read, PROJECTIONS},
        ingress_reads::error::ReadFailure,
    };
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let mut options =
        mongodb::options::ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
    let app_name = "cma-ingress-projection-custody";
    options.app_name = Some(app_name.into());
    options.command_event_handler = Some(mongodb::event::EventHandler::callback(move |event| {
        if let mongodb::event::command::CommandEvent::Started(event) = event {
            if event.command_name == "find"
                && event.command.get_str("find").ok() == Some(PROJECTIONS)
            {
                observed.fetch_add(1, Ordering::SeqCst);
            }
        }
    }));
    let fixture = fixture::Fixture::with_client(
        mongodb::Client::with_options(options).unwrap(),
        "projection_custody",
    )
    .await;
    let store = &fixture.store;
    let (agent, head, row) = fixture::projection_fixture(&fixture.agent, 1);
    store
        .segment_logs
        .projections
        .insert_one(mongodb::bson::serialize_to_document(&row).unwrap())
        .await
        .unwrap();
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = store.client.start_session().await.unwrap();
    let full = AllocationPlan::checked(
        "full projection work",
        &[policy.bounds().transformation_bytes()],
    )
    .unwrap();
    let occupied = policy.admit(IngressClass::Transformation, full).unwrap();
    assert!(matches!(
        read(store, &agent, &head, &mut session, &policy, gate_plan()),
        Err(AdmittedError::Refused(_))
    ));
    drop(occupied);
    drop(read(store, &agent, &head, &mut session, &policy, gate_plan()).unwrap());
    assert!(matches!(
        read(store, &agent, &head, &mut session, &policy, full)
            .unwrap()
            .await,
        Err(AdmittedError::Refused(_))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(root.available_bytes(), idle);
    let occupied = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked("full projection Data", &[policy.bounds().data_bytes()])
                .unwrap(),
        )
        .unwrap();
    assert!(matches!(
        read(store, &agent, &head, &mut session, &policy, gate_plan())
            .unwrap()
            .await,
        Err(AdmittedError::Refused(_))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    drop(occupied);
    assert_eq!(root.available_bytes(), idle);
    let mut large_agent = agent.clone();
    large_agent.id.0 = "a".repeat(4 * 1024 * 1024);
    let occupied = policy
        .admit(
            IngressClass::Transformation,
            AllocationPlan::checked("validation pressure", &[64 * 1024 * 1024]).unwrap(),
        )
        .unwrap();
    let held = root.available_bytes();
    assert!(matches!(
        read(
            store,
            &large_agent,
            &head,
            &mut session,
            &policy,
            AllocationPlan::checked("small projection response", &[1024 * 1024]).unwrap()
        )
        .unwrap()
        .await,
        Err(AdmittedError::Refused(_))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(root.available_bytes(), held);
    drop((occupied, large_agent));
    assert_eq!(root.available_bytes(), idle);
    let admin = store.client.database("admin");
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
    let result = read(store, &agent, &head, &mut session, &policy, gate_plan())
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
    let error = error.map(|error| match error {
        ReadFailure::Driver(driver) => driver,
        _ => unreachable!(),
    });
    assert_eq!(std::ptr::from_ref(error.kind.as_ref()), pointer);
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
async fn publication_read_pins_snapshot_and_admits_each_actual_replacement() {
    use super::fixture::{receipt_fixture, Fixture};
    use crate::store::agent_log_segments::publication::Publication;
    use mongodb::bson::serialize_to_document as document;
    let fixture = Fixture::new("receipt_snapshot").await;
    let store = &fixture.store;
    let rows = &store.segment_logs.publications;
    let small = receipt_fixture(&fixture.agent, 1);
    let grown = receipt_fixture(&fixture.agent, 2);
    rows.insert_one(document(&small).unwrap()).await.unwrap();
    let (root, policy) = super::policy();
    let idle = root.available_bytes();
    let read = Publication::read_at_admitted;
    let mut snapshot = store.client.start_session().snapshot(true).await.unwrap();
    let first = read(
        store,
        &small.id,
        &mut snapshot,
        true,
        &policy,
        super::gate_plan(),
    )
    .unwrap()
    .await
    .unwrap()
    .transpose()
    .unwrap();
    assert_eq!(**first.as_ref().unwrap(), small);
    let timestamp = snapshot.snapshot_time();
    assert!(timestamp.is_some());
    let charge = policy.bounds().data_bytes() - policy.budget(IngressClass::Data).available_bytes();
    drop(first);
    let pressure = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked(
                "receipt small allowance",
                &[policy.bounds().data_bytes() - charge],
            )
            .unwrap(),
        )
        .unwrap();
    let held = root.available_bytes();
    rows.replace_one(doc! {"_id":&small.id}, document(&grown).unwrap())
        .await
        .unwrap();
    let pinned = read(
        store,
        &small.id,
        &mut snapshot,
        true,
        &policy,
        super::gate_plan(),
    )
    .unwrap()
    .await
    .unwrap()
    .transpose()
    .unwrap();
    assert_eq!(**pinned.as_ref().unwrap(), small);
    assert_eq!(snapshot.snapshot_time(), timestamp);
    drop(pinned);
    let mut later = store.client.start_session().await.unwrap();
    assert!(matches!(
        read(
            store,
            &small.id,
            &mut later,
            false,
            &policy,
            super::gate_plan()
        )
        .unwrap()
        .await,
        Err(AdmittedError::Refused(_))
    ));
    assert_eq!(root.available_bytes(), held);
    drop(pressure);
    let changed = read(
        store,
        &small.id,
        &mut later,
        false,
        &policy,
        super::gate_plan(),
    )
    .unwrap()
    .await
    .unwrap()
    .transpose()
    .unwrap();
    assert_eq!(**changed.as_ref().unwrap(), grown);
    drop((changed, snapshot, later));
    assert_eq!(root.available_bytes(), idle);
    fixture.finish().await;
}
