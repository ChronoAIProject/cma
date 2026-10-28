use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use layer2_core::profiles::store::{
    InMemoryProfileStore, SandboxProfileStore as _, WorkspaceProfileStore as _,
};
use layer2_core::profiles::{
    AgentProvider, EnvironmentVariable, ModelAccess, ProfileCapacityBounds, ProfileVisibility,
    SandboxProfile, SandboxProfileContent, SandboxProfileRef, WorkspaceLocation, WorkspaceProfile,
    WorkspaceProfileContent, WorkspaceProfileRef,
};
use layer2_core::sandbox::store::{
    PageRequest, SandboxEventStore as _, SandboxLifecycleCommand,
    SandboxLifecycleCommandStore as _, SandboxStore as _, SandboxStoreError,
    SandboxWorkerStore as _,
};
use layer2_core::sandbox::{
    AgentOperationService, CreateSandbox, DurableAgentUpdateRecorder, InMemorySandboxStore,
    Sandbox, SandboxEventKind, SandboxOrigin, SandboxPolicy, SandboxPolicySource,
    SandboxReviveCause, SandboxRotationIdentity, SandboxService, SandboxStatus, SandboxWorkStep,
};
use layer2_core::{
    rfc3339_at, seconds_from_rfc3339, Authorization, Caller, Capability, SandboxId,
    SandboxProfileId, ServiceId, StoreError, SubjectId, WorkspaceProfileId,
};
use layer3_adapter::codex::{CodexAdapter, CodexSandboxSpec};
use layer3_adapter::hosted::InMemoryHostedBackend;
use layer3_adapter::wire::{RequestFrame, WireOperation};
use layer3_adapter::{RuntimeHandle, SandboxAdapter};
use tokio::sync::Notify;

use crate::auth::persistence::SecretValue;

use super::credential::{CredentialError, CredentialMaterial, SandboxCredentialController};
use super::live_agents::LiveAgentRegistry;
use super::worker::{
    LiveAgentStopper, ProfileWorkspaceProvisioner, SandboxDependencyError, SandboxWorker,
    SandboxWorkerConfig, SandboxWorkerDependencies, CREDENTIAL_EXPIRY_MARGIN,
    CREDENTIAL_ROTATION_AGE,
};

mod background_checkpoint_tests;
mod binding_tests;
mod checkpoint_consistency_tests;
mod checkpoint_lease_tests;
mod checkpoint_maintenance_tests;
mod execution_tests;
mod home_tests;
mod hot_scope_tests;
mod lifecycle_failure_clock_tests;
mod lifecycle_queue_conflict_tests;
mod lifecycle_queue_contention_tests;
mod lifecycle_queue_tests;
mod lifecycle_recovery_tests;
mod native_pool_tests;
mod native_suspension_tests;
mod pause_cancellation_tests;
mod provided_credential_tests;
mod reconciliation_tests;
mod retirement_checkpoint_tests;
mod runtime_attachment_tests;
mod warmup_tests;
mod workspace_adoption_tests;
mod workspace_prepare_recovery_tests;

const PROFILE_TIME: &str = "2026-09-05T00:00:00Z";

fn runtime_image() -> String {
    CodexSandboxSpec::default().deployment.image.uri
}

#[derive(Clone, Default)]
struct ActionLog(Arc<Mutex<Vec<String>>>);

impl ActionLog {
    fn push(&self, action: impl Into<String>) {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(action.into());
    }

    fn snapshot(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

struct MutablePolicy(Mutex<SandboxPolicy>);

#[async_trait::async_trait]
impl SandboxPolicySource for MutablePolicy {
    async fn current_sandbox_policy(&self) -> Result<SandboxPolicy, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone())
    }
}

struct MemoryCredentials {
    materials: Mutex<BTreeMap<(SandboxId, u64), CredentialMaterial>>,
    issue_failures: Mutex<VecDeque<CredentialError>>,
    revoke_failures: AtomicUsize,
    forget_failures: AtomicUsize,
    expiry_seconds: AtomicU64,
    actions: ActionLog,
}

impl MemoryCredentials {
    fn new(actions: ActionLog) -> Arc<Self> {
        Arc::new(Self {
            materials: Mutex::new(BTreeMap::new()),
            issue_failures: Mutex::new(VecDeque::new()),
            revoke_failures: AtomicUsize::new(0),
            forget_failures: AtomicUsize::new(0),
            expiry_seconds: AtomicU64::new(30 * 24 * 60 * 60),
            actions,
        })
    }

    fn fail_issue(&self, failure: CredentialError) {
        self.issue_failures
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push_back(failure);
    }

    fn fail_revokes(&self, count: usize) {
        self.revoke_failures.store(count, Ordering::SeqCst);
    }

    fn fail_forgets(&self, count: usize) {
        self.forget_failures.store(count, Ordering::SeqCst);
    }

