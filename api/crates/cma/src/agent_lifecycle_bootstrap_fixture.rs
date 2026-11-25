use super::*;
use layer2_core::profiles::{SandboxProfileSnapshot, SandboxProfileStore};
use layer2_core::sandbox::store::{
    InMemorySandboxStore, SandboxAdmission, SandboxAdmissionName, SandboxAdmissionStore,
    SandboxStore,
};
use layer2_core::sandbox::{Sandbox, SandboxOrigin, SandboxWorkState, SandboxWorkStep};
use tokio::sync::OwnedSemaphorePermit;

use super::super::fixture as existing;

pub(super) struct Fixture {
    pub db: mongodb::Database,
    pub client: mongodb::Client,
    pub store: Arc<InMemorySandboxStore>,
    pub operations: Arc<MongoAgentLifecycleStore>,
    pub service: Arc<InlineAgentLifecycle>,
    pub owner: CallerContext,
    pub record: crate::store::LifecycleRecord,
    pub sandbox: Sandbox,
    pub workspace_profile: WorkspaceProfileRef,
    _slots: OwnedSemaphorePermit,
}

impl Fixture {
    pub async fn new() -> Self {
        Self::with_request(request(
            json!({"name":"Early Agent", "profile":{"model":"model"}}),
        ))
        .await
    }

    pub async fn with_request(request: AgentLifecycleCreateRequest) -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
            .expect("disposable local Mongo URI is required");
        assert!(
            uri.starts_with("mongodb://127.0.0.1:") || uri.starts_with("mongodb://localhost:"),
            "bootstrap orchestration tests require local disposable Mongo"
        );
        let client = mongodb::Client::with_uri_str(uri).await.unwrap();
        let db = client.database(&format!(
            "cma_bootstrap_lifecycle_test_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let operations = Arc::new(MongoAgentLifecycleStore::open(&db).await.unwrap());
        let profiles = Arc::new(
            crate::store::MongoProfileStore::open(&client, db.name())
                .await
                .unwrap(),
        );
        let store = Arc::new(InMemorySandboxStore::default());
        let service = existing::facade(store.clone(), profiles.clone(), operations.clone());
        let service = if request.initial_message.is_some() {
            service.with_task_experience(existing::task_policy())
        } else {
            service
        };
        let slots = service
            .worker_slots
            .clone()
            .acquire_many_owned(crate::store::DISCOVERY_BATCH as u32)
            .await
            .unwrap();
        let mut owner = existing::context("owner");
        owner.caller.authorization =
            Authorization::new([Capability::UseOwnSandboxes, Capability::CreateSandbox]);
        let receipt = service
            .create(&owner.caller, Some(&owner), None, request, "early-request")
            .await
            .unwrap();
        let record = operations.claim(&receipt.id).await.unwrap().unwrap();
        assert!(record.input.sandbox_availability.always_keep_alive);
        let (_, workspace_profile, sandbox_profile) = profiles::create_profiles(
            service.profiles.as_ref(),
            &owner.caller,
            &guidance::provisioning_request(
                &record.input.request,
                record.input.managed_agent_instructions.as_deref(),
            ),
            None,
            &record.key,
            record.input.captured_capacity(service.bounds),
            record.input.captured_limits(),
            record.input.behavior,
            record.input.sandbox_availability,
            "",
        )
        .await
        .unwrap();
        // Reuse a valid model fixture without copying its ready Workspace into
        // this store. The actual lifecycle facade must reserve its own origin.
        let template = InMemorySandboxStore::default();
        let workspace = existing::seed(&template).await;
        let mut sandbox = template
            .get_sandbox(&workspace.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        sandbox.origin = SandboxOrigin::Lifecycle {
            operation_id: record.id.clone(),
        };
        sandbox.profile = SandboxProfileSnapshot::from(
            &profiles
                .sandbox_profile_revision(&sandbox_profile.id, sandbox_profile.revision)
                .await
                .unwrap()
                .unwrap(),
        );
        sandbox.status = SandboxStatus::Provisioning;
        sandbox.work = SandboxWorkState::new(
            SandboxWorkStep::ProvisionRuntime,
            layer2_core::now_rfc3339(),
        );
        sandbox.limits = record.input.captured_limits();
        store
            .admit_sandbox(
                SandboxAdmission::new(
                    sandbox.clone(),
                    SandboxAdmissionName::Exact(sandbox.name.clone()),
                    4,
                    layer2_core::now_rfc3339(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        operations
            .mark_sandbox(&record.lease().unwrap(), sandbox.id.as_str())
            .await
            .unwrap();
        let record = operations
            .read_for_owner(&owner.caller.subject, &record.id)
            .await
            .unwrap()
            .unwrap();
        Self {
            db,
            client,
            store,
            operations,
            service,
            owner,
            record,
            sandbox,
            workspace_profile,
            _slots: slots,
        }
    }

    pub async fn record(&self) -> crate::store::LifecycleRecord {
        self.operations
            .read_for_owner(&self.owner.caller.subject, &self.record.id)
            .await
            .unwrap()
            .unwrap()
    }

    pub async fn published(&self) -> (crate::store::LifecycleRecord, Agent, Workspace) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let record = self.record().await;
                if let Some(id) = &record.agent_id {
                    let agent = self
                        .store
                        .get_agent(&AgentId(id.clone()))
                        .await
                        .unwrap()
                        .unwrap();
                    let workspace = self
                        .store
                        .get_workspace(&agent.workspace_id)
                        .await
                        .unwrap()
                        .unwrap();
                    return (record, agent, workspace);
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("facade did not publish Agent before runtime readiness")
    }

    pub async fn restart(&self) -> Arc<InlineAgentLifecycle> {
        existing::facade(
            self.store.clone(),
            Arc::new(
                crate::store::MongoProfileStore::open(&self.client, self.db.name())
                    .await
                    .unwrap(),
            ),
            Arc::new(MongoAgentLifecycleStore::open(&self.db).await.unwrap()),
        )
    }
}

pub(super) fn work(record: &crate::store::LifecycleRecord) -> ProvisionInput {
    ProvisionInput {
        creation_fence: None,
        behavior: record.input.behavior,
        first_message: validation::admitted_first_message(&record.input),
        request: guidance::provisioning_request(
            &record.input.request,
            record.input.managed_agent_instructions.as_deref(),
        ),
        runtime_image: record.input.resolved_image.clone(),
        capacity: record.input.capacity.unwrap(),
        limits: record.input.captured_limits(),
        sandbox_availability: record.input.sandbox_availability,
        key: record.key.clone(),
        lease: record.lease().unwrap(),
        existing_sandbox_id: record.sandbox_id.clone(),
        existing_agent_id: record.agent_id.clone(),
        actor: record.input.actor.clone(),
        canary_profiles: None,
        repository_access_pin: None,
    }
}
