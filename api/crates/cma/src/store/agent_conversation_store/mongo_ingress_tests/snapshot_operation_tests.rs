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
        root.capacity_bytes() - unpolled.requested_bytes()
    );
    drop(unpolled);
    assert_eq!(root.available_bytes(), root.capacity_bytes());

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
    assert!(retained < root.capacity_bytes());
    drop(result);
    assert!(root.available_bytes() > retained);
    assert_eq!(
        policy
            .budget(IngressClass::Transformation)
            .available_bytes(),
        policy.bounds().transformation_bytes()
    );
    assert!(root.available_bytes() < root.capacity_bytes());
    assert_eq!(*snapshot, legacy);
    drop(snapshot);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
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
    assert!(root.available_bytes() < root.capacity_bytes());
    drop(error);
    assert_eq!(root.available_bytes(), root.capacity_bytes());

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
    assert_eq!(root.available_bytes(), root.capacity_bytes());
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
    assert!(root.available_bytes() < root.capacity_bytes());
    drop(error);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
    drop(admin);
    fixture.finish().await;
}