    fn restore(&self, material: CredentialMaterial) {
        self.materials
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                (material.sandbox_id.clone(), material.authority.generation),
                material,
            );
    }

    fn expire_after(&self, duration: Duration) {
        self.expiry_seconds
            .store(duration.as_secs(), Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl SandboxCredentialController for MemoryCredentials {
    async fn issue(
        &self,
        sandbox: &Sandbox,
        generation: u64,
        now: SystemTime,
    ) -> Result<CredentialMaterial, CredentialError> {
        if let Some(failure) = self
            .issue_failures
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop_front()
        {
            return Err(failure);
        }
        let key = (sandbox.id.clone(), generation);
        let mut materials = self
            .materials
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(material) = materials.get(&key) {
            return Ok(material.clone());
        }
        self.actions.push(format!("mint:{generation}"));
        let material = CredentialMaterial {
            source: Default::default(),
            sandbox_id: sandbox.id.clone(),
            owner: sandbox.owner.clone(),
            authority: layer2_core::sandbox::SandboxCredentialAuthority {
                generation,
                provider_key_id: format!("provider-key-{generation}"),
                provider_state_version: i64::try_from(generation).unwrap(),
                issued_at: rfc3339_at(now),
                expires_at: rfc3339_at(
                    now + Duration::from_secs(self.expiry_seconds.load(Ordering::SeqCst)),
                ),
                grant_fingerprint: format!("{generation:064x}"),
            },
            value: SecretValue::new(format!("one-time-secret-{generation}")),
        };
        materials.insert(key, material.clone());
        Ok(material)
    }

    async fn material(
        &self,
        sandbox_id: &SandboxId,
        generation: u64,
    ) -> Result<Option<CredentialMaterial>, CredentialError> {
        Ok(self
            .materials
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&(sandbox_id.clone(), generation))
            .cloned())
    }

    async fn custodied(
        &self,
        sandbox_id: &SandboxId,
    ) -> Result<Vec<CredentialMaterial>, CredentialError> {
        Ok(self
            .materials
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .filter(|((id, _), _)| id == sandbox_id)
            .map(|(_, material)| material.clone())
            .collect())
    }

    async fn forget(&self, sandbox_id: &SandboxId, generation: u64) -> Result<(), CredentialError> {
        if self
            .forget_failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            return Err(CredentialError::Unavailable(
                "simulated custody delete outage".into(),
            ));
        }
        self.materials
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&(sandbox_id.clone(), generation));
        Ok(())
    }

    async fn revoke(
        &self,
        _sandbox: &Sandbox,
        provider_key_id: &str,
    ) -> Result<(), CredentialError> {
        self.actions.push(format!("revoke:{provider_key_id}"));
        if self
            .revoke_failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            Err(CredentialError::Unavailable(
                "simulated revoke outage".into(),
            ))
        } else {
            Ok(())
        }
    }
}

struct BlockingCredentials {
    inner: Arc<MemoryCredentials>,
    block_first: AtomicBool,
    entered: Notify,
    release: Notify,
}

impl BlockingCredentials {
    fn new(inner: Arc<MemoryCredentials>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            block_first: AtomicBool::new(true),
            entered: Notify::new(),
            release: Notify::new(),
        })
    }
}

#[async_trait::async_trait]
impl SandboxCredentialController for BlockingCredentials {
    async fn issue(
        &self,
        sandbox: &Sandbox,
        generation: u64,
        now: SystemTime,
    ) -> Result<CredentialMaterial, CredentialError> {
        if self.block_first.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        self.inner.issue(sandbox, generation, now).await
    }

    async fn material(
        &self,
        sandbox_id: &SandboxId,
        generation: u64,
    ) -> Result<Option<CredentialMaterial>, CredentialError> {
        self.inner.material(sandbox_id, generation).await
    }

    async fn custodied(
        &self,
        sandbox_id: &SandboxId,
    ) -> Result<Vec<CredentialMaterial>, CredentialError> {
        self.inner.custodied(sandbox_id).await
    }

    async fn forget(&self, sandbox_id: &SandboxId, generation: u64) -> Result<(), CredentialError> {
        self.inner.forget(sandbox_id, generation).await
    }

    async fn revoke(
        &self,
        sandbox: &Sandbox,
        provider_key_id: &str,
    ) -> Result<(), CredentialError> {
        self.inner.revoke(sandbox, provider_key_id).await
    }
}

struct WorkspaceRecorder {
    actions: ActionLog,
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

struct AgentStopRecorder {
    actions: ActionLog,
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

struct Harness {
    store: Arc<InMemorySandboxStore>,
    service: Arc<SandboxService>,
    worker: Arc<SandboxWorker>,
    credentials: Arc<MemoryCredentials>,
    backend: InMemoryHostedBackend,
    adapter: Arc<dyn SandboxAdapter>,
    registry: Arc<LiveAgentRegistry>,
    actions: ActionLog,
    reference: SandboxProfileRef,
    policy: SandboxPolicy,
}

impl Harness {
    async fn new(commands: Vec<String>, workspace_count: u16, retention: Duration) -> Self {
        Self::new_with_retry_delay(commands, workspace_count, retention, Duration::from_secs(5))
            .await
    }

