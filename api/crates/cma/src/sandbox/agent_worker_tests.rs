use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use layer2_core::profiles::{
    AgentProfileContent, AgentProfileSnapshot, AgentProvider, ModelAccess, PermissionLevel,
    SandboxProfileContent, SandboxProfileSnapshot, SkillRef, SkillSource, ThinkingEffort,
    WorkspaceLocation, WorkspaceProfileContent, WorkspaceProfileSnapshot,
};
use layer2_core::sandbox::store::{
    AgentAdmission, AgentAdmissionResult, AgentAdmissionStore as _, AgentControlStore as _,
    AgentControlTarget, AgentConversationMutation, AgentConversationStore as _, AgentDeleteRequest,
    AgentLogStore as _, AgentStore as _, AgentWorkerStore as _, SandboxEventStore as _,
    SandboxStore as _, WorkspaceStore as _, WorktreeStore as _,
};
use layer2_core::sandbox::{
    Agent, AgentLogKind, AgentOperationService, AgentOrigin, AgentRuntimeError,
    AgentRuntimeRegistry, AgentRuntimeTarget, AgentStatus, AgentStopReason, AgentWorkStep,
    DurableAgentUpdateRecorder, FirstMessageDelivery, FirstMessageRecord, InMemorySandboxStore,
    ResolvedAgentSkill, RuntimeBinding, Sandbox, SandboxCredentialAuthority,
    SandboxCredentialState, SandboxLimits, SandboxOrigin, SandboxSignInState, SandboxStatus,
    SandboxWorkState, SandboxWorkStep, Workspace, WorkspaceOrigin, WorkspaceSource,
    WorkspaceStatus, WorkspaceWorkState, WorkspaceWorkStep, WorktreeStatus,
};
use layer2_core::{
    rfc3339_at, AgentId, AgentProfileId, RuntimeCapabilities, RuntimeCapability, SandboxId,
    SandboxProfileId, ServiceId, SkillId, SubjectId, WorkspaceId, WorkspaceProfileId, WorktreeId,
};
use layer3_adapter::codex::{CodexAdapter, CodexSandboxSpec};
use layer3_adapter::hosted::InMemoryHostedBackend;
use layer3_adapter::wire::WireOperation;
use layer3_adapter::{
    AdapterError, AgentSkill, AgentSlot, ConversationInput, ConversationInputSender,
    ConversationUpdate, CredentialFrame, InstallRequest, RuntimeCredential, RuntimeHandle,
    SandboxAdapter, SandboxCapacity, SandboxSpec, WorkspaceRequest,
    WorkspaceSource as AdapterWorkspaceSource,
};
use tokio::sync::mpsc;

use super::agent_skills::{AgentSkillResolutionError, AgentSkillResolver, EmptyAgentSkillResolver};
use super::agent_worker::{AgentWorker, AgentWorkerConfig, AgentWorkerDependencies};
use super::live_agents::{
    LiveAgentAttachmentIdentity, LiveAgentConnectionLossHandler, LiveAgentRegistry,
    LiveAgentRegistryError, LiveAgentTarget,
};

const DEPLOYMENT_ID: &str = "lifecycle-test";

mod fixture_store;
use fixture_store::FixtureStore;
mod connection_contention_tests;
mod connection_failure_tests;
mod first_message_contention_tests;
mod first_message_publication_tests;
mod lifecycle_connection_tests;
mod open_publication_recovery_tests;
mod prepared_pair_tests;
mod retired_runtime_tests;
mod runtime_attachment_tests;
mod sdk_conformance_tests;
mod settings_replay_tests;
mod sign_in_tests;
mod skill_material_tests;
mod staged_connection_tests;
mod startup_failure_tests;
mod startup_publication_tests;
mod startup_scheduling_tests;
mod warm_handoff_tests;
mod workspace_removal_tests;
mod worktree_recovery_tests;

struct Harness<S = InMemorySandboxStore> {
    store: Arc<S>,
    worker: Arc<AgentWorker>,
    registry: Arc<LiveAgentRegistry>,
    backend: InMemoryHostedBackend,
    adapter: Arc<dyn SandboxAdapter>,
    runtime: RuntimeHandle,
    workspace: Workspace,
    now: SystemTime,
    updates: Arc<DurableAgentUpdateRecorder>,
}

impl Harness {
    async fn new(skills: Arc<dyn AgentSkillResolver>) -> Self {
        Self::with_generation(skills, 1).await
    }

    async fn with_generation(skills: Arc<dyn AgentSkillResolver>, generation: u64) -> Self {
        Self::with_store(
            Arc::new(InMemorySandboxStore::default()),
            skills,
            generation,
        )
        .await
    }
}

impl<S: FixtureStore> Harness<S> {
    async fn with_store(
        store: Arc<S>,
        skills: Arc<dyn AgentSkillResolver>,
        generation: u64,
    ) -> Self {
        Self::with_store_services(store, skills, generation, None).await
    }

    async fn with_store_services(
        store: Arc<S>,
        skills: Arc<dyn AgentSkillResolver>,
        generation: u64,
        services: Option<Vec<ServiceId>>,
    ) -> Self {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_788_537_600);
        let stamp = rfc3339_at(now);
        let owner = SubjectId::new("owner");
        let sandbox_id = SandboxId::generate();
        let mut sandbox = sandbox(&sandbox_id, owner.clone(), &stamp);
        if let Some(mut services) = services {
            services.sort();
            sandbox.credential.reaches = services.clone();
            sandbox.profile.content.nyxid_services = services;
            sandbox.profile.digest =
                layer2_core::profiles::content_digest(&sandbox.profile.content).unwrap();
        }
        let (adapter, backend) = CodexAdapter::in_memory(CodexSandboxSpec::default());
        let adapter = Arc::new(adapter);
        let runtime = adapter
            .provision_runtime(&layer3_adapter::RuntimeProvisionRequest::new(
                generation,
                sandbox_spec(&sandbox),
            ))
            .await
            .unwrap();
        adapter
            .run_install(
                &runtime,
                &InstallRequest::new("sandbox-install-v1:lifecycle", Vec::new()),
            )
            .await
            .unwrap();
        adapter
            .deliver_credential(
                &runtime,
                CredentialFrame {
                    operation_id: "credential-1".into(),
                    value: RuntimeCredential::new("opaque-sandbox-key"),
                    custody_generation: Some(3),
                },
            )
            .await
            .unwrap();
        sandbox.runtime = Some(RuntimeBinding {
            boot_id: None,
            provision_generation: Some(generation),
            runtime_id: runtime.0.clone(),
            runtime_version: CodexSandboxSpec::default().deployment.image.uri,
            bound_at: stamp.clone(),
        });
        sandbox.lifecycle.runtime_generation_high_water = generation;
        store.create_sandbox(sandbox).await.unwrap();

        let workspace_id = WorkspaceId::generate();
        let receipt = adapter
            .prepare_workspace(
                &runtime,
                &WorkspaceRequest::new(
                    "workspace:project:prepare:v1",
                    workspace_id.to_string(),
                    "project",
                    AdapterWorkspaceSource::Init,
                ),
            )
            .await
            .unwrap();
        let workspace = workspace(
            workspace_id,
            sandbox_id,
            owner,
            receipt.path,
            receipt.resolved_commit,
            &stamp,
        );
        store.create_workspace(workspace.clone()).await.unwrap();

