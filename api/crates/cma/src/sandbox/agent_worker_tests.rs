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

pub(crate) mod context_overlap_fixture;
mod fixture_store;
use fixture_store::FixtureStore;
mod active_recovery_tests;
mod cleanup_contention_tests;
mod cleanup_scheduling_tests;
mod connection_contention_tests;
mod connection_failure_tests;
mod create_completion_tests;
mod first_message_contention_tests;
mod first_message_publication_tests;
mod ledger_hold_tests;
mod lifecycle_connection_tests;
mod lifecycle_failure_tests;
mod open_publication_recovery_tests;
mod ownership_pacing_tests;
mod prepared_pair_tests;
mod retired_runtime_tests;
mod runtime_attachment_tests;
mod sdk_conformance_tests;
mod settings_replay_tests;
mod sign_in_tests;
mod skill_home;
mod skill_material_tests;
mod skill_publication_contention_tests;
mod staged_connection_tests;
mod startup_failure_tests;
mod startup_hold_tests;
mod startup_publication_tests;
mod startup_scheduling_tests;
mod warm_handoff_tests;
mod work_retry_policy_tests;
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

    async fn with_upgrade_support(skills: Arc<dyn AgentSkillResolver>) -> Self {
        Self::with_store_services_home(
            Arc::new(InMemorySandboxStore::default()),
            skills,
            1,
            None,
            true,
        )
        .await
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
        Self::with_store_services_home(store, skills, generation, services, false).await
    }
    async fn with_store_services_home(
        store: Arc<S>,
        skills: Arc<dyn AgentSkillResolver>,
        generation: u64,
        services: Option<Vec<ServiceId>>,
        persistent_home: bool,
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
        let adapter = if persistent_home {
            skill_home::attach(adapter, &mut sandbox)
        } else {
            adapter
        };
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
                    github_route: None,
                },
            )
            .await
            .unwrap();
        sandbox.runtime = Some(RuntimeBinding {
            boot_id: if persistent_home {
                Some(adapter.runtime_status(&runtime).await.unwrap().boot_id)
            } else {
                None
            },
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
        let started = tokio::time::Instant::now();
        let mut at = now;
        for attempt in 1..=250 {
            if self.worker.run_one(id, at).await.unwrap() {
                return;
            }
            if attempt == 250 {
                let agent = self.stored(id).await;
                panic!(
                    "agent {id:?} did no work: step={:?} ready_at={:?} lease={:?} now={now:?} elapsed={:?} attempts={attempt}",
                    agent.work.step,
                    agent.work.ready_at,
                    agent.work.lease,
                    started.elapsed()
                );
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
            at = now + started.elapsed();
        }
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

mod cleanup_cases;
mod connection_recovery_cases;
mod first_message_cases;
mod fixture_models;
mod registry_cases;
mod skill_resolution_cases;
mod startup_cases;
use fixture_models::{agent_profile, sandbox, sandbox_spec, workspace, DEPLOYMENT_ID};