    async fn new_with_retry_delay(
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

    async fn create(&self) -> Sandbox {
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

    async fn stored(&self, id: &SandboxId) -> Sandbox {
        self.store.get_sandbox(id).await.unwrap().unwrap()
    }

    async fn run(&self, id: &SandboxId, now: SystemTime) {
        assert!(self.worker.run_one(id, now).await.unwrap());
    }

    async fn drive_active(&self, id: &SandboxId, now: SystemTime) -> Sandbox {
        loop {
            let sandbox = self.stored(id).await;
            if sandbox.status == SandboxStatus::Active {
                return sandbox;
            }
            self.run(id, now).await;
        }
    }

    fn worker_with(
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

    fn worker_with_policy_source(
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

    fn worker_with_execution(
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

    fn worker_with_adapter(
        &self,
        holder: &str,
        adapter: Arc<dyn SandboxAdapter>,
    ) -> Arc<SandboxWorker> {
        self.worker_with_runtime_image(holder, adapter, &runtime_image())
    }

    fn worker_with_runtime_image(
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

fn bounds() -> ProfileCapacityBounds {
    ProfileCapacityBounds {
        min_cpu_millis: 100,
        max_cpu_millis: 8_000,
        min_memory_mib: 128,
        max_memory_mib: 32_768,
    }
}

async fn seed_workspace_profile(
    profiles: &InMemoryProfileStore,
    index: u16,
) -> WorkspaceProfileRef {
    let id = WorkspaceProfileId(format!("wkp_worker_{index}"));
    profiles
        .create_workspace_profile(WorkspaceProfile {
            internal: false,
            id: id.clone(),
            owner: SubjectId::new("owner"),
            name: format!("Workspace {index}"),
            description: "Worker integration fixture".into(),
            visibility: ProfileVisibility::Private,
            forked_from: None,
            uses_count: 0,
            draft: WorkspaceProfileContent {
                location: WorkspaceLocation::Directory {
                    name: format!("project-{index}"),
                },
                name: format!("project-{index}"),
                setup_commands: Vec::new(),
                agent_profiles: Vec::new(),
            },
            draft_version: 1,
            created_at: PROFILE_TIME.into(),
            updated_at: PROFILE_TIME.into(),
            retired_at: None,
        })
        .await
        .unwrap();
    let revision = profiles
        .publish_workspace_profile(&id, 1, PROFILE_TIME.into())
        .await
        .unwrap();
    WorkspaceProfileRef::new(id, revision.revision)
}

async fn seed_sandbox_profile(
    profiles: &InMemoryProfileStore,
    commands: Vec<String>,
    workspace_profiles: Vec<WorkspaceProfileRef>,
) -> SandboxProfileRef {
    let id = SandboxProfileId("sbp_worker".into());
    profiles
        .create_sandbox_profile(SandboxProfile {
            internal: false,
            availability: Default::default(),
            id: id.clone(),
            owner: SubjectId::new("owner"),
            name: "Worker sandbox".into(),
            description: "Worker integration fixture".into(),
            visibility: ProfileVisibility::Private,
            forked_from: None,
            uses_count: 0,
            draft: SandboxProfileContent {
                runtime_image: None,
                cpu_millis: 1_000,
                memory_mib: 2_048,
                nyxid_services: vec![ServiceId::new("profile-service")],
                agent_provider: AgentProvider::Codex,
                model_access: ModelAccess::ChronoaiManaged,
                max_workspaces: 4,
                max_agents_per_workspace: 4,
                workspace_profiles,
                software_install_commands: commands,
                environment_variables: vec![EnvironmentVariable {
                    name: "PROJECT_MODE".into(),
                    value: "integration".into(),
                }],
            },
            draft_version: 1,
            created_at: PROFILE_TIME.into(),
            updated_at: PROFILE_TIME.into(),
            retired_at: None,
        })
        .await
        .unwrap();
    let revision = profiles
        .publish_sandbox_profile(&id, 1, PROFILE_TIME.into(), bounds(), &[])
        .await
        .unwrap();
    SandboxProfileRef::new(id, revision.revision)
}

fn caller(subject: &str, capabilities: impl IntoIterator<Item = Capability>) -> Caller {
    Caller::new(SubjectId::new(subject), Authorization::new(capabilities))
}

fn start_time(sandbox: &Sandbox) -> SystemTime {
    SystemTime::UNIX_EPOCH
        + Duration::from_secs(seconds_from_rfc3339(&sandbox.created_at).unwrap() + 1)
}

fn stamp_time(stamp: &str) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds_from_rfc3339(stamp).unwrap())
}

#[tokio::test]
async fn create_installs_before_credential_then_builds_workspaces_in_order() {
    let harness = Harness::new(
        vec!["apt-get update".into(), "apt-get install -y git".into()],
        2,
        Duration::from_secs(60),
    )
    .await;
    let created = harness.create().await;
    assert_eq!(created.status, SandboxStatus::Provisioning);
    let active = harness
        .drive_active(&created.id, start_time(&created))
        .await;

    assert_eq!(active.status, SandboxStatus::Active);
    assert_eq!(active.work.step, SandboxWorkStep::WaitForCredentialRotation);
    assert_eq!(active.credential.key_generation, 1);
    assert!(active.credential.current.is_some());
    assert!(active.credential.pending.is_none());
    assert!(harness
        .credentials
        .material(&created.id, 1)
        .await
        .unwrap()
        .is_some());
    let requests = harness.backend.requests().await;
    assert!(matches!(&requests[0].1, RequestFrame::Install(request)
        if request.commands == ["apt-get update", "apt-get install -y git"]));
    assert!(matches!(&requests[1].1, RequestFrame::Credential(frame)
        if frame.value.expose() == "one-time-secret-1"));
    assert_eq!(
        harness.actions.snapshot(),
        [
            "mint:1",
            "workspace:0:wkp_worker_0",
            "workspace:1:wkp_worker_1",
        ]
    );
    let binding = active.runtime.as_ref().unwrap();
    let runtime = harness
        .backend
        .snapshot(&RuntimeHandle(binding.runtime_id.clone()))
        .await
        .unwrap();
    assert_eq!(runtime.request.env["PROJECT_MODE"], "integration");
    assert_eq!(
        runtime.request.env["NYXID_ACCESS_TOKEN"],
        layer3_adapter::hosted::PLACEHOLDER_CREDENTIAL
    );
    assert!(runtime.credential_delivered);

    let events = harness
        .store
        .sandbox_events(&created.id, 0, 20)
        .await
        .unwrap();
    assert_eq!(events.len(), 8);
    assert_eq!(events[0].kind, SandboxEventKind::Created);
    assert_eq!(
        events[1].kind,
        SandboxEventKind::StatusChanged {
            from: SandboxStatus::Provisioning,
            to: SandboxStatus::Installing,
        }
    );
    assert_eq!(
        events[2].kind,
        SandboxEventKind::WorkStepCompleted {
            step: SandboxWorkStep::RunInstall,
        }
    );
    assert_eq!(
        events[3].kind,
        SandboxEventKind::WorkStepCompleted {
            step: SandboxWorkStep::MintInitialCredential,
        }
    );
    assert_eq!(
        events[4].kind,
        SandboxEventKind::CredentialChanged { key_generation: 1 }
    );
    assert_eq!(
        events[5].kind,
        SandboxEventKind::WorkStepCompleted {
            step: SandboxWorkStep::CreateProfileWorkspace(0),
        }
    );
    assert_eq!(
        events[6].kind,
        SandboxEventKind::WorkStepCompleted {
            step: SandboxWorkStep::CreateProfileWorkspace(1),
        }
    );
    assert_eq!(
        events[7].kind,
        SandboxEventKind::StatusChanged {
            from: SandboxStatus::Installing,
            to: SandboxStatus::Active,
        }
    );
}

#[tokio::test]
async fn worker_pauses_and_revives_a_sandbox_from_its_held_checkpoint() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
    let old_runtime = RuntimeHandle(active.runtime.as_ref().unwrap().runtime_id.clone());
    let pause_at = now + Duration::from_secs(1);
    harness
        .store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
            active.id.clone(),
            active.owner.clone(),
            rfc3339_at(pause_at),
            Some("inactive".into()),
        ))
        .await
        .unwrap();

    for _ in 0..4 {
        harness.run(&active.id, pause_at).await;
    }
    let paused = harness.stored(&active.id).await;
    assert_eq!(paused.status, SandboxStatus::Paused);
    assert!(paused.runtime.is_none());
    assert!(paused.lifecycle.operation.is_none());
    assert_eq!(
        paused
            .lifecycle
            .checkpoints
            .latest
            .as_ref()
            .unwrap()
            .conversations,
        []
    );
    assert!(harness.backend.snapshot(&old_runtime).await.is_err());

    let revive_at = pause_at + Duration::from_secs(1);
    harness
        .store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
            paused.id.clone(),
            paused.owner.clone(),
            rfc3339_at(revive_at),
            SandboxReviveCause::ExplicitRequest,
        ))
        .await
        .unwrap();
    for _ in 0..8 {
        harness.run(&paused.id, revive_at).await;
    }
    let revived = harness.stored(&paused.id).await;
    assert_eq!(revived.status, SandboxStatus::Active);
    assert!(revived.lifecycle.operation.is_none());
    assert!(revived.lifecycle.paused_notice.is_none());
    assert_ne!(revived.runtime.as_ref().unwrap().runtime_id, old_runtime.0);
    assert_eq!(
        revived.runtime.as_ref().unwrap().runtime_version,
        runtime_image()
    );
    assert!(harness
        .credentials
        .material(&revived.id, 1)
        .await
        .unwrap()
        .is_some());
    let requests = harness.backend.requests().await;
    assert!(requests
        .iter()
        .any(|(_, request)| matches!(request, RequestFrame::BootstrapRestore(_))));
    assert!(requests
        .iter()
        .any(|(_, request)| matches!(request, RequestFrame::Restore(_))));
}

#[tokio::test]
async fn worker_rotates_an_image_through_checkpoint_restore_without_installing_again() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
    let old_runtime = active.runtime.as_ref().unwrap().runtime_id.clone();
    let before = harness
        .backend
        .requests()
        .await
        .iter()
        .filter(|(_, request)| matches!(request, RequestFrame::Install(_)))
        .count();
    let target = format!("registry.example/runtime@sha256:{}", "b".repeat(64));
    let identity = SandboxRotationIdentity::capture(&active, &[]).unwrap();
    let expected_revision = active.revision;
    let rotation_at = now + Duration::from_secs(1);
    let mut rotating = active.clone();
    rotating
        .begin_image_rotation(target.clone(), identity, rfc3339_at(rotation_at))
        .unwrap();
    harness
        .store
        .save_sandbox(expected_revision, rotating)
        .await
        .unwrap();

