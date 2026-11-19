use super::*;
use layer2_core::sandbox::{FixedSandboxCredentialPrerequisite, SandboxCredentialPrerequisite};
use layer2_core::trigger_domain::{
    BrokerHandoff, BrokerHandoffState, ServiceConnectionBroker, ServiceConnectionError,
    ServiceRequirement, ServiceRequirementState,
};
use std::sync::Mutex;

pub(super) struct RepositoryConnection {
    state: Mutex<ServiceRequirementState>,
    observed: Mutex<Vec<ServiceRequirementState>>,
}
impl RepositoryConnection {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(ServiceRequirementState::Missing),
            observed: Mutex::new(Vec::new()),
        })
    }
    pub(super) fn prerequisite(self: &Arc<Self>) -> Arc<dyn SandboxCredentialPrerequisite> {
        crate::connections::ConnectionPrerequisite::new(
            Arc::new(FixedSandboxCredentialPrerequisite::Ready),
            self.clone(),
            Arc::new(SandboxPolicy::default()),
        )
    }
    pub(super) async fn allow_after_blocked_states(
        &self,
        service: &Router,
        harness: &Harness<MongoSandboxStore>,
        input: &Value,
        receipt: &Value,
        operation_path: &str,
    ) {
        for state in [
            ServiceRequirementState::Missing,
            ServiceRequirementState::Inactive,
            ServiceRequirementState::Unknown,
        ] {
            *self.state.lock().unwrap() = state;
            let (_, replay) = create(service, input, "default-environment-task").await;
            assert_eq!(replay["id"], receipt["id"]);
            tokio::time::timeout(Duration::from_secs(15), async {
                loop {
                    let operation: Value = serde_json::from_str(
                        &cross_interface_tests::read(service, operation_path).await,
                    )
                    .unwrap();
                    if operation["status"] == "waiting_for_connection"
                        && self.observed.lock().unwrap().contains(&state)
                    {
                        assert!(operation["agent_id"].is_null());
                        assert_eq!(operation["connection"]["services"], json!(["api-github"]));
                        break;
                    }
                    assert_ne!(operation["status"], "failed", "{operation}");
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .unwrap();
            let sandboxes = harness
                .store
                .list_sandboxes(&SubjectId::new("owner"), &PageRequest::default())
                .await
                .unwrap();
            assert_eq!(
                sandboxes.items.len(),
                1,
                "no Sandbox allocated for {state:?}"
            );
            assert_eq!(sandboxes.items[0].id, harness.workspace.sandbox_id);
        }
        *self.state.lock().unwrap() = ServiceRequirementState::Satisfied;
        let (_, authorized) = create(service, input, "default-environment-task").await;
        assert_eq!(authorized["id"], receipt["id"]);
    }
}

#[async_trait::async_trait]
impl ServiceConnectionBroker for RepositoryConnection {
    async fn requirements(
        &self,
        owner: &SubjectId,
        services: &[ServiceId],
    ) -> Result<Vec<ServiceRequirement>, ServiceConnectionError> {
        assert_eq!(owner.as_str(), "owner");
        Ok(services
            .iter()
            .map(|service| ServiceRequirement {
                service: service.clone(),
                state: if service.as_str() == "api-github" {
                    let state = *self.state.lock().unwrap();
                    self.observed.lock().unwrap().push(state);
                    state
                } else {
                    ServiceRequirementState::Satisfied
                },
            })
            .collect())
    }
    async fn create_handoff(
        &self,
        _: &SubjectId,
        _: &ServiceId,
        _: &str,
    ) -> Result<BrokerHandoff, ServiceConnectionError> {
        panic!("repository preflight must not create a connection handoff")
    }
    async fn handoff_state(
        &self,
        _: &SubjectId,
        _: &str,
        _: &ServiceId,
    ) -> Result<BrokerHandoffState, ServiceConnectionError> {
        panic!("repository preflight only reads current requirements")
    }
}

pub(super) async fn retire_failed_environment(
    harness: &Harness<MongoSandboxStore>,
    agent: &Agent,
    worker: &crate::sandbox::worker::SandboxWorker,
    retry: &super::store_retry::StoreRetry,
) {
    let service = SandboxService::new(
        harness.store.clone(),
        Arc::new(InMemoryProfileStore::default()),
        SandboxPolicy::default(),
        Arc::new(FixedSandboxCredentialPrerequisite::Ready),
    );
    let owner = layer2_core::Caller::new(
        agent.owner.clone(),
        Authorization::new([Capability::DeleteOwnSandbox]),
    );
    let retiring = service.delete(&owner, &agent.sandbox_id).await.unwrap();
    assert_eq!(
        retiring.status,
        layer2_core::sandbox::SandboxStatus::Retiring
    );
    let runtime = retiring
        .runtime
        .as_ref()
        .map(|runtime| RuntimeHandle(runtime.runtime_id.clone()));
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            retry.should_retry(harness.worker.run_one(&agent.id, SystemTime::now()).await);
            retry.should_retry(worker.run_one(&agent.sandbox_id, SystemTime::now()).await);
            let sandbox = harness
                .store
                .get_sandbox(&agent.sandbox_id)
                .await
                .unwrap()
                .unwrap();
            if sandbox.status == layer2_core::sandbox::SandboxStatus::Retired {
                assert!(sandbox.runtime.is_none());
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    if let Some(runtime) = runtime {
        assert!(harness.backend.snapshot(&runtime).await.is_err());
    }
    let retained = harness.stored(&agent.id).await;
    let first = retained.conversation.first_message.as_ref().unwrap();
    assert_eq!(
        first.text,
        agent.conversation.first_message.as_ref().unwrap().text
    );
    assert!(first.dispatch_fenced_at.is_none() && first.turn_sequence.is_none());
    assert!(first.write_confirmed_at.is_none() && first.execution_observed_at.is_none());
}