        let registry = Arc::new(LiveAgentRegistry::default());
        let operations = AgentOperationService::new(store.clone(), registry.clone());
        let updates = DurableAgentUpdateRecorder::new(store.clone());
        let dependencies = AgentWorkerDependencies {
            store: store.clone(),
            adapter: adapter.clone(),
            skills,
            registry: registry.clone(),
            updates: updates.clone(),
            operations,
        };
        let mut config = AgentWorkerConfig::new("agent-worker", DEPLOYMENT_ID);
        config.lease_duration = Duration::from_secs(30);
        config.retry_delay = Duration::from_secs(1);
        config.open_timeout = Duration::from_secs(2);
        let worker = AgentWorker::new(dependencies, config);
        Self {
            store,
            worker,
            registry,
            backend,
            adapter,
            runtime,
            workspace,
            now,
            updates,
        }
    }

    async fn admit(&self, key: &str, skills: Vec<SkillRef>) -> Agent {
        self.admit_with_first_message(key, skills, None).await
    }

    async fn admit_with_first_message(
        &self,
        key: &str,
        skills: Vec<SkillRef>,
        first_message: Option<&str>,
    ) -> Agent {
        let stamp = rfc3339_at(self.now);
        let result = self
            .store
            .admit_agent(AgentAdmission {
                binding: None,
                created_by: None,
                owner: self.workspace.owner.clone(),
                workspace_id: self.workspace.id.clone(),
                agent_id: AgentId::generate(),
                worktree_id: WorktreeId::generate(),
                profile: agent_profile(skills),
                capabilities: RuntimeCapabilities::new([
                    RuntimeCapability::Conversation,
                    RuntimeCapability::Steer,
                    RuntimeCapability::Interrupt,
                    RuntimeCapability::Usage,
                    RuntimeCapability::Resources,
                ]),
                title: None,
                first_message: first_message
                    .map(|text| FirstMessageRecord::new(key, text, stamp.clone()).unwrap()),
                origin: AgentOrigin::Manual,
                creation_key: key.into(),
                event_at: stamp,
            })
            .await
            .unwrap();
        match result {
            AgentAdmissionResult::Created(creation) => creation.agent,
            AgentAdmissionResult::Existing(_) => panic!("expected fresh admission"),
        }
    }

    async fn run(&self, id: &AgentId, now: SystemTime) {
        assert!(self.worker.run_one(id, now).await.unwrap());
    }

    async fn drive_ready(&self, id: &AgentId) -> Agent {
        for _ in 0..5 {
            self.run(id, self.now).await;
        }
        let agent = self.stored(id).await;
        assert_eq!(agent.status, AgentStatus::Ready);
        assert_eq!(agent.work.step, AgentWorkStep::None);
        agent
    }

    async fn stored(&self, id: &AgentId) -> Agent {
        self.store.get_agent(id).await.unwrap().unwrap()
    }
}

impl Harness {
    fn target(agent: &Agent) -> LiveAgentTarget {
        LiveAgentTarget {
            agent_id: agent.id.clone(),
            conversation_id: agent.conversation.conversation_id.clone().unwrap(),
            runtime_generation: agent.conversation.runtime_generation.unwrap(),
        }
    }

    fn replace_input(
        &self,
        agent: &Agent,
        authority_fence: u64,
        input: ConversationInputSender,
    ) -> mpsc::Sender<ConversationUpdate> {
        let (updates, receiver) = mpsc::channel(4);
        let attachment = self
            .registry
            .attach(
                agent.id.clone(),
                agent.conversation.conversation_id.clone().unwrap(),
                agent.conversation.runtime_generation.unwrap(),
                authority_fence,
                RuntimeCapabilities::new([RuntimeCapability::Conversation]),
                input,
                receiver,
                Arc::new(NoopRecorder),
            )
            .unwrap();
        self.registry.publish_and_activate(&attachment).unwrap();
        updates
    }
}

fn sandbox(id: &SandboxId, owner: SubjectId, stamp: &str) -> Sandbox {
    Sandbox {
        id: id.clone(),
        owner,
        name: "research".into(),
        origin: SandboxOrigin::Manual,
        profile: SandboxProfileSnapshot {
            id: SandboxProfileId::generate(),
            revision: 1,
            digest: "a".repeat(64),
            content: SandboxProfileContent {
                runtime_image: None,
                cpu_millis: 1_000,
                memory_mib: 2_048,
                nyxid_services: vec![ServiceId::new("ornn-api")],
                agent_provider: AgentProvider::Codex,
                model_access: ModelAccess::ChronoaiManaged,
                max_workspaces: 1,
                max_agents_per_workspace: 8,
                workspace_profiles: Vec::new(),
                software_install_commands: Vec::new(),
                environment_variables: Vec::new(),
            },
        },
        status: SandboxStatus::Active,
        failure: None,
        runtime: None,
        credential: SandboxCredentialState {
            source: Default::default(),
            service_grants: Vec::new(),
            service_scope: None,
            reaches: vec![ServiceId::new("ornn-api")],
            key_generation: 3,
            delivered: true,
            current: Some(SandboxCredentialAuthority {
                generation: 3,
                provider_key_id: "provider-key-3".into(),
                provider_state_version: 3,
                issued_at: stamp.into(),
                expires_at: stamp.into(),
                grant_fingerprint: "b".repeat(64),
            }),
            pending: None,
            revoke_debt: Vec::new(),
            sign_in: SandboxSignInState::NotRequired,
        },
        work: SandboxWorkState::new(SandboxWorkStep::None, stamp.into()),
        limits: SandboxLimits {
            max_workspaces: 1,
            max_agents_per_workspace: 8,
        },
        lifecycle: Default::default(),
        last_activity_at: stamp.into(),
        created_at: stamp.into(),
        updated_at: stamp.into(),
        retired_at: None,
        purge_after: None,
        revision: 1,
    }
}

fn workspace(
    id: WorkspaceId,
    sandbox_id: SandboxId,
    owner: SubjectId,
    path: String,
    resolved_commit: String,
    stamp: &str,
) -> Workspace {
    Workspace {
        id,
        sandbox_id,
        owner,
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: WorkspaceProfileId::generate(),
            revision: 1,
            digest: "c".repeat(64),
            content: WorkspaceProfileContent {
                location: WorkspaceLocation::Directory {
                    name: "project".into(),
                },
                name: "project".into(),
                setup_commands: Vec::new(),
                agent_profiles: Vec::new(),
            },
        },
        name: "project".into(),
        path,
        source: WorkspaceSource::Empty,
        resolved_commit: Some(resolved_commit),
        status: WorkspaceStatus::Ready,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::None, stamp.into()),
        next_worktree_number: 1,
        created_at: stamp.into(),
        ready_at: Some(stamp.into()),
        failed: None,
        removed_at: None,
        revision: 1,
    }
}

