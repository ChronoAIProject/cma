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