    let mut spec = CodexSandboxSpec::default();
    spec.deployment.image.uri = target.clone();
    let worker = harness.worker_with_adapter(
        "rotation-worker",
        Arc::new(CodexAdapter::with_backend(harness.backend.clone(), spec)),
    );
    for _ in 0..11 {
        assert!(worker.run_one(&active.id, rotation_at).await.unwrap());
    }
    let rotated = harness.stored(&active.id).await;
    assert_eq!(rotated.status, SandboxStatus::Active);
    assert_eq!(rotated.lifecycle.rotation.rotations, 1);
    assert_ne!(rotated.runtime.as_ref().unwrap().runtime_id, old_runtime);
    assert_eq!(rotated.runtime.as_ref().unwrap().runtime_version, target);
    let requests = harness.backend.requests().await;
    assert_eq!(
        requests
            .iter()
            .filter(|(_, request)| matches!(request, RequestFrame::Install(_)))
            .count(),
        before
    );
    assert!(requests
        .iter()
        .any(|(_, request)| matches!(request, RequestFrame::BootstrapRestore(_))));
}

#[tokio::test]
async fn worker_rejects_unpinned_rotation_target_before_bootstrap_restore() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
    let identity = SandboxRotationIdentity::capture(&active, &[]).unwrap();
    let rotation_at = now + Duration::from_secs(1);
    let mut rotating = active.clone();
    rotating
        .begin_image_rotation("runtime-v3".into(), identity, rfc3339_at(rotation_at))
        .unwrap();
    harness
        .store
        .save_sandbox(active.revision, rotating)
        .await
        .unwrap();
    let worker = harness.worker_with_adapter(
        "rotation-worker",
        Arc::new(CodexAdapter::with_backend(
            harness.backend.clone(),
            CodexSandboxSpec::default(),
        )),
    );
    for _ in 0..6 {
        assert!(worker.run_one(&active.id, rotation_at).await.unwrap());
    }
    let failed = harness.stored(&active.id).await;
    assert_eq!(failed.status, SandboxStatus::Failed);
    let failure = failed.failure.as_ref().unwrap();
    assert_eq!(failure.class, "provisioning_failed");
    assert_eq!(failure.detail, "runtime rejected the lifecycle operation");
    assert!(failed.runtime.is_none());
    assert!(!harness
        .backend
        .requests()
        .await
        .iter()
        .any(|(_, request)| matches!(request, RequestFrame::BootstrapRestore(_))));
}

