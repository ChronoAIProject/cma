use super::*;

pub(crate) struct WorkspaceRecorder {
    pub(crate) actions: ActionLog,
}

#[async_trait::async_trait]
impl ProfileWorkspaceProvisioner for WorkspaceRecorder {
    async fn create_profile_workspace(
        &self,
        _sandbox: &Sandbox,
        _runtime: &RuntimeHandle,
        profile: &WorkspaceProfileRef,
        index: u16,
        _operation_id: &str,
    ) -> Result<(), SandboxDependencyError> {
        self.actions
            .push(format!("workspace:{index}:{}", profile.id));
        Ok(())
    }
}

pub(crate) struct AgentStopRecorder {
    pub(crate) actions: ActionLog,
}

#[async_trait::async_trait]
impl LiveAgentStopper for AgentStopRecorder {
    async fn stop_live_agents(
        &self,
        _sandbox: &Sandbox,
        _runtime: Option<&RuntimeHandle>,
        _operation_id: &str,
    ) -> Result<(), SandboxDependencyError> {
        self.actions.push("stop-agents");
        Ok(())
    }
}

pub(super) struct Harness {
    pub(super) store: Arc<InMemorySandboxStore>,
    pub(super) service: Arc<SandboxService>,
    pub(super) worker: Arc<SandboxWorker>,
    pub(super) credentials: Arc<MemoryCredentials>,
    pub(super) backend: InMemoryHostedBackend,
    pub(super) adapter: Arc<dyn SandboxAdapter>,
    pub(super) registry: Arc<LiveAgentRegistry>,
    pub(super) actions: ActionLog,
    pub(super) reference: SandboxProfileRef,
    pub(super) policy: SandboxPolicy,
}

impl Harness {
    pub(super) async fn new(
        commands: Vec<String>,
        workspace_count: u16,
        retention: Duration,
    ) -> Self {
        Self::new_with_retry_delay(commands, workspace_count, retention, Duration::from_secs(5))
            .await
    }

    pub(super) async fn new_with_retry_delay(
        commands: Vec<String>,
        workspace_count: u16,
        retention: Duration,
        retry_delay: Duration,
    ) -> Self {
        let profiles = Arc::new(InMemoryProfileStore::default());
        let mut workspace_profiles = Vec::new();
        for index in 0..workspace_count {
            workspace_profiles.push(seed_workspace_profile(&profiles, index).await);
        }
        let reference = seed_sandbox_profile(&profiles, commands, workspace_profiles).await;
        let store = Arc::new(InMemorySandboxStore::default());
        let policy = SandboxPolicy {
            max_sandboxes_per_owner: 10,
            max_sandboxes_per_visitor: 1,
            inactivity_pause_after_seconds: 15 * 60,
            checkpoint_interval_seconds: layer2_core::DEFAULT_CHECKPOINT_INTERVAL_SECONDS,
            retention_grace: retention,
            baseline_services: vec![ServiceId::new("baseline-model")],
        };
        let service = SandboxService::new(
            store.clone(),
            profiles,
            policy.clone(),
            Arc::new(layer2_core::sandbox::FixedSandboxCredentialPrerequisite::Ready),
        );
        let actions = ActionLog::default();
        let credentials = MemoryCredentials::new(actions.clone());
        let (adapter, backend) = CodexAdapter::in_memory(CodexSandboxSpec::default());
        let adapter: Arc<dyn SandboxAdapter> = Arc::new(adapter);
        let registry = Arc::new(LiveAgentRegistry::default());
        let dependencies = SandboxWorkerDependencies {
            store: store.clone(),
            adapter: Arc::clone(&adapter),
            credentials: credentials.clone(),
            workspaces: Arc::new(WorkspaceRecorder {
                actions: actions.clone(),
            }),
            agents: Arc::new(AgentStopRecorder {
                actions: actions.clone(),
            }),
            registry: registry.clone(),
            updates: DurableAgentUpdateRecorder::new(store.clone()),
            operations: AgentOperationService::new(store.clone(), registry.clone()),
        };
        let mut config =
            SandboxWorkerConfig::new("worker-a", "test", runtime_image(), policy.clone());
        config.lease_duration = Duration::from_secs(60);
        config.retry_delay = retry_delay;
        let worker = SandboxWorker::new(dependencies, config);
        Self {
            store,
            service,
            worker,
            credentials,
            backend,
            adapter,
            registry,
            actions,
            reference,
            policy,
        }
    }

