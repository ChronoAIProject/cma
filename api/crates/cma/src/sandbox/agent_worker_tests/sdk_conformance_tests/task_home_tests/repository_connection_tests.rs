use super::*;
use layer2_core::sandbox::{FixedSandboxCredentialPrerequisite, SandboxCredentialPrerequisite};
use layer2_core::sandbox_usage::Allocation;
use layer2_core::sandbox_usage_service::SandboxUsageStore;
use layer2_core::trigger_domain::{
    BrokerHandoff, BrokerHandoffState, ServiceConnectionBroker, ServiceConnectionError,
    ServiceRequirement, ServiceRequirementState,
};
use std::sync::Mutex;

const SIBLING: &str = "wks_retained_synthetic_sibling";

#[derive(Clone, Copy)]
pub(super) struct RepositoryRefusal {
    pub code: &'static str,
    pub sibling: bool,
    pub before_marker: bool,
    pub delay_initial_renewal: bool,
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn repository_task_public_preparation_pins_checkout_before_exact_task_and_replays_identity() {
    for _ in 0..3 {
        super::environment::initial_task_environment(
            Some(json!({"url":"https://github.com/example/project", "ref":"proof/fixed"})),
            None,
            false,
        )
        .await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn repository_task_public_source_failure_preserves_identity_without_dispatching_task() {
    for failure in [
        "repository_connection_required",
        "repository_ref_not_found",
        "repository_unavailable",
        "repository_archive_unavailable",
        "repository_source_invalid",
    ] {
        super::environment::initial_task_environment(
            Some(json!({"url":"https://github.com/example/project", "ref":"proof/fixed"})),
            Some(RepositoryRefusal {
                code: failure,
                sibling: false,
                before_marker: false,
                delay_initial_renewal: false,
            }),
            false,
        )
        .await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn repository_task_public_failure_preserves_a_sandbox_with_another_workspace() {
    super::environment::initial_task_environment(
        Some(json!({"url":"https://github.com/example/project", "ref":"proof/fixed"})),
        Some(RepositoryRefusal {
            code: "repository_archive_unavailable",
            sibling: true,
            before_marker: false,
            delay_initial_renewal: false,
        }),
        false,
    )
    .await;
}

pub(super) async fn retain_another_workspace(
    harness: &Harness<MongoSandboxStore>,
    agent_id: &AgentId,
) {
    let agent = harness.stored(agent_id).await;
    let mut sibling = harness
        .store
        .get_workspace(&agent.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let now = layer2_core::now_rfc3339();
    sibling.id = WorkspaceId(SIBLING.into());
    sibling.origin = layer2_core::sandbox::WorkspaceOrigin::Manual;
    sibling.name = "retained-workspace".into();
    sibling.path = "/home/cma/retained-workspace".into();
    sibling.revision = 1;
    sibling.work =
        layer2_core::sandbox::WorkspaceWorkState::new(WorkspaceWorkStep::None, now.clone());
    sibling.status = WorkspaceStatus::Ready;
    sibling.ready_at = Some(now);
    sibling.resolved_commit = Some("b".repeat(40));
    harness.store.create_workspace(sibling).await.unwrap();
}

pub(super) async fn assert_other_workspace_preserved(
    harness: &Harness<MongoSandboxStore>,
    agent: &Agent,
) {
    let sibling = harness
        .store
        .get_workspace(&WorkspaceId(SIBLING.into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sibling.status, WorkspaceStatus::Ready);
    assert_eq!(sibling.sandbox_id, agent.sandbox_id);
    let parent = harness
        .store
        .get_sandbox(&agent.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(parent.status, SandboxStatus::Failed);
    assert!(harness
        .store
        .retirement_admitted_at(&agent.sandbox_id)
        .await
        .unwrap()
        .is_none());
}

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
            let mut last_operation = Value::Null;
            // A timed-out renewal leaves its 90-second claim intact. Allow the
            // real discovery loop to reclaim it before asserting publication.
            let waited = tokio::time::timeout(Duration::from_secs(120), async {
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
                    last_operation = operation;
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await;
            assert!(
                waited.is_ok(),
                "connection state {state:?} was not published: {last_operation}; observed {:?}",
                self.observed.lock().unwrap()
            );
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

pub(super) async fn complete_failed_environment_retirement(
    harness: &Harness<MongoSandboxStore>,
    sandbox_id: &SandboxId,
    agent: Option<&Agent>,
    worker: &crate::sandbox::worker::SandboxWorker,
    retry: &super::store_retry::StoreRetry,
) {
    let retiring = harness
        .store
        .get_sandbox(sandbox_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        retiring.status,
        layer2_core::sandbox::SandboxStatus::Retiring
    );
    let runtime = retiring
        .runtime
        .as_ref()
        .map(|runtime| RuntimeHandle(runtime.runtime_id.clone()));
    let admitted_at = harness
        .store
        .retirement_admitted_at(sandbox_id)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let Some(agent) = agent {
                retry.agent(harness, &agent.id).await;
            }
            retry.sandbox(harness, worker, sandbox_id).await;
            let sandbox = harness
                .store
                .get_sandbox(sandbox_id)
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
    let usage = harness.store.usage(sandbox_id).await.unwrap().unwrap();
    assert_eq!(usage.current_allocation(), Allocation::default());
    assert_eq!(
        usage.totals_at(usage.last_observed_at + 60).unwrap(),
        usage.totals
    );
    assert_eq!(
        harness
            .store
            .retirement_admitted_at(sandbox_id)
            .await
            .unwrap()
            .as_deref(),
        Some(admitted_at.as_str())
    );
    if let Some(agent) = agent {
        let retained = harness.stored(&agent.id).await;
        let first = retained.conversation.first_message.as_ref().unwrap();
        assert_eq!(
            first.text,
            agent.conversation.first_message.as_ref().unwrap().text
        );
        assert!(first.dispatch_fenced_at.is_none() && first.turn_sequence.is_none());
        assert!(first.write_confirmed_at.is_none() && first.execution_observed_at.is_none());
    }
}
