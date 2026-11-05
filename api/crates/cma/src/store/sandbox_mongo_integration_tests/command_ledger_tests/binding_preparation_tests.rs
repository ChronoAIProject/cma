use super::*;
use layer3_adapter::{CheckpointReceipt, WriterState};
use std::io::Write;

struct Prerequisite {
    result: Mutex<Result<(), SandboxCredentialPrerequisiteError>>,
    calls: AtomicUsize,
}
#[async_trait::async_trait]
impl SandboxCredentialPrerequisite for Prerequisite {
    async fn check(&self, _: &SubjectId) -> Result<(), SandboxCredentialPrerequisiteError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.result.lock().unwrap().clone()
    }
}
#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);
impl Write for Logs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Logs {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

struct Fixture {
    store: Arc<MongoSandboxStore>,
    agent: Agent,
    active: Sandbox,
    command: AdmittedAgentCommand,
    execution: Arc<crate::commands::ResourceCommandExecution>,
    sends: Arc<AtomicUsize>,
    prerequisite: Arc<Prerequisite>,
}
impl Fixture {
    async fn new(
        client: &Client,
        name: &str,
        supplied: bool,
        result: Result<(), SandboxCredentialPrerequisiteError>,
    ) -> Self {
        let prerequisite = Arc::new(Prerequisite {
            result: Mutex::new(result),
            calls: AtomicUsize::new(0),
        });
        // Credential source is immutable: seed supplied custody at creation.
        let mut initial = active_sandbox("sbx_commands");
        if supplied {
            initial.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
            initial.credential.source = SandboxCredentialSource::Binding {
                binding_id: "command-binding".into(),
                tool_endpoint: None,
            };
            let authority = initial.credential.current.as_mut().unwrap();
            authority.provider_key_id = format!("provided-{}", "a".repeat(64));
            authority.provider_state_version = 1;
            authority.expires_at.clear();
        }
        let (store, mut agent, mut parent, execution, _, sends) =
            composed_with_credentials(client, name, prerequisite.clone(), initial).await;
        let (binding, _, _) = binding_for(&store, &agent).await;
        let active = parent.clone();
        let revision = parent.revision;
        parent.status = SandboxStatus::Paused;
        parent.runtime = None;
        parent.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
        parent.lifecycle.checkpoints.latest = Some(
            SandboxCheckpointRef::seal(
                &CheckpointReceipt {
                    epoch: 1,
                    object_reference: "checkpoints/preparation".into(),
                    sha256: "a".repeat(64),
                    writers: WriterState::Resumed,
                },
                &[],
                NOW,
            )
            .unwrap(),
        );
        parent.revision += 1;
        store.save_sandbox(revision, parent.clone()).await.unwrap();
        let revision = agent.revision;
        agent.status = AgentStatus::Paused;
        agent.revision += 1;
        store.save_agent(revision, agent.clone()).await.unwrap();
        let request = reservation(&store, &agent, &parent, &binding).await;
        let command = record(store.reserve_command(request).await.unwrap());
        let authority = auth(store.clone());
        let access = Arc::new(
            ResourceAccessService::new(store.clone(), authority.clone())
                .with_bindings(BindingAccessService::new(store.clone())),
        );
        let execution = Arc::new(crate::commands::ResourceCommandExecution {
            source_guard: None,
            authority,
            access,
            store: store.clone(),
            operations: execution.operations.clone(),
            context: execution.context.clone(),
        });
        Self {
            store,
            agent,
            active,
            command,
            execution,
            sends,
            prerequisite,
        }
    }
    fn worker(&self) -> Arc<AgentCommandWorker> {
        AgentCommandWorker::new(
            self.store.clone(),
            self.execution.clone(),
            "binding-preparation".into(),
        )
        .unwrap()
    }
    async fn command(&self) -> AdmittedAgentCommand {
        self.store
            .get_command(&self.command.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap()
    }
    async fn parent(&self) -> Sandbox {
        self.store
            .get_sandbox(&self.agent.sandbox_id)
            .await
            .unwrap()
            .unwrap()
    }
    async fn finish_revival(&self) {
        let revived = self.parent().await;
        // Model the lifecycle worker completing the retained conversation reopen.
        // The command worker must then acquire its ordinary dispatch fence.
        let mut active = self.active.clone();
        active.revision = revived.revision + 1;
        self.store
            .save_sandbox(revived.revision, active)
            .await
            .unwrap();
        let mut ready = self.agent.clone();
        ready.status = AgentStatus::Ready;
        ready.revision += 1;
        self.store
            .save_agent(self.agent.revision, ready)
            .await
            .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn paused_binding_command_uses_retained_credential_and_schedules_then_dispatches_once() {
    for supplied in [false, true] {
        let (client, name) = database("binding_preparation_success").await;
        let result = if supplied {
            Err(SandboxCredentialPrerequisiteError::SetupRequired)
        } else {
            Ok(())
        };
        let fixture = Fixture::new(&client, &name, supplied, result).await;
        let worker = fixture.worker();
        let id = &fixture.command.receipt.envelope.command_id;
        assert_eq!(
            fixture.execution.check_queued(&fixture.command).await,
            CommandReadiness::Ready
        );
        worker.run_scheduled(id).await.unwrap();
        let revived = fixture.parent().await;
        assert_eq!(revived.status, SandboxStatus::Reviving);
        assert_eq!(fixture.sends.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.command().await, fixture.command);
        assert_eq!(
            fixture.prerequisite.calls.load(Ordering::SeqCst),
            usize::from(!supplied)
        );
        *fixture.prerequisite.result.lock().unwrap() =
            Err(SandboxCredentialPrerequisiteError::SetupRequired);
        worker.run_scheduled(id).await.unwrap();
        assert_eq!(fixture.command().await, fixture.command);
        assert_eq!(
            fixture.prerequisite.calls.load(Ordering::SeqCst),
            usize::from(!supplied)
        );
        assert_eq!(fixture.parent().await, revived);
        if !supplied {
            *fixture.prerequisite.result.lock().unwrap() = Ok(());
        }
        fixture.finish_revival().await;
        worker.run_scheduled(id).await.unwrap();
        worker.run_scheduled(id).await.unwrap();
        let dispatched = fixture.command().await;
        assert_eq!(dispatched.receipt, fixture.command.receipt);
        assert!(dispatched.work.dispatch.is_some());
        assert!(dispatched.work.failure.is_none());
        assert_eq!(fixture.sends.load(Ordering::SeqCst), 1);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn managed_prerequisite_refusals_fail_fast_with_typed_logged_results() {
    for (error, code, kind) in [
        (
            SandboxCredentialPrerequisiteError::SetupRequired,
            CommandFailureCode::CredentialSetupRequired,
            "credential_setup_required",
        ),
        (
            SandboxCredentialPrerequisiteError::ConnectionsRequired {
                services: vec![layer2_core::ServiceId::new("private-service-marker")],
            },
            CommandFailureCode::ConnectionsRequired,
            "connections_required",
        ),
    ] {
        let (client, name) = database("binding_preparation_refusal").await;
        let fixture = Fixture::new(&client, &name, false, Err(error)).await;
        let logs = Logs::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(logs.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let id = &fixture.command.receipt.envelope.command_id;
        assert_eq!(
            fixture.execution.prepare_queued(&fixture.command).await,
            Err(CommandPreparationError::Rejected(code))
        );
        assert_eq!(
            fixture.worker().run_scheduled(id).await.unwrap(),
            Duration::ZERO
        );
        let current = fixture.command().await;
        assert_eq!(
            current.work.failure.unwrap().code,
            CommandFailureCode::ExecutionFailed
        );
        assert_eq!(current.status, AgentResponseStatus::Failed);
        assert_eq!(current.receipt, fixture.command.receipt);
        assert!(current.work.dispatch.is_none());
        assert!(current.work.lease.is_none());
        assert_eq!(fixture.parent().await.status, SandboxStatus::Paused);
        assert_eq!(fixture.sends.load(Ordering::SeqCst), 0);
        let output = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
        for expected in [
            "command_readiness",
            "agent_not_ready",
            "revival_prerequisites",
            kind,
            id.as_str(),
            fixture.agent.id.as_str(),
        ] {
            assert!(output.contains(expected), "missing {expected}");
        }
        assert!(!output.contains("private-service-marker"));
        assert!(!output.contains("Inspect this."));
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn unavailable_prerequisite_is_typed_logged_and_retryable_without_changing_deadline() {
    let (client, name) = database("binding_preparation_transient").await;
    let fixture = Fixture::new(
        &client,
        &name,
        false,
        Err(SandboxCredentialPrerequisiteError::Unavailable(
            "private-provider-marker".into(),
        )),
    )
    .await;
    let logs = Logs::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(logs.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    assert_eq!(
        fixture.execution.prepare_queued(&fixture.command).await,
        Err(CommandPreparationError::DependencyUnavailable)
    );
    assert!(!fixture
        .worker()
        .run_scheduled(&fixture.command.receipt.envelope.command_id)
        .await
        .unwrap()
        .is_zero());
    assert_eq!(fixture.command().await, fixture.command);
    assert_eq!(fixture.parent().await.status, SandboxStatus::Paused);
    assert_eq!(fixture.sends.load(Ordering::SeqCst), 0);
    let output = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert!(output.contains("revival_prerequisites"));
    assert!(output.contains("prerequisite_unavailable"));
    assert!(!output.contains("private-provider-marker"));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn prerequisite_refusal_after_revival_is_terminal_before_runtime_dispatch() {
    for (error, code) in [
        (
            SandboxCredentialPrerequisiteError::SetupRequired,
            CommandFailureCode::CredentialSetupRequired,
        ),
        (
            SandboxCredentialPrerequisiteError::ConnectionsRequired {
                services: vec![layer2_core::ServiceId::new("service-fixture")],
            },
            CommandFailureCode::ConnectionsRequired,
        ),
    ] {
        let (client, name) = database("binding_dispatch_prerequisite").await;
        let fixture = Fixture::new(&client, &name, false, Ok(())).await;
        let worker = fixture.worker();
        let id = &fixture.command.receipt.envelope.command_id;
        worker.run_scheduled(id).await.unwrap();
        assert_eq!(fixture.parent().await.status, SandboxStatus::Reviving);
        fixture.finish_revival().await;
        *fixture.prerequisite.result.lock().unwrap() = Err(error);
        let logs = Logs::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(logs.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        worker.run_scheduled(id).await.unwrap();
        let output = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
        assert!(output.contains("command_refusal"));
        assert!(output.contains(&format!("error_kind={code:?}")));
        assert!(!output.contains("service-fixture") && !output.contains("Inspect this."));
        let current = fixture.command().await;
        assert_eq!(current.status, AgentResponseStatus::Failed);
        assert_eq!(
            current.work.failure.unwrap().code,
            CommandFailureCode::ExecutionFailed
        );
        assert!(current.work.dispatch.is_none());
        assert_eq!(fixture.sends.load(Ordering::SeqCst), 0);
        client.database(&name).drop().await.unwrap();
    }
}

struct UnavailableRuntime;
#[async_trait::async_trait]
impl AgentRuntimeRegistry for UnavailableRuntime {
    async fn resolve(
        &self,
        _: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        Err(AgentRuntimeError::Unavailable(
            "private-runtime-marker".into(),
        ))
    }
    async fn fence(&self, _: &AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        panic!("unavailable input cannot fence the runtime")
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn unavailable_runtime_keeps_original_queue_deadline_and_logs_its_cause() {
    let (client, name) = database("binding_runtime_unavailable").await;
    let fixture = Fixture::new(&client, &name, false, Ok(())).await;
    let id = &fixture.command.receipt.envelope.command_id;
    fixture.worker().run_scheduled(id).await.unwrap();
    fixture.finish_revival().await;
    let execution = Arc::new(crate::commands::ResourceCommandExecution {
        source_guard: None,
        authority: fixture.execution.authority.clone(),
        access: fixture.execution.access.clone(),
        store: fixture.store.clone(),
        context: fixture.execution.context.clone(),
        operations: AgentOperationService::new(fixture.store.clone(), Arc::new(UnavailableRuntime)),
    });
    let logs = Logs::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(logs.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    AgentCommandWorker::new(fixture.store.clone(), execution, "unavailable".into())
        .unwrap()
        .run_scheduled(id)
        .await
        .unwrap();
    let current = fixture.command().await;
    assert_eq!(current.status, AgentResponseStatus::Queued);
    assert_eq!(current.receipt, fixture.command.receipt);
    assert_eq!(current.queue_deadline, fixture.command.queue_deadline);
    assert!(
        current.work.lease.is_none()
            && current.work.dispatch.is_none()
            && current.work.failure.is_none()
    );
    let output = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert!(output.contains("command_execution") && output.contains("agent_runtime_unavailable"));
    assert!(!output.contains("private-runtime-marker"));
    client.database(&name).drop().await.unwrap();
}