fn sandbox_spec(sandbox: &Sandbox) -> SandboxSpec {
    SandboxSpec {
        sandbox_id: sandbox.id.to_string(),
        capacity: SandboxCapacity {
            cpu_millis: sandbox.profile.content.cpu_millis,
            memory_mib: sandbox.profile.content.memory_mib,
        },
        environment: BTreeMap::new(),
        reaches: sandbox
            .credential
            .reaches
            .iter()
            .map(ToString::to_string)
            .collect(),
        own_credential: false,
        created_by: Some(DEPLOYMENT_ID.into()),
        ..SandboxSpec::default()
    }
}

fn agent_profile(skills: Vec<SkillRef>) -> AgentProfileSnapshot {
    AgentProfileSnapshot {
        id: AgentProfileId::generate(),
        revision: 1,
        digest: "d".repeat(64),
        content: AgentProfileContent {
            isolated_agent: false,
            model: Some("gpt-test".into()),
            thinking_effort: ThinkingEffort::High,
            permission_level: PermissionLevel::Autonomous,
            skills,
            system_prompt: "Be exact.".into(),
        },
    }
}

#[derive(Default)]
struct ScriptedSkills {
    results: Mutex<VecDeque<Result<Vec<AgentSkill>, AgentSkillResolutionError>>>,
    locators: Mutex<VecDeque<Vec<String>>>,
    calls: AtomicUsize,
}

impl ScriptedSkills {
    fn new(
        results: impl IntoIterator<Item = Result<Vec<AgentSkill>, AgentSkillResolutionError>>,
    ) -> Arc<Self> {
        let results: VecDeque<_> = results.into_iter().collect();
        let locators = results
            .iter()
            .filter_map(|result| result.as_ref().ok())
            .map(|skills| {
                skills
                    .iter()
                    .map(|skill| skill.source_url.clone())
                    .collect()
            })
            .collect();
        Arc::new(Self {
            results: Mutex::new(results),
            locators: Mutex::new(locators),
            calls: AtomicUsize::new(0),
        })
    }
}

#[async_trait::async_trait]
impl AgentSkillResolver for ScriptedSkills {
    async fn resolve(
        &self,
        _sandbox: &Sandbox,
        _references: &[SkillRef],
    ) -> Result<Vec<ResolvedAgentSkill>, AgentSkillResolutionError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.results
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop_front()
            .expect("scripted skill result")
            .map(|skills| {
                skills
                    .into_iter()
                    .map(|skill| ResolvedAgentSkill {
                        source: SkillSource::Ornn,
                        id: SkillId::new(skill.id),
                        name: skill.name,
                        version: skill.version.expect("resolved literal"),
                        archive_sha256: skill.archive_sha256,
                    })
                    .collect()
            })
    }

    fn runtime_skills(
        &self,
        material: &[ResolvedAgentSkill],
    ) -> Result<Vec<AgentSkill>, AgentSkillResolutionError> {
        let locators = self
            .locators
            .lock()
            .unwrap()
            .pop_front()
            .expect("scripted runtime locators");
        Ok(material
            .iter()
            .zip(locators)
            .map(|(skill, source_url)| AgentSkill {
                id: skill.id.as_str().to_owned(),
                name: skill.name.clone(),
                version: Some(skill.version.clone()),
                archive_sha256: skill.archive_sha256.clone(),
                source_url,
            })
            .collect())
    }
}

#[tokio::test]
async fn startup_is_isolated_receipted_published_and_records_runtime_updates() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let first = harness.admit("first", Vec::new()).await;
    let second = harness.admit("second", Vec::new()).await;
    let first = harness.drive_ready(&first.id).await;
    let second = harness.drive_ready(&second.id).await;
    let first_worktree = harness
        .store
        .get_worktree(&first.worktree_id)
        .await
        .unwrap()
        .unwrap();
    let second_worktree = harness
        .store
        .get_worktree(&second.worktree_id)
        .await
        .unwrap()
        .unwrap();

    assert_ne!(first.id, second.id);
    assert_ne!(first_worktree.name, second_worktree.name);
    assert_ne!(first_worktree.branch, second_worktree.branch);
    assert_ne!(first_worktree.path, second_worktree.path);
    assert_ne!(
        first.conversation.conversation_id,
        second.conversation.conversation_id
    );
    assert_eq!(first_worktree.status, WorktreeStatus::Ready);
    assert_eq!(second_worktree.status, WorktreeStatus::Ready);
    assert_eq!(
        first
            .work
            .receipts
            .iter()
            .map(|receipt| receipt.step)
            .collect::<Vec<_>>(),
        [
            AgentWorkStep::AddWorktree,
            AgentWorkStep::MaterializeHome,
            AgentWorkStep::OpenConversation,
        ]
    );
    assert_eq!(
        first.work.receipts[0].operation_id,
        format!("worktree:project:{}:add:v1", first_worktree.name)
    );
    assert_eq!(
        first.work.receipts[1].operation_id,
        format!("agent:{}:home:create:v1", first.id)
    );
    assert_eq!(
        first.work.receipts[2].operation_id,
        format!("agent:{}:open:v1", first.id)
    );
    assert!(first
        .work
        .receipts
        .iter()
        .all(|receipt| receipt.request_sha256.len() == 64));

    let snapshot = harness.backend.snapshot(&harness.runtime).await.unwrap();
    assert_eq!(snapshot.worktrees.len(), 2);
    assert_eq!(snapshot.homes.len(), 2);
    assert_eq!(snapshot.agents.len(), 2);
    assert!(snapshot
        .homes
        .contains_key(&AgentSlot::new(first.id.to_string())));
    assert!(snapshot
        .homes
        .contains_key(&AgentSlot::new(second.id.to_string())));

    let logs = harness.store.agent_log(&first.id, 0, 20).await.unwrap();
    assert!(matches!(
        logs.as_slice(),
        [
            _,
            _,
            _,
            _,
            layer2_core::sandbox::AgentLogEntry {
                kind: AgentLogKind::ConversationOpened { .. },
                ..
            },
            layer2_core::sandbox::AgentLogEntry {
                kind: AgentLogKind::StatusChanged {
                    from: AgentStatus::Starting,
                    to: AgentStatus::Ready,
                },
                ..
            },
            layer2_core::sandbox::AgentLogEntry {
                kind: AgentLogKind::WorkStepCompleted {
                    step: AgentWorkStep::PublishConnection,
                },
                ..
            }
        ]
    ));
    let events = harness
        .store
        .sandbox_events(&first.sandbox_id, 0, 40)
        .await
        .unwrap();
    assert!(events.iter().any(|event| matches!(
        &event.kind,
        layer2_core::sandbox::SandboxEventKind::AgentStatusChanged {
            agent_id,
            from: AgentStatus::Starting,
            to: AgentStatus::Ready,
        } if agent_id == &first.id
    )));

    let target = Harness::target(&first);
    let connection = harness.registry.resolve(&target).expect("published agent");
    assert_eq!(connection.capabilities, first.capabilities);
    assert!(harness
        .registry
        .resolve(&LiveAgentTarget {
            runtime_generation: 2,
            ..target.clone()
        })
        .is_none());
    assert!(harness
        .registry
        .resolve(&LiveAgentTarget {
            conversation_id: "another-conversation".into(),
            ..target
        })
        .is_none());

    harness
        .backend
        .send_agent_update(
            &harness.runtime,
            &AgentSlot::new(first.id.to_string()),
            ConversationUpdate::Lost { frames: 2 },
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let logs = harness.store.agent_log(&first.id, 0, 20).await.unwrap();
            if logs
                .iter()
                .any(|entry| matches!(entry.kind, AgentLogKind::ConversationGap { frames: 2 }))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("runtime update persisted");
}

