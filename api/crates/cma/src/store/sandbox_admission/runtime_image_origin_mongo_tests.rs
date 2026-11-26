use super::*;
use crate::store::{CreateResult, LifecycleBehavior, LifecycleInput, MongoAgentLifecycleStore};
use layer2_core::profiles::private_sandbox_profile_id;
use layer2_core::sandbox::SandboxRuntimeImageOrigin;

#[tokio::test]
#[ignore = "requires disposable CHRONO_TEST_MONGODB_URI"]
async fn desktop_image_origin_survives_operation_reclaim_and_release_change() {
    let (client, name) = database("desktop_origin").await;
    let db = client.database(&name);
    let operations = MongoAgentLifecycleStore::open(&db).await.unwrap();
    let mut store = MongoSandboxStore::open(&client, &name).await.unwrap();
    store.runtime_image_origin_enabled = true;
    let original = format!("registry.example/runtime@sha256:{}", "a".repeat(64));
    let successor = format!("registry.example/runtime@sha256:{}", "b".repeat(64));
    store = store.with_managed_runtime_version(original.clone());

    for (key, image, expected) in [
        (
            "implicit",
            serde_json::Value::Null,
            SandboxRuntimeImageOrigin::ManagedDesktop,
        ),
        (
            "explicit",
            serde_json::json!({"id":"fixture-image"}),
            SandboxRuntimeImageOrigin::UserSelected,
        ),
    ] {
        let mut candidate = sandbox(&format!("sbx_{key}"), "unused");
        let request = serde_json::from_value(serde_json::json!({
            "name": "Fixture Desktop", "desktop": true,
            "initial_message": "fixture task", "image": image
        }))
        .unwrap();
        let input = LifecycleInput {
            behavior: LifecycleBehavior::PromptRepository,
            request,
            resolved_image: Some(original.clone()),
            capacity: Some(layer3_adapter::SandboxCapacity {
                cpu_millis: 1000,
                memory_mib: 2048,
            }),
            managed_agent_instructions: None,
            setup_message: None,
            sandbox_availability: Default::default(),
            capabilities: vec![],
            actor: None,
        };
        let CreateResult::Created(id) = operations
            .create(&candidate.owner, key, input)
            .await
            .unwrap()
        else {
            panic!("expected a new operation");
        };
        let claimed = operations.claim(&id).await.unwrap().unwrap();
        let first = claimed.lease().unwrap();
        let profiles_key = format!("agent-manager:{key}:sandbox");
        candidate.origin = SandboxOrigin::Lifecycle {
            operation_id: id.clone(),
        };
        candidate.profile.id = private_sandbox_profile_id(&candidate.owner, &profiles_key).unwrap();
        candidate.profile.content.runtime_image = Some(original.clone());
        candidate.profile.digest = content_digest(&candidate.profile.content).unwrap();
        let admitted_profile = candidate.profile.clone();

        db.collection::<mongodb::bson::Document>(
            crate::store::agent_lifecycle_operations::COLLECTION,
        )
        .update_one(
            doc! {"_id": &id},
            doc! {"$set": {"lease_expires_at": mongodb::bson::DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
        store = store.with_managed_runtime_version(successor.clone());
        let reclaimed = operations.claim(&id).await.unwrap().unwrap();
        let second = reclaimed.lease().unwrap();
        assert_ne!(first.token, second.token);
        assert_eq!(
            reclaimed.input.resolved_image.as_deref(),
            Some(original.as_str())
        );
        candidate.name = format!("{key} Desktop");
        let requested_name = SandboxAdmissionName::Allocate(candidate.name.clone());
        let replay = SandboxAdmission::new(candidate, requested_name, 4, NOW.into())
            .unwrap()
            .with_lifecycle_lease(&second.token);
        let SandboxAdmissionResult::Created(created) =
            store.admit_sandbox(replay.clone()).await.unwrap()
        else {
            panic!("expected the reclaimed operation to admit the Sandbox");
        };
        assert_eq!(created.profile, admitted_profile);
        let capacity = store.owner_capacity(&created.owner).await.unwrap();
        let occupied = capacity
            .counts
            .iter()
            .find(|(status, _)| *status == SandboxStatus::Provisioning);
        assert_eq!(
            occupied.map(|(_, count)| *count),
            Some(if key == "implicit" { 1 } else { 2 })
        );
        assert_eq!(created.lifecycle.runtime_image_origin, Some(expected));
        assert_eq!(
            store.admit_sandbox(replay).await.unwrap(),
            SandboxAdmissionResult::Existing(created.clone()),
        );
        assert_eq!(store.get_sandbox(&created.id).await.unwrap(), Some(created));
    }
    crate::store::work_schema::floor::verify(&db).await.unwrap();
    db.drop().await.unwrap();
}
