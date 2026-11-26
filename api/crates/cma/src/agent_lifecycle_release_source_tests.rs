use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn desktop_creation_persists_the_runtime_it_admitted() {
    use crate::warm_pools::tests::runtime_pointer::released_source::{Call, ControlledSource};
    use layer3_adapter::ReleasedRuntime;
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let name = format!("lifecycle_release_{}", uuid::Uuid::new_v4().simple());
    let db = client.database(&name);
    let operations = Arc::new(MongoAgentLifecycleStore::open(&db).await.unwrap());
    let profiles = Arc::new(
        crate::store::MongoProfileStore::open(&client, &name)
            .await
            .unwrap(),
    );
    let release = |revision: u64, machine: char| ReleasedRuntime {
        lane_revisions: None,
        revision,
        agent: format!("registry.example/agent@sha256:{}", "a".repeat(64)),
        machine: Some(format!(
            "registry.example/machine@sha256:{}",
            machine.to_string().repeat(64)
        )),
    };
    let source = Arc::new(ControlledSource::new(release(1, '1')));
    source.advance_after_snapshot(release(2, '2'));
    let service = fixture::facade(
        Arc::new(InMemorySandboxStore::default()),
        profiles,
        operations.clone(),
    )
    .with_released_runtime(source.clone())
    .with_task_experience(Arc::new(
        serde_json::from_value(json!({
            "mode": "accounts",
            "accounts": ["owner"],
            "capabilities": ["desktop_enable"]
        }))
        .unwrap(),
    ));
    let _slots = service
        .worker_slots
        .clone()
        .acquire_many_owned(crate::store::DISCOVERY_BATCH as u32)
        .await
        .unwrap();
    let mut owner = fixture::context("owner");
    owner.caller.authorization =
        Authorization::new([Capability::UseOwnSandboxes, Capability::CreateSandbox]);
    for (revision, digest, key) in [(1, '1', "machine-before"), (2, '2', "machine-after")] {
        let expected = release(revision, digest);
        source.take_calls();
        let receipt = service
            .create(
                &owner.caller,
                Some(&owner),
                None,
                request(json!({"name": "Machine admission", "machine_included": true})),
                key,
            )
            .await
            .unwrap();
        let record = operations
            .read_for_owner(&owner.caller.subject, &receipt.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.input.resolved_image, expected.machine);
        assert_eq!(source.take_calls(), [Call::Snapshot(revision)]);
    }
    db.drop().await.unwrap();
}