// These first-message fixtures reuse one wall-clock instant for startup steps.
// Freeze elapsed time too, so a step cannot publish the next second while the
// following claim still uses the previous one. Retry deadlines remain explicit.
#[tokio::test(start_paused = true)]
async fn worker_dispatches_a_configured_first_message_and_retires_lifecycle_work() {
    let mut harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    harness.now = SystemTime::now();
    let agent = harness
        .admit_with_first_message("first-message", Vec::new(), Some("Inspect the workspace"))
        .await;
    for _ in 0..6 {
        harness.run(&agent.id, harness.now).await;
    }

    let dispatched = harness.stored(&agent.id).await;
    let first = dispatched.conversation.first_message.as_ref().unwrap();
    assert_eq!(dispatched.status, AgentStatus::Working);
    assert_eq!(dispatched.work.step, AgentWorkStep::None);
    assert_eq!(first.state, FirstMessageDelivery::WriteConfirmed);
    assert_eq!(first.attempt, 1);
    let logs = harness.store.agent_log(&agent.id, 0, 40).await.unwrap();
    assert_eq!(
        logs.iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::OwnerMessage { .. }))
            .count(),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn restart_reattaches_without_overwriting_first_message_work() {
    let mut harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    harness.now = SystemTime::now();
    let agent = harness
        .admit_with_first_message(
            "restart-first-message",
            Vec::new(),
            Some("Inspect after restart"),
        )
        .await;
    for _ in 0..5 {
        harness.run(&agent.id, harness.now).await;
    }
    let dispatching = harness.stored(&agent.id).await;
    assert_eq!(dispatching.work.step, AgentWorkStep::DispatchFirstMessage);
    assert_eq!(
        dispatching
            .conversation
            .first_message
            .as_ref()
            .unwrap()
            .state,
        FirstMessageDelivery::Queued
    );
    harness.registry.fence(
        &dispatching.id,
        dispatching.conversation.runtime_generation,
        dispatching.work.next_fence - 1,
    );

    assert_eq!(
        harness
            .worker
            .reconcile_connections(harness.now)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        harness.stored(&agent.id).await.work.step,
        AgentWorkStep::DispatchFirstMessage
    );
    harness.run(&agent.id, harness.now).await;

    let recovered = harness.stored(&agent.id).await;
    assert_eq!(recovered.status, AgentStatus::Working);
    assert_eq!(recovered.work.step, AgentWorkStep::None);
    assert_eq!(
        recovered.conversation.first_message.as_ref().unwrap().state,
        FirstMessageDelivery::WriteConfirmed
    );
    assert_eq!(
        harness
            .store
            .agent_log(&agent.id, 0, 40)
            .await
            .unwrap()
            .iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::OwnerMessage { .. }))
            .count(),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn worker_completes_an_unconfigured_dispatch_step_without_runtime_input() {
    let mut harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    harness.now = SystemTime::now();
    let agent = harness.admit("unconfigured-dispatch", Vec::new()).await;
    let mut ready = harness.drive_ready(&agent.id).await;
    let expected = ready.revision;
    ready
        .work
        .schedule(AgentWorkStep::DispatchFirstMessage, rfc3339_at(harness.now));
    ready.revision += 1;
    harness.store.save_agent(expected, ready).await.unwrap();

    harness.run(&agent.id, harness.now).await;
    let completed = harness.stored(&agent.id).await;
    assert_eq!(completed.status, AgentStatus::Ready);
    assert_eq!(completed.work.step, AgentWorkStep::None);
    assert!(completed.conversation.first_message.is_none());
}

#[tokio::test(start_paused = true)]
async fn worker_retries_only_a_proven_first_message_pre_write_failure() {
    let mut harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    harness.now = SystemTime::now();
    let agent = harness
        .admit_with_first_message("retry-first", Vec::new(), Some("Run checks"))
        .await;
    for _ in 0..5 {
        harness.run(&agent.id, harness.now).await;
    }
    let dispatching = harness.stored(&agent.id).await;
    assert_eq!(dispatching.work.step, AgentWorkStep::DispatchFirstMessage);

    let closed = ConversationInputSender::spawn(1, |_| async {
        Err(AdapterError::Unavailable("retired writer".into()))
    });
    assert!(closed.send(ConversationInput::Cancel).await.is_err());
    tokio::task::yield_now().await;
    let _updates = harness.replace_input(&dispatching, dispatching.work.next_fence, closed);
    harness.run(&agent.id, harness.now).await;

    let retrying = harness.stored(&agent.id).await;
    assert_eq!(retrying.status, AgentStatus::Ready);
    assert_eq!(retrying.work.step, AgentWorkStep::DispatchFirstMessage);
    assert_eq!(retrying.work.attempts, 1);
    assert_eq!(
        retrying.conversation.first_message.as_ref().unwrap().state,
        FirstMessageDelivery::Queued
    );

    let working = ConversationInputSender::spawn(1, |_| async { Ok(()) });
    let _updates = harness.replace_input(&retrying, retrying.work.next_fence, working);
    let retry_at = SystemTime::UNIX_EPOCH
        + Duration::from_secs(layer2_core::seconds_from_rfc3339(&retrying.work.ready_at).unwrap());
    harness.run(&agent.id, retry_at).await;
    let accepted = harness.stored(&agent.id).await;
    assert_eq!(accepted.work.step, AgentWorkStep::None);
    assert_eq!(
        accepted.conversation.first_message.as_ref().unwrap().state,
        FirstMessageDelivery::WriteConfirmed
    );
}

#[tokio::test(start_paused = true)]
async fn worker_retires_ambiguous_first_message_work_into_explicit_recovery() {
    let mut harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    harness.now = SystemTime::now();
    let agent = harness
        .admit_with_first_message("ambiguous-first", Vec::new(), Some("Run migration"))
        .await;
    for _ in 0..5 {
        harness.run(&agent.id, harness.now).await;
    }
    let dispatching = harness.stored(&agent.id).await;
    let ambiguous = ConversationInputSender::spawn(1, |_| async {
        Err(AdapterError::Unavailable(
            "receipt lost after admission".into(),
        ))
    });
    let _updates = harness.replace_input(&dispatching, dispatching.work.next_fence, ambiguous);

    harness.run(&agent.id, harness.now).await;
    let recovery = harness.stored(&agent.id).await;
    assert_eq!(recovery.status, AgentStatus::Working);
    assert_eq!(recovery.work.step, AgentWorkStep::None);
    assert_eq!(
        recovery.conversation.first_message.as_ref().unwrap().state,
        FirstMessageDelivery::RecoveryRequired
    );
}