    pub(super) async fn create(&self) -> Sandbox {
        self.service
            .create(
                &caller(
                    "owner",
                    [
                        Capability::CreateSandbox,
                        Capability::UseOwnSandboxes,
                        Capability::DeleteOwnSandbox,
                    ],
                ),
                CreateSandbox {
                    profile: self.reference.clone(),
                    name: None,
                    confirmed_digest: None,
                    origin: SandboxOrigin::Manual,
                },
            )
            .await
            .unwrap()
    }

    pub(super) async fn stored(&self, id: &SandboxId) -> Sandbox {
        self.store.get_sandbox(id).await.unwrap().unwrap()
    }

    pub(super) async fn run(&self, id: &SandboxId, now: SystemTime) {
        assert!(self.worker.run_one(id, now).await.unwrap());
    }

    pub(super) async fn drive_active(&self, id: &SandboxId, now: SystemTime) -> Sandbox {
        loop {
            let sandbox = self.stored(id).await;
            if sandbox.status == SandboxStatus::Active {
                return sandbox;
            }
            self.run(id, now).await;
        }
    }

    pub(super) fn worker_with(
        &self,
        holder: &str,
        credentials: Arc<dyn SandboxCredentialController>,
        lease_duration: Duration,
    ) -> Arc<SandboxWorker> {
        self.worker_with_policy_source(
            holder,
            credentials,
            lease_duration,
            Arc::new(self.policy.clone()),
        )
    }

    pub(super) fn worker_with_policy_source(
        &self,
        holder: &str,
        credentials: Arc<dyn SandboxCredentialController>,
        lease_duration: Duration,
        policy: Arc<dyn SandboxPolicySource>,
    ) -> Arc<SandboxWorker> {
        self.worker_with_execution(
            holder,
            credentials,
            lease_duration,
            policy,
            Default::default(),
        )
    }

    pub(super) fn worker_with_execution(
        &self,
        holder: &str,
        credentials: Arc<dyn SandboxCredentialController>,
        lease_duration: Duration,
        policy: Arc<dyn SandboxPolicySource>,
        execution: layer2_core::sandbox::ProvisioningExecution,
    ) -> Arc<SandboxWorker> {
        let dependencies = SandboxWorkerDependencies {
            store: self.store.clone(),
            adapter: Arc::clone(&self.adapter),
            credentials,
            workspaces: Arc::new(WorkspaceRecorder {
                actions: self.actions.clone(),
            }),
            agents: Arc::new(AgentStopRecorder {
                actions: self.actions.clone(),
            }),
            registry: self.registry.clone(),
            updates: DurableAgentUpdateRecorder::new(self.store.clone()),
            operations: AgentOperationService::new(self.store.clone(), self.registry.clone()),
        };
        let mut config =
            SandboxWorkerConfig::with_policy_source(holder, "test", runtime_image(), policy)
                .with_execution(execution);
        config.lease_duration = lease_duration;
        config.retry_delay = Duration::from_secs(5);
        SandboxWorker::new(dependencies, config)
    }

    pub(super) fn worker_with_adapter(
        &self,
        holder: &str,
        adapter: Arc<dyn SandboxAdapter>,
    ) -> Arc<SandboxWorker> {
        self.worker_with_runtime_image(holder, adapter, &runtime_image())
    }

    pub(super) fn worker_with_runtime_image(
        &self,
        holder: &str,
        adapter: Arc<dyn SandboxAdapter>,
        image: &str,
    ) -> Arc<SandboxWorker> {
        let dependencies = SandboxWorkerDependencies {
            store: self.store.clone(),
            adapter,
            credentials: self.credentials.clone(),
            workspaces: Arc::new(WorkspaceRecorder {
                actions: self.actions.clone(),
            }),
            agents: Arc::new(AgentStopRecorder {
                actions: self.actions.clone(),
            }),
            registry: self.registry.clone(),
            updates: DurableAgentUpdateRecorder::new(self.store.clone()),
            operations: AgentOperationService::new(self.store.clone(), self.registry.clone()),
        };
        let mut config = SandboxWorkerConfig::new(holder, "test", image, self.policy.clone());
        config.lease_duration = Duration::from_secs(60);
        config.retry_delay = Duration::from_secs(5);
        SandboxWorker::new(dependencies, config)
    }
}