#[tokio::test]
async fn restarted_worker_reattaches_before_install_with_a_fence_independent_identity() {
    let harness = Harness::new(vec!["touch installed".into()], 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    harness.run(&created.id, now).await;
    let mut installing = harness.stored(&created.id).await;
    let revision = installing.revision;
    installing.runtime.as_mut().unwrap().runtime_version =
        CodexSandboxSpec::default().deployment.image.uri;
    installing.revision += 1;
    harness
        .store
        .save_sandbox(revision, installing.clone())
        .await
        .unwrap();
    let expected_runtime = installing.runtime.as_ref().unwrap().runtime_id.clone();

    let restarted: Arc<dyn SandboxAdapter> = Arc::new(CodexAdapter::with_backend(
        harness.backend.clone(),
        CodexSandboxSpec::default(),
    ));
    crate::sandbox::reconciliation::SandboxReconciler::new(
        harness.store.clone(),
        restarted.clone(),
        Arc::new(harness.policy.clone()),
        "test".into(),
    )
    .initialize(now)
    .await
    .unwrap();
    let worker = harness.worker_with_adapter("restarted-worker", restarted);
    assert!(worker.run_one(&created.id, now).await.unwrap());

    let installed = harness.stored(&created.id).await;
    assert_eq!(installed.work.step, SandboxWorkStep::MintInitialCredential);
    assert_eq!(
        installed.runtime.as_ref().unwrap().runtime_id,
        expected_runtime
    );
    let requests = harness.backend.requests().await;
    assert!(matches!(&requests[0].1, RequestFrame::Install(request)
        if request.operation_id == format!("sandbox-install-v1:{}", created.id)
            && request.commands == ["touch installed"]));
}

#[tokio::test]
async fn install_rejection_fails_with_the_command_index_and_delivers_no_secret() {
    let harness = Harness::new(vec!["false".into()], 0, Duration::from_secs(60)).await;
    harness
        .backend
        .fail_next(WireOperation::Install, "command_failed", "command 0 failed");
    let created = harness.create().await;
    let now = start_time(&created);
    harness.run(&created.id, now).await;
    harness.run(&created.id, now).await;
    let failed = harness.stored(&created.id).await;

    assert_eq!(failed.status, SandboxStatus::Failed);
    assert_eq!(failed.work.step, SandboxWorkStep::None);
    assert_eq!(
        failed.failure.as_ref().unwrap().class,
        "terminal_validation"
    );
    assert_eq!(
        failed.failure.as_ref().unwrap().detail,
        "sandbox install command 0 failed"
    );
    assert!(harness.actions.snapshot().is_empty());
    assert!(harness
        .backend
        .requests()
        .await
        .iter()
        .all(|(_, request)| !matches!(request, RequestFrame::Credential(_))));
}

#[tokio::test]
async fn an_ungranted_service_is_a_terminal_typed_failure() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    harness
        .credentials
        .fail_issue(CredentialError::ServiceNotGranted(ServiceId::new(
            "missing-service",
        )));
    let created = harness.create().await;
    let now = start_time(&created);
    harness.run(&created.id, now).await;
    harness.run(&created.id, now).await;
    harness.run(&created.id, now).await;
    let failed = harness.stored(&created.id).await;

    assert_eq!(failed.status, SandboxStatus::Failed);
    assert_eq!(
        failed.failure.as_ref().unwrap().class,
        "service_not_granted"
    );
    assert_eq!(failed.failure.as_ref().unwrap().detail, "missing-service");
    assert!(harness
        .backend
        .requests()
        .await
        .iter()
        .all(|(_, request)| !matches!(request, RequestFrame::Credential(_))));
}