#[tokio::test(start_paused = true)]
async fn worker_does_not_resend_a_first_message_fenced_by_the_same_lease() {
    let mut harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    harness.now = SystemTime::now();
    let agent = harness
        .admit_with_first_message("in-progress-first", Vec::new(), Some("Keep running"))
        .await;
    for _ in 0..5 {
        harness.run(&agent.id, harness.now).await;
    }
    let stamp = rfc3339_at(harness.now);
    let claimed = harness
        .store
        .claim_agent_work(
            &agent.id,
            "agent-worker",
            stamp.clone(),
            rfc3339_at(harness.now + Duration::from_secs(30)),
        )
        .await
        .unwrap()
        .unwrap();
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    let expected_last_seq = claimed.conversation.last_seq;
    let mut fenced = claimed.clone();
    let fence_sequence = expected_last_seq + 1;
    let turn_sequence = fence_sequence + 1;
    fenced
        .conversation
        .first_message
        .as_mut()
        .unwrap()
        .fence_dispatch_for(turn_sequence, fence_sequence, fence, stamp.clone())
        .unwrap();
    fenced.conversation.active_turn = Some(turn_sequence);
    fenced
        .transition(AgentStatus::Working, stamp.clone(), None)
        .unwrap();
    let logs = vec![
        AgentLogKind::FirstMessageChanged {
            state: FirstMessageDelivery::DispatchFenced,
            recovery_generation: 1,
        },
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence,
            origin_key: Some("in-progress-first".into()),
        },
        AgentLogKind::OwnerMessage {
            content: "Keep running".into(),
        },
        AgentLogKind::StatusChanged {
            from: AgentStatus::Ready,
            to: AgentStatus::Working,
        },
    ];
    fenced.conversation.last_seq += u64::try_from(logs.len()).unwrap();
    let parent = layer2_core::sandbox::store::SandboxStore::get_sandbox(
        harness.store.as_ref(),
        &claimed.sandbox_id,
    )
    .await
    .unwrap()
    .unwrap();
    harness
        .store
        .commit_agent_conversation(AgentConversationMutation {
            settings_fence: None,
            command_claim: None,
            execution_fence: None,
            trigger_execution_fence: None,
            admission_guard: Some(layer2_core::sandbox::store::SandboxMutationGuard::capture(
                &parent,
            )),
            expected_agent_revision: claimed.revision,
            expected_last_seq,
            runtime_generation: claimed.conversation.runtime_generation.unwrap(),
            agent: fenced.clone(),
            appended_at: stamp,
            agent_logs: logs,
        })
        .await
        .unwrap();

    harness
        .worker
        .dispatch_first_message(claimed, harness.now)
        .await
        .unwrap();
    assert_eq!(harness.stored(&agent.id).await, fenced);
}

#[tokio::test]
async fn transient_skill_resolution_retries_then_materializes_exact_archive() {
    let reference = SkillRef {
        source: SkillSource::Ornn,
        id: SkillId::new("research"),
        version: Some("7.0".into()),
    };
    let material = AgentSkill {
        id: "research".into(),
        name: None,
        version: Some("7.0".into()),
        archive_sha256: "e".repeat(64),
        source_url: "https://ornn.example/skills/research/7.0.tar.gz".into(),
    };
    let skills = ScriptedSkills::new([
        Err(AgentSkillResolutionError::Unavailable(
            "temporary outage".into(),
        )),
        Ok(vec![material.clone()]),
    ]);
    let harness = Harness::new(skills.clone()).await;
    let agent = harness.admit("skills", vec![reference]).await;

    harness.run(&agent.id, harness.now).await;
    harness.run(&agent.id, harness.now).await;
    let retrying = harness.stored(&agent.id).await;
    assert_eq!(retrying.status, AgentStatus::Starting);
    assert_eq!(retrying.work.step, AgentWorkStep::MaterializeHome);
    assert_eq!(retrying.work.attempts, 1);
    assert!(matches!(
        harness
            .store
            .agent_log(&agent.id, 0, 20)
            .await
            .unwrap()
            .last()
            .map(|entry| &entry.kind),
        Some(AgentLogKind::WorkStepFailed {
            step: AgentWorkStep::MaterializeHome,
            failure,
        }) if failure.class == "agent_skill_source_unavailable"
    ));

    for _ in 0..3 {
        harness
            .run(&agent.id, harness.now + Duration::from_secs(2))
            .await;
    }
    let ready = harness.stored(&agent.id).await;
    assert_eq!(ready.status, AgentStatus::Ready);
    let snapshot = harness.backend.snapshot(&harness.runtime).await.unwrap();
    let home = &snapshot.homes[&AgentSlot::new(agent.id.to_string())];
    assert_eq!(home.skills, [material]);
    assert_eq!(skills.calls.load(Ordering::Relaxed), 2);
    assert_eq!(
        ready.work.resolved_skills.as_ref().unwrap()[0].version,
        "7.0"
    );
}

#[tokio::test]
async fn lost_home_reply_retries_with_a_rotated_locator_for_the_same_archive() {
    let reference = SkillRef {
        source: SkillSource::Ornn,
        id: SkillId::new("research"),
        version: Some("@stable".into()),
    };
    let first = AgentSkill {
        id: "research".into(),
        name: None,
        version: Some("7.0".into()),
        archive_sha256: "e".repeat(64),
        source_url: "https://ornn.example/temporary/a.tar.gz".into(),
    };
    let second = AgentSkill {
        source_url: "https://ornn.example/temporary/b.tar.gz".into(),
        ..first.clone()
    };
    let skills = ScriptedSkills::new([Ok(vec![first.clone()]), Ok(vec![second])]);
    let harness = Harness::new(skills.clone()).await;
    let agent = harness
        .admit("rotated-skill-locator", vec![reference])
        .await;

    harness.run(&agent.id, harness.now).await;
    harness
        .backend
        .lose_next_response(WireOperation::MaterializeAgentHome);
    harness.run(&agent.id, harness.now).await;
    let retrying = harness.stored(&agent.id).await;
    assert_eq!(retrying.work.step, AgentWorkStep::MaterializeHome);
    assert_eq!(retrying.work.attempts, 1);
    assert_eq!(retrying.work.cleanup_debt.len(), 1);

    for _ in 0..4 {
        harness
            .run(&agent.id, harness.now + Duration::from_secs(2))
            .await;
    }
    let ready = harness.stored(&agent.id).await;
    assert_eq!(ready.status, AgentStatus::Ready);
    assert!(ready.work.cleanup_debt.is_empty());
    assert_eq!(skills.calls.load(Ordering::Relaxed), 1);
    assert_eq!(ready.profile, agent.profile);
    assert_eq!(retrying.work.resolved_skills, ready.work.resolved_skills);
    let snapshot = harness.backend.snapshot(&harness.runtime).await.unwrap();
    assert_eq!(
        snapshot.homes[&AgentSlot::new(agent.id.to_string())].skills,
        [first]
    );
}

#[tokio::test]
async fn terminal_runtime_rejection_fails_agent_and_worktree_atomically() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let agent = harness.admit("rejected", Vec::new()).await;
    harness.backend.fail_next(
        WireOperation::AddWorktree,
        "quota_rejected",
        "runtime rejected the worktree",
    );

    harness.run(&agent.id, harness.now).await;
    let failed = harness.stored(&agent.id).await;
    let worktree = harness
        .store
        .get_worktree(&failed.worktree_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(failed.status, AgentStatus::Failed);
    assert_eq!(failed.work.step, AgentWorkStep::None);
    assert_eq!(worktree.status, WorktreeStatus::Failed);
    assert_eq!(
        failed
            .failure
            .as_ref()
            .map(|failure| failure.class.as_str()),
        Some("agent_runtime_rejected")
    );
    let logs = harness.store.agent_log(&agent.id, 0, 20).await.unwrap();
    assert!(matches!(
        logs.last().map(|entry| &entry.kind),
        Some(AgentLogKind::WorkStepFailed {
            step: AgentWorkStep::AddWorktree,
            ..
        })
    ));
}

