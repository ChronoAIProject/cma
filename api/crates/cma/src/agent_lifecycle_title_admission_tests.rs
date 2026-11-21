use super::*;
use layer2_core::sandbox::store::{SandboxStore, WorktreeStore};
use layer2_core::sandbox::task_experience::TaskExperiencePolicy;
use layer2_core::sandbox::{Sandbox, Workspace};
use mongodb::bson::{doc, Document};
use tokio::sync::OwnedSemaphorePermit;

struct AdmissionFixture {
    db: mongodb::Database,
    store: Arc<InMemorySandboxStore>,
    profiles: Arc<crate::store::MongoProfileStore>,
    operations: Arc<MongoAgentLifecycleStore>,
    owner: CallerContext,
    sandbox: Sandbox,
    workspace: Workspace,
}

impl AdmissionFixture {
    async fn new() -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
        assert!(uri.starts_with("mongodb://127.0.0.1:") || uri.starts_with("mongodb://localhost:"));
        let client = mongodb::Client::with_uri_str(uri).await.unwrap();
        let db = client.database(&format!(
            "cma_title_admission_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let operations = Arc::new(MongoAgentLifecycleStore::open(&db).await.unwrap());
        let profiles = Arc::new(
            crate::store::MongoProfileStore::open(&client, db.name())
                .await
                .unwrap(),
        );
        let store = Arc::new(InMemorySandboxStore::default());
        let workspace = fixture::seed(&store).await;
        let sandbox = store
            .get_sandbox(&workspace.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        let mut owner = fixture::context("owner");
        owner.caller.authorization =
            Authorization::new([Capability::UseOwnSandboxes, Capability::CreateSandbox]);
        Self {
            db,
            store,
            profiles,
            operations,
            owner,
            sandbox,
            workspace,
        }
    }

    async fn service(
        &self,
        policy: Option<Arc<TaskExperiencePolicy>>,
    ) -> (Arc<InlineAgentLifecycle>, OwnedSemaphorePermit) {
        let operations = Arc::new(MongoAgentLifecycleStore::open(&self.db).await.unwrap());
        let service = fixture::facade(self.store.clone(), self.profiles.clone(), operations);
        let service = if let Some(policy) = policy {
            service.with_task_experience(policy)
        } else {
            service
        };
        let slots = service
            .worker_slots
            .clone()
            .acquire_many_owned(crate::store::DISCOVERY_BATCH as u32)
            .await
            .unwrap();
        (service, slots)
    }

    async fn assert_writes(&self, admitted: u64) {
        assert_eq!(
            self.db
                .collection::<Document>("agent_lifecycle_operations")
                .count_documents(doc! {})
                .await
                .unwrap(),
            admitted
        );
        for collection in ["agent_profiles", "workspace_profiles", "sandbox_profiles"] {
            assert_eq!(
                self.db
                    .collection::<Document>(collection)
                    .count_documents(doc! {})
                    .await
                    .unwrap(),
                0
            );
        }
        assert_eq!(
            self.store
                .list_sandboxes(&self.owner.caller.subject, &Default::default())
                .await
                .unwrap()
                .items,
            vec![self.sandbox.clone()]
        );
        assert_eq!(
            self.store
                .list_workspaces(&self.sandbox.id, &Default::default())
                .await
                .unwrap()
                .items,
            vec![self.workspace.clone()]
        );
        assert!(self
            .store
            .list_worktrees(&self.workspace.id, &Default::default())
            .await
            .unwrap()
            .items
            .is_empty());
        assert!(self
            .store
            .list_agents(&self.owner.caller.subject, &Default::default())
            .await
            .unwrap()
            .items
            .is_empty());
    }

    fn request(
        &self,
        context: Option<&str>,
        mode: Option<&str>,
        message: bool,
    ) -> AgentLifecycleCreateRequest {
        let mut body = json!({"name":"New task"});
        if let Some(context) = context {
            body[context] = json!(if context == "workspace_id" {
                self.workspace.id.as_str()
            } else {
                self.sandbox.id.as_str()
            });
        }
        if let Some(mode) = mode {
            body["title_mode"] = json!(mode);
        }
        if message {
            body["initial_message"] = json!("Compare tools");
        }
        request(body)
    }
}

fn policies() -> [Option<Arc<TaskExperiencePolicy>>; 4] {
    [
        None,
        Some(Arc::new(TaskExperiencePolicy::default())),
        Some(Arc::new(serde_json::from_value(json!({
            "mode":"accounts", "accounts":["another-owner"], "capabilities":["task_home_admission"]
        })).unwrap())),
        Some(fixture::task_policy()),
    ]
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable local replica set"]
async fn explicit_title_modes_require_admission_policy_with_or_without_initial_message() {
    for context in [None, Some("workspace_id"), Some("sandbox_id")] {
        for (policy_index, policy) in policies().into_iter().enumerate() {
            let f = AdmissionFixture::new().await;
            let (service, _slots) = f.service(policy).await;
            let mut admitted = 0;
            for mode in [Some("automatic"), Some("named"), None] {
                for message in [false, true] {
                    let input = f.request(context, mode, message);
                    let key = format!("title-{mode:?}-message-{message}");
                    let result = service
                        .create(&f.owner.caller, Some(&f.owner), None, input.clone(), &key)
                        .await;
                    let allowed = policy_index == 3 || (mode.is_none() && !message);
                    if allowed {
                        let receipt = result.unwrap();
                        admitted += 1;
                        let record = f
                            .operations
                            .read_for_owner(&f.owner.caller.subject, &receipt.id)
                            .await
                            .unwrap()
                            .unwrap();
                        assert_eq!(
                            serde_json::to_value(&record.input.request).unwrap(),
                            serde_json::to_value(&input).unwrap()
                        );
                    } else {
                        assert!(matches!(result, Err(AgentLifecycleProvisioningError::Forbidden)),
                            "context={context:?} policy={policy_index} mode={mode:?} message={message}: {result:?}");
                        assert!(f
                            .operations
                            .replay(&f.owner.caller.subject, &key, &input)
                            .await
                            .unwrap()
                            .is_none());
                    }
                    f.assert_writes(admitted).await;
                }
            }
            f.db.drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable local replica set"]
async fn admitted_title_modes_replay_exactly_after_policy_is_withdrawn() {
    for context in [None, Some("workspace_id"), Some("sandbox_id")] {
        for mode in ["automatic", "named"] {
            for message in [false, true] {
                let f = AdmissionFixture::new().await;
                let (service, _slots) = f.service(Some(fixture::task_policy())).await;
                let input = f.request(context, Some(mode), message);
                let receipt = service
                    .create(
                        &f.owner.caller,
                        Some(&f.owner),
                        None,
                        input.clone(),
                        "admitted",
                    )
                    .await
                    .unwrap();
                for policy in policies().into_iter().take(3) {
                    let (reopened, _reopened_slots) = f.service(policy).await;
                    let replayed = reopened
                        .create(
                            &f.owner.caller,
                            Some(&f.owner),
                            None,
                            input.clone(),
                            "admitted",
                        )
                        .await
                        .unwrap();
                    assert_eq!(replayed.id, receipt.id);
                    let record = f
                        .operations
                        .read_for_owner(&f.owner.caller.subject, &receipt.id)
                        .await
                        .unwrap()
                        .unwrap();
                    assert_eq!(
                        serde_json::to_value(&record.input.request).unwrap(),
                        serde_json::to_value(&input).unwrap()
                    );
                    let mut changed = input.clone();
                    changed.title_mode = None;
                    assert!(matches!(
                        reopened
                            .create(&f.owner.caller, Some(&f.owner), None, changed, "admitted")
                            .await,
                        Err(AgentLifecycleProvisioningError::Conflict(_))
                    ));
                    assert!(matches!(
                        reopened
                            .create(
                                &f.owner.caller,
                                Some(&f.owner),
                                None,
                                input.clone(),
                                "new-admission"
                            )
                            .await,
                        Err(AgentLifecycleProvisioningError::Forbidden)
                    ));
                    f.assert_writes(1).await;
                }
                f.db.drop().await.unwrap();
            }
        }
    }
}
