use super::*;
use crate::agent_lifecycle::repository_preflight::RepositoryReadiness;
use layer1_public::rest::AgentLifecycleRepository;
use layer2_core::{
    profiles::authoring::AuthoringSourceError,
    sandbox::{SandboxCredentialPrerequisite, SandboxCredentialPrerequisiteError},
    ServiceId,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[path = "agent_lifecycle_managed_repository_tests.rs"]
mod managed;

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

#[derive(Clone)]
struct Connections(Arc<AtomicBool>);

#[async_trait]
impl SandboxCredentialPrerequisite for Connections {
    async fn check(&self, _: &SubjectId) -> Result<(), SandboxCredentialPrerequisiteError> {
        Ok(())
    }

    async fn check_services(
        &self,
        owner: &SubjectId,
        services: &[ServiceId],
    ) -> Result<(), SandboxCredentialPrerequisiteError> {
        assert_eq!(owner.as_str(), "owner");
        assert!(services.contains(&ServiceId::new("api-github")));
        if self.0.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(SandboxCredentialPrerequisiteError::ConnectionsRequired {
                services: vec![ServiceId::new("api-github")],
            })
        }
    }
}

async fn read_operation_when(
    operations: &MongoAgentLifecycleStore,
    id: &str,
    ready: impl Fn(&crate::store::LifecycleRecord) -> bool,
) -> crate::store::LifecycleRecord {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let record = operations
                .read_for_owner(&SubjectId::new("owner"), id)
                .await
                .unwrap()
                .unwrap();
            if ready(&record) {
                break record;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn repository_preflight_waits_for_repair_before_refusing_and_pins_connected_success() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
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
            Some("failed"),
        ),
        (
            "unavailable",
            Err(AuthoringSourceError::Unavailable("fixture timeout".into())),
            Some("queued"),
        ),
        ("allowed", Ok(commit.clone()), None),
    ] {
        let probe = Arc::new(Probe {
            result,
            reads: AtomicUsize::new(0),
        });
        let connection = Connections(Arc::new(AtomicBool::new(false)));
        let service = fixture::facade_with_limit(
            store.clone(),
            profiles.clone(),
            operations.clone(),
            4,
            connection.clone(),
        )
        .with_task_experience(fixture::task_policy())
        .with_repository_preflight(probe.clone());
        let receipt = service
            .create(&owner.caller, Some(&owner), None, request.clone(), key)
            .await
            .unwrap();

        let waiting = read_operation_when(&operations, &receipt.id, |record| {
            record.status == "waiting_for_connection"
        })
        .await;
        assert!(waiting.sandbox_id.is_none() && waiting.agent_id.is_none());
        assert!(waiting.repository_commit.is_none());
        let requirement = serde_json::to_value(waiting.connection.unwrap()).unwrap();
        assert_eq!(requirement["services"], json!(["api-github"]));
        assert_eq!(requirement["target"]["type"], "sandbox_profile");
        assert_eq!(probe.reads.load(Ordering::SeqCst), 0);
        assert_eq!(
            service.sandbox.capacity(&owner.caller).await.unwrap().live,
            0
        );

        connection.0.store(true, Ordering::SeqCst);
        let replay = service
            .create(&owner.caller, Some(&owner), None, request.clone(), key)
            .await
            .unwrap();
        assert_eq!(replay.id, receipt.id);
        let after = read_operation_when(&operations, &receipt.id, |record| {
            probe.reads.load(Ordering::SeqCst) == 1
                && expected.map_or_else(
                    || record.sandbox_id.is_some(),
                    |status| record.status == status && record.connection.is_none(),
                )
        })
        .await;
        match key {
            "refused" => {
                assert!(after.error.unwrap().contains("repository_ref_not_found"));
                assert!(after.sandbox_id.is_none());
            }
            "allowed" => {
                assert_eq!(after.repository_commit.as_deref(), Some(commit.as_str()));
                assert_eq!(
                    after.input.request.repository.unwrap().reference.as_deref(),
                    Some("proof/fixed")
                );
                assert!(after.sandbox_id.is_some());
            }
            _ => assert!(after.error.is_none() && after.repository_commit.is_none()),
        }
        let live = service.sandbox.capacity(&owner.caller).await.unwrap().live;
        assert_eq!(live, if expected.is_none() { 1 } else { 0 });
    }
    database.drop().await.unwrap();
}