#[tokio::test]
async fn stop_retains_resources_then_delete_removes_them_in_runtime_order() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let agent = harness.admit("cleanup", Vec::new()).await;
    let ready = harness.drive_ready(&agent.id).await;
    let target = Harness::target(&ready);
    let before = harness.backend.requests().await.len();
    harness
        .store
        .schedule_agent_stop(layer2_core::sandbox::store::AgentStopRequest {
            target: AgentControlTarget::from(&ready),
            reason: AgentStopReason::Owner,
            requested_at: rfc3339_at(harness.now + Duration::from_secs(5)),
        })
        .await
        .unwrap();
    for _ in 0..3 {
        harness
            .run(&agent.id, harness.now + Duration::from_secs(5))
            .await;
    }
    let stopped = harness.stored(&agent.id).await;
    assert_eq!(stopped.status, AgentStatus::Stopped);
    assert!(harness.registry.resolve(&target).is_none());
    let snapshot = harness.backend.snapshot(&harness.runtime).await.unwrap();
    assert!(snapshot
        .homes
        .contains_key(&AgentSlot::new(agent.id.to_string())));
    assert!(snapshot
        .worktrees
        .contains_key(&("project".into(), "wt-1".into())));

    harness
        .store
        .schedule_agent_delete(AgentDeleteRequest {
            target: AgentControlTarget::from(&stopped),
            requested_at: rfc3339_at(harness.now + Duration::from_secs(10)),
        })
        .await
        .unwrap();
    for _ in 0..4 {
        harness
            .run(&agent.id, harness.now + Duration::from_secs(10))
            .await;
    }
    let deleted = harness.stored(&agent.id).await;
    assert_eq!(deleted.status, AgentStatus::Deleted);
    assert_eq!(deleted.work.step, AgentWorkStep::None);
    let worktree = harness
        .store
        .get_worktree(&agent.worktree_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(worktree.status, WorktreeStatus::Removed);
    let snapshot = harness.backend.snapshot(&harness.runtime).await.unwrap();
    assert!(!snapshot
        .homes
        .contains_key(&AgentSlot::new(agent.id.to_string())));
    assert!(!snapshot
        .worktrees
        .contains_key(&("project".into(), "wt-1".into())));
    let operations = harness.backend.requests().await[before..]
        .iter()
        .map(|(_, request)| request.operation())
        .collect::<Vec<_>>();
    assert_eq!(
        operations,
        [
            WireOperation::CloseAgent,
            WireOperation::RemoveAgentHome,
            WireOperation::RemoveWorktree,
        ]
    );
    let logs = harness.store.agent_log(&agent.id, 0, 40).await.unwrap();
    let stopped_index = logs
        .iter()
        .position(|entry| matches!(entry.kind, AgentLogKind::Stopped { .. }))
        .unwrap();
    let deleted_index = logs
        .iter()
        .position(|entry| matches!(entry.kind, AgentLogKind::Deleted))
        .unwrap();
    assert!(stopped_index < deleted_index);
}

#[tokio::test]
async fn deletion_after_rejected_creation_compensates_durable_intent() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let agent = harness.admit("failed-cleanup", Vec::new()).await;
    harness.backend.fail_next(
        WireOperation::AddWorktree,
        "request_rejected",
        "worktree was never created",
    );
    harness.run(&agent.id, harness.now).await;
    let failed = harness.stored(&agent.id).await;
    let before = harness.backend.requests().await.len();
    harness
        .store
        .schedule_agent_delete(AgentDeleteRequest {
            target: AgentControlTarget::from(&failed),
            requested_at: rfc3339_at(harness.now + Duration::from_secs(5)),
        })
        .await
        .unwrap();

    for _ in 0..4 {
        harness
            .run(&agent.id, harness.now + Duration::from_secs(5))
            .await;
    }
    let deleted = harness.stored(&agent.id).await;
    assert_eq!(deleted.status, AgentStatus::Deleted);
    assert!(deleted.work.cleanup_debt.is_empty());
    assert_eq!(
        harness
            .store
            .get_worktree(&agent.worktree_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        WorktreeStatus::Removed
    );
    assert_eq!(
        harness.backend.requests().await[before..]
            .iter()
            .map(|(_, request)| request.operation())
            .collect::<Vec<_>>(),
        [WireOperation::RemoveWorktree]
    );
}

#[tokio::test]
async fn stop_and_delete_before_startup_skip_every_runtime_agent_resource() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let agent = harness.admit("early-stop", Vec::new()).await;
    let before = harness.backend.requests().await.len();
    harness
        .store
        .schedule_agent_stop(layer2_core::sandbox::store::AgentStopRequest {
            target: AgentControlTarget::from(&agent),
            reason: AgentStopReason::Owner,
            requested_at: rfc3339_at(harness.now + Duration::from_secs(5)),
        })
        .await
        .unwrap();
    for _ in 0..3 {
        harness
            .run(&agent.id, harness.now + Duration::from_secs(5))
            .await;
    }
    let stopped = harness.stored(&agent.id).await;
    assert_eq!(stopped.status, AgentStatus::Stopped);
    assert_eq!(
        harness
            .store
            .get_worktree(&agent.worktree_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        WorktreeStatus::Failed
    );
    harness
        .store
        .schedule_agent_delete(AgentDeleteRequest {
            target: AgentControlTarget::from(&stopped),
            requested_at: rfc3339_at(harness.now + Duration::from_secs(10)),
        })
        .await
        .unwrap();
    for _ in 0..4 {
        harness
            .run(&agent.id, harness.now + Duration::from_secs(10))
            .await;
    }
    assert_eq!(harness.stored(&agent.id).await.status, AgentStatus::Deleted);
    assert!(harness.backend.requests().await[before..].is_empty());
}