#[tokio::test]
async fn transient_failure_keeps_the_step_and_delays_a_fenced_retry() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    harness
        .credentials
        .fail_issue(CredentialError::Unavailable("provider outage".into()));
    let created = harness.create().await;
    let now = start_time(&created);
    harness.run(&created.id, now).await;
    harness.run(&created.id, now).await;
    harness.run(&created.id, now).await;
    let retrying = harness.stored(&created.id).await;

    assert_eq!(retrying.status, SandboxStatus::Installing);
    assert_eq!(retrying.work.step, SandboxWorkStep::MintInitialCredential);
    assert_eq!(retrying.work.attempts, 1);
    assert!(retrying.work.lease.is_none());
    assert_eq!(
        retrying.work.ready_at,
        rfc3339_at(now + Duration::from_secs(5))
    );
    assert!(!harness.worker.run_one(&created.id, now).await.unwrap());
    harness.run(&created.id, now + Duration::from_secs(5)).await;
    let recovered = harness.stored(&created.id).await;
    assert_eq!(
        recovered.work.step,
        SandboxWorkStep::DeliverInitialCredential
    );
    assert_eq!(recovered.work.attempts, 0);

    let events = harness
        .store
        .sandbox_events(&created.id, 0, 20)
        .await
        .unwrap();
    assert!(matches!(events[3].kind, SandboxEventKind::WorkStepFailed {
        step: SandboxWorkStep::MintInitialCredential,
        ref failure,
    } if failure.class == "retryable_transient"));
}

#[tokio::test]
async fn due_sweep_continues_after_one_sandbox_returns_a_store_error() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let first = harness.create().await;
    let second = harness.create().await;
    let (blocked, following) = if first.id < second.id {
        (first, second)
    } else {
        (second, first)
    };
    let now = start_time(&blocked);
    harness.run(&blocked.id, now).await;
    harness.run(&blocked.id, now).await;
    assert_eq!(
        harness.stored(&blocked.id).await.work.step,
        SandboxWorkStep::MintInitialCredential
    );

    let blocking = BlockingCredentials::new(Arc::clone(&harness.credentials));
    let credentials: Arc<dyn SandboxCredentialController> = blocking.clone();
    let worker = harness.worker_with("sweep-worker", credentials, Duration::from_secs(1));
    let entered = blocking.entered.notified();
    let sweep = tokio::spawn(async move { worker.run_due(now).await });
    entered.await;
    harness
        .store
        .claim_sandbox_work(
            &blocked.id,
            "takeover-worker",
            rfc3339_at(now + Duration::from_secs(2)),
            rfc3339_at(now + Duration::from_secs(3)),
        )
        .await
        .unwrap()
        .unwrap();
    blocking.release.notify_one();

    assert_eq!(sweep.await.unwrap(), Err(SandboxStoreError::Conflict));
    assert_eq!(
        harness.stored(&following.id).await.status,
        SandboxStatus::Active
    );
}

#[tokio::test]
async fn rotation_runs_at_twelve_hours_and_revokes_only_after_delivery() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let first_active = harness.drive_active(&created.id, now).await;
    let first = first_active.credential.current.clone().unwrap();
    assert_eq!(
        first_active.work.ready_at,
        rfc3339_at(stamp_time(&first.issued_at) + CREDENTIAL_ROTATION_AGE)
    );
    let rotation_at = stamp_time(&first_active.work.ready_at);

    harness.run(&created.id, rotation_at).await;
    harness.run(&created.id, rotation_at).await;
    harness.run(&created.id, rotation_at).await;
    let delivered = harness.stored(&created.id).await;
    assert_eq!(
        delivered.work.step,
        SandboxWorkStep::RevokeRotatedCredential
    );
    assert_eq!(delivered.credential.current.as_ref().unwrap().generation, 2);
    assert_eq!(
        delivered.credential.revoke_debt[0].provider_key_id,
        first.provider_key_id
    );
    harness.run(&created.id, rotation_at).await;
    harness.run(&created.id, rotation_at).await;
    let active = harness.stored(&created.id).await;

    assert_eq!(active.status, SandboxStatus::Active);
    assert_eq!(active.credential.current.as_ref().unwrap().generation, 2);
    assert!(active.credential.revoke_debt.is_empty());
    assert_eq!(
        harness.actions.snapshot(),
        ["mint:1", "mint:2", "revoke:provider-key-1"]
    );
    assert_eq!(
        harness
            .backend
            .requests()
            .await
            .iter()
            .filter(|(_, request)| matches!(request, RequestFrame::Credential(_)))
            .count(),
        2
    );
}

