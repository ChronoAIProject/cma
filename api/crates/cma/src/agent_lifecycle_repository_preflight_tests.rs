use super::super::repository_preflight::RepositoryReadiness;
use super::*;
use layer1_public::rest::AgentLifecycleRepository;
use layer2_core::{
    profiles::authoring::AuthoringSourceError, sandbox::FixedSandboxCredentialPrerequisite,
};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Probe {
    result: Result<String, AuthoringSourceError>,
    reads: AtomicUsize,
}

#[async_trait]
impl RepositoryReadiness for Probe {
    async fn resolve_for_runtime(
        &self,
        owner: &SubjectId,
        repository: &AgentLifecycleRepository,
    ) -> Result<String, AuthoringSourceError> {
        assert_eq!(owner.as_str(), "owner");
        assert_eq!(repository.url, "https://github.com/example/project");
        assert_eq!(repository.reference.as_deref(), Some("proof/fixed"));
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.result.clone()
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn repository_preflight_refuses_before_allocation_and_pins_success_before_generic_gate() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    assert!(uri.starts_with("mongodb://127.0.0.1:") || uri.starts_with("mongodb://localhost:"));
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let database = client.database(&format!(
        "repository_preflight_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let operations = Arc::new(MongoAgentLifecycleStore::open(&database).await.unwrap());
    let profiles = Arc::new(
        crate::store::MongoProfileStore::open(&client, database.name())
            .await
            .unwrap(),
    );
    let store = Arc::new(layer2_core::sandbox::store::InMemorySandboxStore::default());
    let mut owner = fixture::context("owner");
    owner.caller.authorization =
        Authorization::new([Capability::UseOwnSandboxes, Capability::CreateSandbox]);
    let request = request(json!({
        "name": "Repository draft", "initial_message": "Inspect the synthetic source.",
        "repository": {"url":"https://github.com/example/project", "ref":"proof/fixed"},
    }));
    let commit = "a".repeat(40);
    for (key, result, expected) in [
        (
            "refused",
            Err(AuthoringSourceError::ReferenceNotFound),
            "failed",
        ),
        (
            "temporarily-unavailable",
            Err(AuthoringSourceError::Unavailable("fixture timeout".into())),
            "queued",
        ),
        ("allowed", Ok(commit.clone()), "waiting_for_connection"),
    ] {
        let probe = Arc::new(Probe {
            result,
            reads: AtomicUsize::new(0),
        });
        let service = fixture::facade_with_limit(
            store.clone(),
            profiles.clone(),
            operations.clone(),
            4,
            FixedSandboxCredentialPrerequisite::SetupRequired,
        )
        .with_task_experience(fixture::task_policy())
        .with_repository_preflight(probe.clone());
        let receipt = service
            .create(&owner.caller, Some(&owner), None, request.clone(), key)
            .await
            .unwrap();
        let record = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let record = operations
                    .read_for_owner(&owner.caller.subject, &receipt.id)
                    .await
                    .unwrap()
                    .unwrap();
                if record.status == expected && probe.reads.load(Ordering::SeqCst) == 1 {
                    break record;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(record.sandbox_id.is_none() && record.agent_id.is_none());
        assert_eq!(
            service.sandbox.capacity(&owner.caller).await.unwrap().live,
            0
        );
        match key {
            "refused" => assert!(record.error.unwrap().contains("repository_ref_not_found")),
            "allowed" => {
                assert_eq!(record.repository_commit.as_deref(), Some(commit.as_str()));
                assert_eq!(
                    record
                        .input
                        .request
                        .repository
                        .unwrap()
                        .reference
                        .as_deref(),
                    Some("proof/fixed")
                );
            }
            _ => assert!(record.error.is_none() && record.repository_commit.is_none()),
        }
    }
    database.drop().await.unwrap();
}