#[tokio::test]
async fn registry_rejects_stale_generations_fences_and_attachment_tokens() {
    let registry = Arc::new(LiveAgentRegistry::default());
    let recorder = Arc::new(NoopRecorder);
    let id = AgentId::generate();
    let (first_input, first_updates, _first_sender) = channels();
    let first = registry
        .attach(
            id.clone(),
            "conversation-1".into(),
            1,
            10,
            RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            first_input,
            first_updates,
            recorder.clone(),
        )
        .unwrap();
    registry.publish_and_activate(&first).unwrap();
    let first_target = LiveAgentTarget {
        agent_id: id.clone(),
        conversation_id: "conversation-1".into(),
        runtime_generation: 1,
    };
    assert!(registry.resolve(&first_target).is_some());
    let runtime_target = AgentRuntimeTarget {
        agent_id: id.clone(),
        conversation_id: "conversation-1".into(),
        runtime_generation: 1,
    };
    let connection = AgentRuntimeRegistry::resolve(registry.as_ref(), &runtime_target)
        .await
        .unwrap();
    assert!(connection
        .capabilities
        .supports(RuntimeCapability::Conversation));
    assert_eq!(
        AgentRuntimeRegistry::resolve(
            registry.as_ref(),
            &AgentRuntimeTarget {
                runtime_generation: 2,
                ..runtime_target.clone()
            }
        )
        .await
        .unwrap_err(),
        AgentRuntimeError::StaleGeneration
    );
    assert_eq!(
        AgentRuntimeRegistry::resolve(
            registry.as_ref(),
            &AgentRuntimeTarget {
                conversation_id: "conversation-other".into(),
                ..runtime_target.clone()
            }
        )
        .await
        .unwrap_err(),
        AgentRuntimeError::NotFound
    );
    assert_eq!(
        AgentRuntimeRegistry::resolve(
            registry.as_ref(),
            &AgentRuntimeTarget {
                agent_id: AgentId::generate(),
                ..runtime_target
            }
        )
        .await
        .unwrap_err(),
        AgentRuntimeError::NotFound
    );

    let (stale_input, stale_updates, _stale_sender) = channels();
    assert_eq!(
        registry
            .attach(
                id.clone(),
                "conversation-1".into(),
                1,
                9,
                RuntimeCapabilities::default(),
                stale_input,
                stale_updates,
                recorder.clone(),
            )
            .unwrap_err(),
        LiveAgentRegistryError::StaleGeneration
    );
    registry.fence(&id, Some(1), 9);
    assert!(registry.resolve(&first_target).is_some());

    let (newer_input, newer_updates, _newer_sender) = channels();
    let newer = registry
        .attach(
            id.clone(),
            "conversation-1".into(),
            1,
            11,
            RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            newer_input,
            newer_updates,
            recorder.clone(),
        )
        .unwrap();
    registry.rollback(&first);
    assert!(registry.target_is_attached(&first_target));
    registry.publish_and_activate(&first).unwrap();
    registry.publish_and_activate(&newer).unwrap();
    registry.fence(&id, Some(1), 10);
    assert!(registry.resolve(&first_target).is_some());
    registry.fence(&id, Some(1), 11);
    assert!(registry.resolve(&first_target).is_none());
    let (fenced_input, fenced_updates, _fenced_sender) = channels();
    assert_eq!(
        registry
            .attach(
                id.clone(),
                "conversation-1".into(),
                1,
                11,
                RuntimeCapabilities::default(),
                fenced_input,
                fenced_updates,
                recorder.clone(),
            )
            .unwrap_err(),
        LiveAgentRegistryError::StaleGeneration
    );

    let (next_input, next_updates, _next_sender) = channels();
    let next = registry
        .attach(
            id.clone(),
            "conversation-2".into(),
            2,
            12,
            RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            next_input,
            next_updates,
            recorder.clone(),
        )
        .unwrap();
    registry.publish_and_activate(&next).unwrap();
    let (old_input, old_updates, _old_sender) = channels();
    assert_eq!(
        registry
            .attach(
                id.clone(),
                "conversation-1".into(),
                1,
                13,
                RuntimeCapabilities::default(),
                old_input,
                old_updates,
                recorder,
            )
            .unwrap_err(),
        LiveAgentRegistryError::StaleGeneration
    );
    let final_target = AgentRuntimeTarget {
        agent_id: id.clone(),
        conversation_id: "conversation-2".into(),
        runtime_generation: 2,
    };
    assert!(
        AgentRuntimeRegistry::resolve(registry.as_ref(), &final_target)
            .await
            .is_ok()
    );
    AgentRuntimeRegistry::fence(registry.as_ref(), &id, 13)
        .await
        .unwrap();
    assert_eq!(
        AgentRuntimeRegistry::resolve(registry.as_ref(), &final_target)
            .await
            .unwrap_err(),
        AgentRuntimeError::NotFound
    );
    let (late_input, late_updates, _late_sender) = channels();
    assert_eq!(
        registry
            .attach(
                id,
                "conversation-2".into(),
                2,
                13,
                RuntimeCapabilities::default(),
                late_input,
                late_updates,
                Arc::new(NoopRecorder),
            )
            .unwrap_err(),
        LiveAgentRegistryError::StaleGeneration
    );
}

#[tokio::test]
async fn registry_orders_authority_lexicographically_by_runtime_generation() {
    let registry = Arc::new(LiveAgentRegistry::default());
    let recorder = Arc::new(NoopRecorder);
    let id = AgentId::generate();
    let (old_input, old_updates, _old_sender) = channels();
    let old = registry
        .attach(
            id.clone(),
            "conversation-stable".into(),
            7,
            90,
            RuntimeCapabilities::default(),
            old_input,
            old_updates,
            recorder.clone(),
        )
        .unwrap();
    registry.publish_and_activate(&old).unwrap();
    registry.fence(&id, Some(7), 100);

    let (new_input, new_updates, _new_sender) = channels();
    let replacement = registry
        .attach(
            id.clone(),
            "conversation-stable".into(),
            8,
            2,
            RuntimeCapabilities::default(),
            new_input,
            new_updates,
            recorder.clone(),
        )
        .unwrap();
    registry.publish_and_activate(&replacement).unwrap();
    let target = LiveAgentTarget {
        agent_id: id.clone(),
        conversation_id: "conversation-stable".into(),
        runtime_generation: 8,
    };
    assert!(registry.resolve(&target).is_some());

    registry.fence(&id, Some(7), 1_000);
    assert!(registry.resolve(&target).is_some());
    let (stale_input, stale_updates, _stale_sender) = channels();
    assert_eq!(
        registry
            .attach(
                id,
                "conversation-stable".into(),
                7,
                1_001,
                RuntimeCapabilities::default(),
                stale_input,
                stale_updates,
                recorder,
            )
            .unwrap_err(),
        LiveAgentRegistryError::StaleGeneration
    );
}

#[derive(Default)]
struct ConnectionLossProbe {
    seen: Mutex<Vec<LiveAgentAttachmentIdentity>>,
}

#[async_trait::async_trait]
impl LiveAgentConnectionLossHandler for ConnectionLossProbe {
    async fn connection_lost(&self, attachment: LiveAgentAttachmentIdentity) {
        self.seen.lock().unwrap().push(attachment);
    }
}

#[tokio::test]
async fn exact_stream_eof_removes_the_route_and_reports_complete_ownership() {
    let registry = Arc::new(LiveAgentRegistry::default());
    let probe = Arc::new(ConnectionLossProbe::default());
    assert!(registry.install_connection_loss_handler(probe.clone()));
    let id = AgentId::generate();
    let (input, updates, sender) = channels();
    let attachment = registry
        .attach(
            id.clone(),
            "conversation-eof".into(),
            3,
            9,
            RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            input,
            updates,
            Arc::new(NoopRecorder),
        )
        .unwrap();
    registry.publish_and_activate(&attachment).unwrap();
    let identity = attachment.identity();

    drop(sender);
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if !probe.seen.lock().unwrap().is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("EOF callback completed");

    assert_eq!(*probe.seen.lock().unwrap(), vec![identity]);
    let (stale_input, stale_updates, _stale_sender) = channels();
    assert_eq!(
        registry
            .attach(
                id.clone(),
                "conversation-eof".into(),
                3,
                9,
                RuntimeCapabilities::default(),
                stale_input,
                stale_updates,
                Arc::new(NoopRecorder),
            )
            .unwrap_err(),
        LiveAgentRegistryError::StaleGeneration
    );
    assert!(registry
        .resolve(&LiveAgentTarget {
            agent_id: id,
            conversation_id: "conversation-eof".into(),
            runtime_generation: 3,
        })
        .is_none());
}