#[tokio::test]
async fn expiry_margin_controls_delivery_and_the_earlier_rotation_deadline() {
    let too_short = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    too_short.credentials.expire_after(CREDENTIAL_EXPIRY_MARGIN);
    let created = too_short.create().await;
    let now = start_time(&created);
    too_short.run(&created.id, now).await;
    too_short.run(&created.id, now).await;
    too_short.run(&created.id, now).await;
    let failed = too_short.stored(&created.id).await;
    assert_eq!(failed.status, SandboxStatus::Failed);
    assert_eq!(failed.failure.as_ref().unwrap().class, "terminal_security");
    assert!(too_short
        .backend
        .requests()
        .await
        .iter()
        .all(|(_, request)| !matches!(request, RequestFrame::Credential(_))));

    let early = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    early.credentials.expire_after(Duration::from_secs(60 * 60));
    let created = early.create().await;
    let active = early.drive_active(&created.id, start_time(&created)).await;
    let authority = active.credential.current.as_ref().unwrap();
    assert_eq!(
        active.work.ready_at,
        rfc3339_at(stamp_time(&authority.expires_at) - CREDENTIAL_EXPIRY_MARGIN)
    );
}

#[tokio::test]
async fn delete_cleans_snapshots_before_retirement_and_preserves_history_until_grace() {
    let grace = Duration::from_secs(60);
    let harness = Harness::new(Vec::new(), 0, grace).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
    let deleter = caller("owner", [Capability::DeleteOwnSandbox]);
    let retiring = harness.service.delete(&deleter, &active.id).await.unwrap();
    assert_eq!(retiring.status, SandboxStatus::Retiring);

    for _ in 0..4 {
        harness.run(&created.id, now).await;
    }
    let retired = harness.stored(&created.id).await;
    assert_eq!(retired.status, SandboxStatus::Retired);
    assert!(retired.runtime.is_none());
    assert!(retired.credential.current.is_none());
    assert_eq!(retired.work.step, SandboxWorkStep::WaitForPurge);
    assert_eq!(retired.purge_after, Some(rfc3339_at(now + grace)));
    assert_eq!(
        harness.actions.snapshot(),
        ["mint:1", "stop-agents", "revoke:provider-key-1"]
    );
    assert!(!harness.worker.run_one(&created.id, now).await.unwrap());
    assert!(!harness
        .store
        .sandbox_events(&created.id, 0, 50)
        .await
        .unwrap()
        .is_empty());

    let purge_at = now + grace;
    harness.run(&created.id, purge_at).await;
    harness.run(&created.id, purge_at).await;
    assert!(harness
        .store
        .get_sandbox(&created.id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        harness.actions.snapshot(),
        ["mint:1", "stop-agents", "revoke:provider-key-1"]
    );
    assert!(harness
        .store
        .list_sandboxes(&SubjectId::new("owner"), &PageRequest::default())
        .await
        .unwrap()
        .items
        .is_empty());
}

#[tokio::test]
async fn retirement_reads_current_policy_when_it_sets_the_purge_deadline() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
    harness
        .service
        .delete(&caller("owner", [Capability::DeleteOwnSandbox]), &active.id)
        .await
        .unwrap();
    for _ in 0..3 {
        harness.run(&active.id, now).await;
    }
    assert_eq!(
        harness.stored(&active.id).await.work.step,
        SandboxWorkStep::MarkRetired
    );

    let source = Arc::new(MutablePolicy(Mutex::new(harness.policy.clone())));
    let policy_source: Arc<dyn SandboxPolicySource> = source.clone();
    let credentials: Arc<dyn SandboxCredentialController> = harness.credentials.clone();
    let worker = harness.worker_with_policy_source(
        "live-policy-worker",
        credentials,
        Duration::from_secs(60),
        policy_source,
    );
    source
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .retention_grace = Duration::from_secs(120);
    assert!(worker.run_one(&active.id, now).await.unwrap());

    let retired = harness.stored(&active.id).await;
    assert_eq!(retired.status, SandboxStatus::Retired);
    assert_eq!(
        retired.purge_after,
        Some(rfc3339_at(now + Duration::from_secs(120)))
    );
}

#[tokio::test]
async fn failed_retirement_revocation_is_durable_debt_and_retried() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
    harness.credentials.fail_revokes(1);
    harness
        .service
        .delete(&caller("owner", [Capability::DeleteOwnSandbox]), &active.id)
        .await
        .unwrap();

    for _ in 0..4 {
        harness.run(&created.id, now).await;
    }
    let retired_with_debt = harness.stored(&created.id).await;
    assert_eq!(retired_with_debt.status, SandboxStatus::Retired);
    assert_eq!(
        retired_with_debt.work.step,
        SandboxWorkStep::RetryRetiredRevocations
    );
    assert_eq!(retired_with_debt.credential.revoke_debt.len(), 1);
    assert_eq!(
        harness.actions.snapshot(),
        ["mint:1", "stop-agents", "revoke:provider-key-1"]
    );
    harness.run(&created.id, now).await;
    let settled = harness.stored(&created.id).await;
    assert!(settled.credential.revoke_debt.is_empty());
    assert_eq!(settled.work.step, SandboxWorkStep::WaitForPurge);
    assert_eq!(
        harness.actions.snapshot(),
        [
            "mint:1",
            "stop-agents",
            "revoke:provider-key-1",
            "revoke:provider-key-1"
        ]
    );
}

