use super::*;
use layer1_public::rest::AgentLifecycleImageReference;
use layer2_core::sandbox::store::InMemorySandboxStore;
use layer2_core::sandbox::{FixedSandboxCredentialPrerequisite, SandboxPolicy};

struct PublishedImage(String);

#[async_trait]
impl ImageResolver for PublishedImage {
    async fn resolve(
        &self,
        _: Option<&str>,
        _: &AgentLifecycleImageReference,
    ) -> Result<String, AgentLifecycleProvisioningError> {
        Ok(self.0.clone())
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mixed_replicas_preserve_an_admitted_agent_image_pin() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable Mongo URI is required");
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let db = client.database(&format!(
        "cma_image_retirement_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let operations = Arc::new(MongoAgentLifecycleStore::open(&db).await.unwrap());
    let profiles = Arc::new(
        crate::store::MongoProfileStore::open(&client, db.name())
            .await
            .unwrap(),
    );
    let sandboxes = Arc::new(InMemorySandboxStore::default());
    let reference = format!("registry.example/runtime@sha256:{}", "a".repeat(64));
    let build = |resolver: Arc<dyn ImageResolver>| {
        fixture::facade_with_resolver(
            sandboxes.clone(),
            profiles.clone(),
            operations.clone(),
            SandboxPolicy::default().max_sandboxes_per_owner,
            FixedSandboxCredentialPrerequisite::SetupRequired,
            resolver,
        )
    };
    let legacy = build(Arc::new(PublishedImage(reference.clone())));
    let retired = build(ImageManagerResolver::new("http://127.0.0.1:1".into(), true).unwrap());
    // Keep admission independent of the background worker while checking the
    // two replica facades against a shared, real operation store.
    let permits = crate::store::DISCOVERY_BATCH as u32;
    let _legacy_workers = legacy.worker_slots.acquire_many(permits).await.unwrap();
    let _retired_workers = retired.worker_slots.acquire_many(permits).await.unwrap();
    let mut context = fixture::context("owner");
    context.caller.authorization =
        Authorization::new([Capability::CreateSandbox, Capability::UseOwnSandboxes]);
    let caller = &context.caller;
    let request: AgentLifecycleCreateRequest = serde_json::from_value(json!({
        "name": "Image fixture",
        "image": {"id":"fixture-image","version":"fixture-version"}
    }))
    .unwrap();
    let first = legacy
        .create(caller, Some(&context), None, request.clone(), "existing")
        .await
        .unwrap();
    let replayed = retired
        .create(caller, Some(&context), None, request.clone(), "existing")
        .await
        .unwrap();
    assert_eq!(replayed.id, first.id);
    let persisted = operations
        .replay(&caller.subject, "existing", &request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(persisted.input.resolved_image, Some(reference));

    let fresh = retired.create(caller, Some(&context), None, request.clone(), "fresh");
    assert!(matches!(
        fresh.await,
        Err(AgentLifecycleProvisioningError::ImageSelectionUnsupported)
    ));
    assert!(operations
        .replay(&caller.subject, "fresh", &request)
        .await
        .unwrap()
        .is_none());
    db.drop().await.unwrap();
}