#[tokio::test]
async fn stale_eof_cannot_remove_or_recover_a_replacement_route() {
    let registry = Arc::new(LiveAgentRegistry::default());
    let probe = Arc::new(ConnectionLossProbe::default());
    assert!(registry.install_connection_loss_handler(probe.clone()));
    let id = AgentId::generate();
    let (old_input, old_updates, old_sender) = channels();
    let old = registry
        .attach(
            id.clone(),
            "conversation-stable".into(),
            4,
            20,
            RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            old_input,
            old_updates,
            Arc::new(NoopRecorder),
        )
        .unwrap();
    registry.publish_and_activate(&old).unwrap();
    let stale_identity = old.identity();
    let (new_input, new_updates, _new_sender) = channels();
    let replacement = registry
        .attach(
            id.clone(),
            "conversation-stable".into(),
            4,
            21,
            RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            new_input,
            new_updates,
            Arc::new(NoopRecorder),
        )
        .unwrap();

    drop(old_sender);
    let target = LiveAgentTarget {
        agent_id: id.clone(),
        conversation_id: "conversation-stable".into(),
        runtime_generation: 4,
    };
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if registry.resolve(&target).is_none() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("stale route observed EOF");
    assert!(probe.seen.lock().unwrap().is_empty());
    registry.publish_and_activate(&replacement).unwrap();
    registry.connection_ended(stale_identity).await;

    assert!(probe.seen.lock().unwrap().is_empty());
    assert!(registry
        .resolve(&LiveAgentTarget {
            agent_id: id,
            ..target
        })
        .is_some());
}

#[tokio::test]
async fn durable_eof_schedules_and_republishes_the_same_conversation() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let admitted = harness.admit("eof-recovery", vec![]).await;
    let ready = harness.drive_ready(&admitted.id).await;
    let target = Harness::target(&ready);
    let attachment = harness
        .registry
        .attachment(&target, ready.work.next_fence - 1)
        .unwrap()
        .unwrap();

    harness
        .registry
        .connection_ended(attachment.identity())
        .await;
    let scheduled = harness.stored(&ready.id).await;
    assert_eq!(scheduled.status, AgentStatus::Ready);
    assert_eq!(scheduled.work.step, AgentWorkStep::PublishConnection);
    assert_eq!(scheduled.conversation, ready.conversation);

    harness
        .run(&ready.id, SystemTime::now() + Duration::from_secs(1))
        .await;
    let recovered = harness.stored(&ready.id).await;
    assert_eq!(recovered.work.step, AgentWorkStep::None);
    assert_eq!(
        recovered.conversation.conversation_id,
        ready.conversation.conversation_id
    );
    assert!(harness.registry.resolve(&target).is_some());
}

#[tokio::test]
async fn restart_reconciles_ready_working_and_waiting_approval_agents() {
    for status in [
        AgentStatus::Ready,
        AgentStatus::Working,
        AgentStatus::WaitingApproval,
    ] {
        let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        let admitted = harness.admit(status.name(), vec![]).await;
        let mut agent = harness.drive_ready(&admitted.id).await;
        let target = Harness::target(&agent);
        harness.registry.fence(
            &agent.id,
            agent.conversation.runtime_generation,
            agent.work.next_fence - 1,
        );
        if status != AgentStatus::Ready {
            let expected = agent.revision;
            agent.status = status;
            agent.conversation.active_turn = Some(41);
            agent.revision += 1;
            harness
                .store
                .save_agent(expected, agent.clone())
                .await
                .unwrap();
        }

        assert_eq!(
            harness
                .worker
                .reconcile_connections(harness.now)
                .await
                .unwrap(),
            1
        );
        let scheduled = harness.stored(&agent.id).await;
        assert_eq!(scheduled.status, status);
        assert_eq!(scheduled.work.step, AgentWorkStep::PublishConnection);
        assert_eq!(
            scheduled.conversation.active_turn,
            agent.conversation.active_turn
        );
        harness.run(&agent.id, harness.now).await;
        let recovered = harness.stored(&agent.id).await;
        assert_eq!(recovered.status, status);
        assert_eq!(recovered.work.step, AgentWorkStep::None);
        assert_eq!(
            recovered.conversation.active_turn,
            agent.conversation.active_turn
        );
        assert!(harness.registry.resolve(&target).is_some());
    }
}

struct NoopRecorder;

#[async_trait::async_trait]
impl layer2_core::sandbox::AgentUpdateRecorder for NoopRecorder {
    async fn record_update(
        &self,
        _id: &AgentId,
        _runtime_generation: u64,
        _assembly: &mut layer2_core::sandbox::AgentMessageAssembly,
        _update: ConversationUpdate,
        _appended_at: String,
    ) -> Result<
        layer2_core::sandbox::AgentUpdateOutcome,
        layer2_core::sandbox::AgentConversationError,
    > {
        Ok(layer2_core::sandbox::AgentUpdateOutcome {
            appended: Vec::new(),
            turn_closed: false,
        })
    }
}

fn channels() -> (
    ConversationInputSender,
    mpsc::Receiver<ConversationUpdate>,
    mpsc::Sender<ConversationUpdate>,
) {
    let input = ConversationInputSender::spawn(4, |_| async { Ok(()) });
    let (updates, receiver) = mpsc::channel(4);
    (input, receiver, updates)
}

#[tokio::test]
async fn staged_registry_replacement_preserves_active_connection_until_promotion() {
    let registry = Arc::new(LiveAgentRegistry::default());
    let recorder = Arc::new(NoopRecorder);
    let id = AgentId::generate();
    let (active_input, active_updates, _active_sender) = channels();
    let active = registry
        .attach(
            id.clone(),
            "conversation-1".into(),
            1,
            10,
            RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            active_input,
            active_updates,
            recorder.clone(),
        )
        .unwrap();
    registry.publish_and_activate(&active).unwrap();
    let active_target = LiveAgentTarget {
        agent_id: id.clone(),
        conversation_id: "conversation-1".into(),
        runtime_generation: 1,
    };

    let (candidate_input, candidate_updates, _candidate_sender) = channels();
    let candidate = registry
        .attach(
            id.clone(),
            "conversation-2".into(),
            2,
            11,
            RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            candidate_input,
            candidate_updates,
            recorder,
        )
        .unwrap();
    let candidate_target = LiveAgentTarget {
        agent_id: id,
        conversation_id: "conversation-2".into(),
        runtime_generation: 2,
    };
    assert!(registry.resolve(&active_target).is_some());
    assert!(registry.resolve(&candidate_target).is_none());

    registry.rollback(&candidate);
    assert!(registry.resolve(&active_target).is_some());
    assert_eq!(
        registry.publish_and_activate(&candidate).unwrap_err(),
        LiveAgentRegistryError::NotAttached
    );
}