#[tokio::test]
async fn retirement_keeps_revoke_debt_until_custody_is_deleted() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
    let authority = active.credential.current.clone().unwrap();
    harness.credentials.restore(CredentialMaterial {
        source: Default::default(),
        sandbox_id: active.id.clone(),
        owner: active.owner.clone(),
        authority: authority.clone(),
        value: SecretValue::new("restored-custody".into()),
    });
    harness.credentials.fail_forgets(1);
    harness
        .service
        .delete(&caller("owner", [Capability::DeleteOwnSandbox]), &active.id)
        .await
        .unwrap();

    for _ in 0..4 {
        harness.run(&active.id, now).await;
    }
    let retired = harness.stored(&active.id).await;
    assert_eq!(retired.status, SandboxStatus::Retired);
    assert_eq!(retired.credential.revoke_debt.len(), 1);
    assert!(harness
        .credentials
        .material(&active.id, authority.generation)
        .await
        .unwrap()
        .is_some());

    harness.run(&active.id, now).await;
    let settled = harness.stored(&active.id).await;
    assert!(settled.credential.revoke_debt.is_empty());
    assert_eq!(settled.work.step, SandboxWorkStep::WaitForPurge);
    assert!(harness
        .credentials
        .material(&active.id, authority.generation)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn custody_that_appears_after_the_grace_check_blocks_purge_until_revoked() {
    let grace = Duration::from_secs(60);
    let harness = Harness::new(Vec::new(), 0, grace).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
    harness
        .service
        .delete(&caller("owner", [Capability::DeleteOwnSandbox]), &active.id)
        .await
        .unwrap();
    for _ in 0..4 {
        harness.run(&active.id, now).await;
    }

    let purge_at = now + grace;
    harness.run(&active.id, purge_at).await;
    assert_eq!(
        harness.stored(&active.id).await.work.step,
        SandboxWorkStep::Purge
    );
    harness.credentials.restore(CredentialMaterial {
        source: Default::default(),
        sandbox_id: active.id.clone(),
        owner: active.owner.clone(),
        authority: layer2_core::sandbox::SandboxCredentialAuthority {
            generation: 2,
            provider_key_id: "late-provider-key".into(),
            provider_state_version: 2,
            issued_at: rfc3339_at(purge_at),
            expires_at: rfc3339_at(purge_at + Duration::from_secs(3_600)),
            grant_fingerprint: "b".repeat(64),
        },
        value: SecretValue::new("late-custody".into()),
    });

    harness.run(&active.id, purge_at).await;
    let deferred = harness.stored(&active.id).await;
    assert_eq!(deferred.work.step, SandboxWorkStep::RetryRetiredRevocations);
    assert_eq!(deferred.credential.revoke_debt.len(), 1);
    assert!(!harness
        .actions
        .snapshot()
        .iter()
        .any(|action| action.as_str() == "purge-checkpoints"));

    harness.run(&active.id, purge_at).await;
    harness.run(&active.id, purge_at).await;
    harness.run(&active.id, purge_at).await;
    assert!(harness
        .store
        .get_sandbox(&active.id)
        .await
        .unwrap()
        .is_none());
    assert!(harness
        .actions
        .snapshot()
        .contains(&"revoke:late-provider-key".into()));
}

#[tokio::test]
async fn stale_mint_worker_is_fenced_after_lease_takeover() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    harness.run(&created.id, now).await;
    harness.run(&created.id, now).await;
    assert_eq!(
        harness.stored(&created.id).await.work.step,
        SandboxWorkStep::MintInitialCredential
    );

    let blocking = BlockingCredentials::new(Arc::clone(&harness.credentials));
    let credentials: Arc<dyn SandboxCredentialController> = blocking.clone();
    let worker_a = harness.worker_with(
        "stale-worker",
        Arc::clone(&credentials),
        Duration::from_secs(1),
    );
    let worker_b = harness.worker_with("takeover-worker", credentials, Duration::from_secs(1));
    let entered = blocking.entered.notified();
    let id = created.id.clone();
    let stale = tokio::spawn(async move { worker_a.run_one(&id, now).await });
    entered.await;

    assert!(worker_b
        .run_one(&created.id, now + Duration::from_secs(2))
        .await
        .unwrap());
    assert_eq!(
        harness.stored(&created.id).await.work.step,
        SandboxWorkStep::DeliverInitialCredential
    );
    blocking.release.notify_one();

    assert_eq!(stale.await.unwrap(), Err(SandboxStoreError::Conflict));
    assert_eq!(
        harness.stored(&created.id).await.work.step,
        SandboxWorkStep::DeliverInitialCredential
    );
}
