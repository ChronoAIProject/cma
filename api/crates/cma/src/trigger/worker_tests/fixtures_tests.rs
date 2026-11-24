use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use layer2_core::profiles::{
    content_digest, AgentProfileContent, AgentProfileRevision, AgentProfileSnapshot, AgentProvider,
    ModelAccess, PermissionLevel, SandboxProfileContent, SandboxProfileRef, SandboxProfileRevision,
    SandboxProfileSnapshot, ThinkingEffort, WorkspaceLocation, WorkspaceProfileContent,
    WorkspaceProfileRef, WorkspaceProfileRevision, WorkspaceProfileSnapshot,
};
use layer2_core::sandbox::{
    Agent, AgentOrigin, AgentStatus, AgentStopReason, AgentWorkState, AgentWorkStep,
    ConversationState, FirstMessageRecord, RuntimeBinding, Sandbox, SandboxCredentialState,
    SandboxLimits, SandboxOrigin, SandboxSignInState, SandboxStatus, SandboxWorkState,
    SandboxWorkStep, TypedFailure, Workspace, WorkspaceOrigin, WorkspaceSource, WorkspaceStatus,
    WorkspaceWorkState, WorkspaceWorkStep,
};
use layer2_core::trigger_domain::{
    AgentReusePolicy, ButtonLabel, Launch, LaunchAdmission, LaunchAdmissionResult,
    LaunchAuthorityClass, LaunchCommit, LaunchConfirmation, LaunchConsentSurface, LaunchEvent,
    LaunchResolution, LaunchSeed, LaunchStore, LaunchStoreError, LaunchTrustManifest, Presentation,
    PublicTriggerId, RequirementsSnapshot, ResolvedResource, ResolvedTriggerProfiles, ReusePolicy,
    ReviewRequirements, SandboxReusePolicy, ServiceRequirement, ServiceRequirementState, Theme,
    TriggerConsent, TriggerContent, TriggerId, TriggerRevision, TriggerTarget,
    WorkspaceReusePolicy,
};
use layer2_core::{
    rfc3339_at, AgentId, Capability, LaunchId, RuntimeCapabilities, RuntimeCapability, SandboxId,
    ServiceId, SubjectId, WorkspaceId, WorktreeId,
};

use super::super::worker::{
    LaunchResourceDriver, LaunchResourceEffect, LaunchResourceError, LaunchWorker,
    LaunchWorkerConfig,
};

const BASE_SECONDS: u64 = 1_780_000_000;

pub(super) fn time(minute: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(BASE_SECONDS + minute * 60)
}

pub(super) fn stamp(minute: u64) -> String {
    rfc3339_at(time(minute))
}

#[derive(Default)]
pub(super) struct TestLaunchStore {
    launches: Mutex<HashMap<LaunchId, Launch>>,
    fail_claim_once: Mutex<HashSet<LaunchId>>,
}

impl TestLaunchStore {
    pub(super) fn insert(&self, launch: Launch) {
        launch.validate().unwrap();
        self.launches
            .lock()
            .unwrap()
            .insert(launch.id.clone(), launch);
    }

    pub(super) fn stored(&self, id: &LaunchId) -> Launch {
        self.launches.lock().unwrap().get(id).unwrap().clone()
    }

    pub(super) fn fail_next_claim(&self, id: LaunchId) {
        self.fail_claim_once.lock().unwrap().insert(id);
    }
}

#[async_trait::async_trait]
impl LaunchStore for TestLaunchStore {
    async fn admit_launch(
        &self,
        _admission: LaunchAdmission,
    ) -> Result<LaunchAdmissionResult, LaunchStoreError> {
        Err(LaunchStoreError::Unavailable(
            "worker test admission is unsupported".into(),
        ))
    }

    async fn launch_by_command(
        &self,
        _owner: &SubjectId,
        _public_id: &PublicTriggerId,
        _command_key_hash: &str,
    ) -> Result<Option<Launch>, LaunchStoreError> {
        Ok(None)
    }

    async fn launch(
        &self,
        id: &LaunchId,
        owner: &SubjectId,
    ) -> Result<Option<Launch>, LaunchStoreError> {
        Ok(self
            .launches
            .lock()
            .unwrap()
            .get(id)
            .filter(|launch| &launch.owner == owner)
            .cloned())
    }

    async fn list_launches(
        &self,
        _owner: &SubjectId,
        _page: &layer2_core::trigger_domain::LaunchPageRequest,
    ) -> Result<layer2_core::trigger_domain::LaunchPage, LaunchStoreError> {
        Err(LaunchStoreError::Unavailable(
            "worker test listing is unsupported".into(),
        ))
    }

    async fn placement_stats(
        &self,
        _trigger_id: &TriggerId,
    ) -> Result<Vec<layer2_core::trigger_domain::LaunchPlacementStats>, LaunchStoreError> {
        Err(LaunchStoreError::Unavailable(
            "worker test placement stats are unsupported".into(),
        ))
    }

    async fn commit_launch(&self, commit: LaunchCommit) -> Result<Launch, LaunchStoreError> {
        let mut launches = self.launches.lock().unwrap();
        let current = launches
            .get(&commit.launch.id)
            .ok_or(LaunchStoreError::NotFound)?;
        if current.revision != commit.expected_revision {
            return Err(LaunchStoreError::Conflict {
                expected: commit.expected_revision,
                actual: current.revision,
            });
        }
        if commit.launch.revision != current.revision.saturating_add(1)
            || commit.launch.event_head != current.event_head
        {
            return Err(LaunchStoreError::Corrupt(
                "worker test commit crossed its aggregate boundary".into(),
            ));
        }
        let mut next = commit.launch;
        next.event_head = next
            .event_head
            .checked_add(u64::try_from(commit.events.len()).unwrap())
            .ok_or_else(|| LaunchStoreError::Corrupt("event sequence overflow".into()))?;
        next.validate()?;
        launches.insert(next.id.clone(), next.clone());
        Ok(next)
    }

    async fn launch_events(
        &self,
        _id: &LaunchId,
        _owner: &SubjectId,
        _after: u64,
        _limit: u16,
    ) -> Result<Vec<LaunchEvent>, LaunchStoreError> {
        Ok(Vec::new())
    }

    async fn due_launches(&self, now: &str, limit: u16) -> Result<Vec<LaunchId>, LaunchStoreError> {
        let mut due = self
            .launches
            .lock()
            .unwrap()
            .values()
            .filter(|launch| launch.is_worker_due(now))
            .map(|launch| (launch.work.ready_at.clone(), launch.id.clone()))
            .collect::<Vec<_>>();
        due.sort();
        due.truncate(usize::from(limit));
        Ok(due.into_iter().map(|(_, id)| id).collect())
    }

    async fn claim_launch(
        &self,
        id: &LaunchId,
        holder: &str,
        now: String,
        lease_expires_at: String,
    ) -> Result<Option<Launch>, LaunchStoreError> {
        if self.fail_claim_once.lock().unwrap().remove(id) {
            return Err(LaunchStoreError::Unavailable(
                "simulated isolated claim failure".into(),
            ));
        }
        let mut launches = self.launches.lock().unwrap();
        let Some(current) = launches.get(id).cloned() else {
            return Ok(None);
        };
        if !current.is_worker_due(&now) {
            return Ok(None);
        }
        let mut claimed = current;
        claimed
            .work
            .claim(holder.into(), &now, lease_expires_at)
            .map_err(LaunchStoreError::Validation)?;
        claimed.revision = claimed
            .revision
            .checked_add(1)
            .ok_or_else(|| LaunchStoreError::Corrupt("revision overflow".into()))?;
        claimed.updated_at = now;
        claimed.validate()?;
        launches.insert(id.clone(), claimed.clone());
        Ok(Some(claimed))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EffectCall {
    pub kind: layer2_core::trigger_domain::LaunchEffectKind,
    pub subject: SubjectId,
    pub capabilities: Vec<Capability>,
    pub authority: Option<Capability>,
    pub launch_id: String,
    pub profile_id: Option<String>,
    pub confirmed_digest: Option<String>,
    pub first_message: Option<String>,
    pub result_id: String,
}

pub(super) struct FakeDriver {
    trigger_id: TriggerId,
    revisions: Mutex<HashMap<String, TriggerRevision>>,
    contexts: Mutex<Vec<ResolvedTriggerProfiles>>,
    paused: AtomicBool,
    requirements: Mutex<HashMap<ServiceId, ServiceRequirementState>>,
    sandboxes: Mutex<HashMap<SandboxId, Sandbox>>,
    workspaces: Mutex<HashMap<WorkspaceId, Workspace>>,
    agents: Mutex<HashMap<AgentId, Agent>>,
    calls: Mutex<Vec<EffectCall>>,
    trigger_queries: Mutex<Vec<(TriggerId, String)>>,
    profile_queries: Mutex<Vec<TriggerTarget>>,
    crash_once: Mutex<Option<layer2_core::trigger_domain::LaunchEffectKind>>,
    install_target_workspace: AtomicBool,
    invalid_result: Mutex<Option<(layer2_core::trigger_domain::LaunchEffectKind, String)>>,
    checkpoint_once: Mutex<Option<layer2_core::trigger_domain::LaunchEffectKind>>,
}

impl FakeDriver {
    pub(super) fn new(inputs: FrozenInputs) -> Arc<Self> {
        let driver = Arc::new(Self {
            trigger_id: inputs.trigger_id.clone(),
            revisions: Mutex::new(HashMap::new()),
            contexts: Mutex::new(Vec::new()),
            paused: AtomicBool::new(false),
            requirements: Mutex::new(HashMap::new()),
            sandboxes: Mutex::new(HashMap::new()),
            workspaces: Mutex::new(HashMap::new()),
            agents: Mutex::new(HashMap::new()),
            calls: Mutex::new(Vec::new()),
            trigger_queries: Mutex::new(Vec::new()),
            profile_queries: Mutex::new(Vec::new()),
            crash_once: Mutex::new(None),
            install_target_workspace: AtomicBool::new(false),
            invalid_result: Mutex::new(None),
            checkpoint_once: Mutex::new(None),
        });
        driver.install_context(inputs);
        driver
    }

    pub(super) fn install_context(&self, inputs: FrozenInputs) {
        self.revisions
            .lock()
            .unwrap()
            .insert(inputs.revision.digest.clone(), inputs.revision);
        self.contexts.lock().unwrap().push(inputs.profiles);
    }

    pub(super) fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::SeqCst);
    }

    pub(super) fn set_requirement(&self, service: ServiceId, state: ServiceRequirementState) {
        self.requirements.lock().unwrap().insert(service, state);
    }

    pub(super) fn set_crash_once(&self, kind: layer2_core::trigger_domain::LaunchEffectKind) {
        *self.crash_once.lock().unwrap() = Some(kind);
    }

    pub(super) fn checkpoint_before_next_creation(
        &self,
        kind: layer2_core::trigger_domain::LaunchEffectKind,
    ) {
        *self.checkpoint_once.lock().unwrap() = Some(kind);
    }

    pub(super) fn set_invalid_result(
        &self,
        kind: layer2_core::trigger_domain::LaunchEffectKind,
        result: impl Into<String>,
    ) {
        *self.invalid_result.lock().unwrap() = Some((kind, result.into()));
    }

    pub(super) fn enable_installed_target_workspace(&self) {
        self.install_target_workspace.store(true, Ordering::SeqCst);
    }

    pub(super) fn put_sandbox(&self, sandbox: Sandbox) {
        self.sandboxes
            .lock()
            .unwrap()
            .insert(sandbox.id.clone(), sandbox);
    }

    pub(super) fn put_workspace(&self, workspace: Workspace) {
        self.workspaces
            .lock()
            .unwrap()
            .insert(workspace.id.clone(), workspace);
    }

    pub(super) fn put_agent(&self, agent: Agent) {
        self.agents.lock().unwrap().insert(agent.id.clone(), agent);
    }

    pub(super) fn mutate_sandbox(&self, id: &SandboxId, change: impl FnOnce(&mut Sandbox)) {
        change(self.sandboxes.lock().unwrap().get_mut(id).unwrap());
    }

    pub(super) fn mutate_agent(&self, id: &AgentId, change: impl FnOnce(&mut Agent)) {
        change(self.agents.lock().unwrap().get_mut(id).unwrap());
    }

    pub(super) fn sandbox_value(&self, id: &SandboxId) -> Sandbox {
        self.sandboxes.lock().unwrap().get(id).unwrap().clone()
    }

    pub(super) fn agent_value(&self, id: &AgentId) -> Agent {
        self.agents.lock().unwrap().get(id).unwrap().clone()
    }

    pub(super) fn calls(&self) -> Vec<EffectCall> {
        self.calls.lock().unwrap().clone()
    }

    pub(super) fn trigger_queries(&self) -> Vec<(TriggerId, String)> {
        self.trigger_queries.lock().unwrap().clone()
    }

    pub(super) fn profile_queries(&self) -> Vec<TriggerTarget> {
        self.profile_queries.lock().unwrap().clone()
    }

    pub(super) fn resource_counts(&self) -> (usize, usize, usize) {
        (
            self.sandboxes.lock().unwrap().len(),
            self.workspaces.lock().unwrap().len(),
            self.agents.lock().unwrap().len(),
        )
    }

    pub(super) fn complete_first_message(&self, id: &AgentId, minute: u64) {
        self.mutate_agent(id, |agent| {
            let message = agent.conversation.first_message.as_mut().unwrap();
            message.fence_dispatch(1, 1, stamp(minute)).unwrap();
            message.mark_write_confirmed(stamp(minute)).unwrap();
            message.complete(stamp(minute)).unwrap();
            agent.updated_at = stamp(minute);
            agent.revision += 1;
        });
    }

    pub(super) fn stop_agent(&self, id: &AgentId, minute: u64) {
        self.mutate_agent(id, |agent| {
            agent
                .transition(
                    AgentStatus::Stopped,
                    stamp(minute),
                    Some(AgentStopReason::Owner),
                )
                .unwrap();
        });
    }

    fn context_for_sandbox(&self, profile: &SandboxProfileRef) -> Option<ResolvedTriggerProfiles> {
        self.contexts
            .lock()
            .unwrap()
            .iter()
            .find(|context| {
                context.sandbox.profile_id == profile.id
                    && context.sandbox.revision == profile.revision
            })
            .cloned()
    }

    fn context_for_workspace(
        &self,
        profile: &WorkspaceProfileRef,
    ) -> Option<ResolvedTriggerProfiles> {
        self.contexts
            .lock()
            .unwrap()
            .iter()
            .find(|context| {
                context.workspace.profile_id == profile.id
                    && context.workspace.revision == profile.revision
            })
            .cloned()
    }

    fn context_for_agent(
        &self,
        profile: &layer2_core::profiles::AgentProfileRef,
    ) -> Option<ResolvedTriggerProfiles> {
        self.contexts
            .lock()
            .unwrap()
            .iter()
            .find(|context| {
                context.agent.profile_id == profile.id && context.agent.revision == profile.revision
            })
            .cloned()
    }

    fn record_result(&self, mut call: EffectCall) -> Result<String, LaunchResourceError> {
        let invalid_result = self.invalid_result.lock().unwrap().take();
        if let Some((kind, invalid)) = invalid_result {
            if kind == call.kind {
                call.result_id = invalid;
            } else {
                *self.invalid_result.lock().unwrap() = Some((kind, invalid));
            }
        }
        let result = call.result_id.clone();
        let kind = call.kind;
        self.calls.lock().unwrap().push(call);
        if self.crash_once.lock().unwrap().take() == Some(kind) {
            panic!("simulated crash after external mutation");
        }
        Ok(result)
    }

    fn missing_profile() -> LaunchResourceError {
        LaunchResourceError::terminal("test_profile_missing", "Frozen test profile is missing")
    }
}

#[async_trait::async_trait]
impl LaunchResourceDriver for FakeDriver {
    async fn trigger_revision(
        &self,
        trigger_id: &TriggerId,
        digest: &str,
    ) -> Result<Option<TriggerRevision>, LaunchResourceError> {
        self.trigger_queries
            .lock()
            .unwrap()
            .push((trigger_id.clone(), digest.into()));
        if trigger_id != &self.trigger_id {
            return Ok(None);
        }
        Ok(self.revisions.lock().unwrap().get(digest).cloned())
    }

    async fn frozen_profiles(
        &self,
        target: &TriggerTarget,
    ) -> Result<ResolvedTriggerProfiles, LaunchResourceError> {
        self.profile_queries.lock().unwrap().push(target.clone());
        self.contexts
            .lock()
            .unwrap()
            .iter()
            .find(|context| {
                context.sandbox.profile_id == target.sandbox_profile.id
                    && context.sandbox.revision == target.sandbox_profile.revision
                    && context.workspace.profile_id == target.workspace_profile.id
                    && context.workspace.revision == target.workspace_profile.revision
                    && context.agent.profile_id == target.agent_profile.id
                    && context.agent.revision == target.agent_profile.revision
            })
            .cloned()
            .ok_or_else(Self::missing_profile)
    }

    async fn orchestration_paused(&self) -> Result<bool, LaunchResourceError> {
        Ok(self.paused.load(Ordering::SeqCst))
    }

    async fn requirements(
        &self,
        _owner: &SubjectId,
        services: &[ServiceId],
    ) -> Result<Vec<ServiceRequirement>, LaunchResourceError> {
        let states = self.requirements.lock().unwrap();
        Ok(services
            .iter()
            .map(|service| ServiceRequirement {
                service: service.clone(),
                state: states
                    .get(service)
                    .copied()
                    .unwrap_or(ServiceRequirementState::Satisfied),
            })
            .collect())
    }

    async fn sandbox(&self, id: &SandboxId) -> Result<Option<Sandbox>, LaunchResourceError> {
        Ok(self.sandboxes.lock().unwrap().get(id).cloned())
    }

    async fn workspace(&self, id: &WorkspaceId) -> Result<Option<Workspace>, LaunchResourceError> {
        Ok(self.workspaces.lock().unwrap().get(id).cloned())
    }

    async fn agent(&self, id: &AgentId) -> Result<Option<Agent>, LaunchResourceError> {
        Ok(self.agents.lock().unwrap().get(id).cloned())
    }

    async fn matching_workspace(
        &self,
        owner: &SubjectId,
        sandbox_id: &SandboxId,
        profile_id: &layer2_core::WorkspaceProfileId,
    ) -> Result<Option<Workspace>, LaunchResourceError> {
        Ok(self
            .workspaces
            .lock()
            .unwrap()
            .values()
            .find(|workspace| {
                &workspace.owner == owner
                    && &workspace.sandbox_id == sandbox_id
                    && &workspace.profile.id == profile_id
            })
            .cloned())
    }

    async fn apply(&self, effect: LaunchResourceEffect) -> Result<String, LaunchResourceError> {
        if self.checkpoint_once.lock().unwrap().take() == Some(effect.kind()) {
            let sandbox_id = match &effect {
                LaunchResourceEffect::CreateWorkspace { sandbox_id, .. } => sandbox_id.clone(),
                LaunchResourceEffect::CreateAgent { workspace_id, .. } => self
                    .workspaces
                    .lock()
                    .unwrap()
                    .get(workspace_id)
                    .unwrap()
                    .sandbox_id
                    .clone(),
                _ => panic!("checkpoint test hook only supports child creation"),
            };
            self.mutate_sandbox(&sandbox_id, |sandbox| {
                sandbox.begin_checkpoint_capture(stamp(0)).unwrap()
            });
            return Err(LaunchResourceError::retryable(
                "launch_sandbox_not_ready",
                "Sandbox is checkpointing",
            ));
        }
        match effect {
            LaunchResourceEffect::CreateSandbox {
                caller,
                authority,
                request,
            } => {
                let launch_id = match &request.origin {
                    SandboxOrigin::Trigger { launch_id } => launch_id.clone(),
                    SandboxOrigin::Manual | SandboxOrigin::Lifecycle { .. } => {
                        return Err(Self::missing_profile())
                    }
                };
                let context = self
                    .context_for_sandbox(&request.profile)
                    .ok_or_else(Self::missing_profile)?;
                let existing = self
                    .sandboxes
                    .lock()
                    .unwrap()
                    .values()
                    .find(|sandbox| {
                        matches!(&sandbox.origin, SandboxOrigin::Trigger { launch_id: stored } if stored == &launch_id)
                    })
                    .map(|sandbox| sandbox.id.clone());
                let id = existing.unwrap_or_else(|| SandboxId(format!("sbx_{launch_id}")));
                if !self.sandboxes.lock().unwrap().contains_key(&id) {
                    self.put_sandbox(sandbox_resource(
                        &context,
                        caller.subject.clone(),
                        id.clone(),
                        SandboxStatus::Active,
                        request.origin,
                    ));
                }
                if self.install_target_workspace.load(Ordering::SeqCst) {
                    if let Some(index) =
                        context
                            .sandbox
                            .content
                            .workspace_profiles
                            .iter()
                            .position(|reference| {
                                reference.id == context.workspace.profile_id
                                    && reference.revision == context.workspace.revision
                            })
                    {
                        let workspace_id = WorkspaceId(format!("wks_installed_{launch_id}"));
                        if !self.workspaces.lock().unwrap().contains_key(&workspace_id) {
                            self.put_workspace(workspace_resource(
                                &context,
                                caller.subject.clone(),
                                id.clone(),
                                workspace_id,
                                WorkspaceStatus::Ready,
                                WorkspaceOrigin::SandboxProfile {
                                    index: u16::try_from(index).unwrap(),
                                },
                            ));
                        }
                    }
                }
                self.record_result(EffectCall {
                    kind: layer2_core::trigger_domain::LaunchEffectKind::CreateSandbox,
                    subject: caller.subject,
                    capabilities: caller.authorization.iter().collect(),
                    authority: Some(authority),
                    launch_id,
                    profile_id: Some(request.profile.id.to_string()),
                    confirmed_digest: request.confirmed_digest,
                    first_message: None,
                    result_id: id.to_string(),
                })
            }
            LaunchResourceEffect::ReviveSandbox {
                caller,
                sandbox_id,
                launch_id,
                reused: _,
            } => {
                self.mutate_sandbox(&sandbox_id, |sandbox| {
                    if sandbox.status == SandboxStatus::Paused {
                        sandbox.status = SandboxStatus::Reviving;
                        sandbox.updated_at = stamp(1);
                    } else if sandbox.status == SandboxStatus::Pausing {
                        sandbox.request_pause_cancellation(stamp(1)).unwrap();
                    }
                });
                self.record_result(EffectCall {
                    kind: layer2_core::trigger_domain::LaunchEffectKind::ReviveSandbox,
                    subject: caller.subject,
                    capabilities: caller.authorization.iter().collect(),
                    authority: None,
                    launch_id,
                    profile_id: None,
                    confirmed_digest: None,
                    first_message: None,
                    result_id: sandbox_id.to_string(),
                })
            }
            LaunchResourceEffect::RepairSandboxServiceGrant {
                caller,
                sandbox_id,
                launch_id,
            } => {
                self.mutate_sandbox(&sandbox_id, |sandbox| {
                    sandbox.status = SandboxStatus::Active;
                    sandbox.failure = None;
                    sandbox.updated_at = stamp(1);
                });
                self.record_result(EffectCall {
                    kind: layer2_core::trigger_domain::LaunchEffectKind::RepairSandboxServiceGrant,
                    subject: caller.subject,
                    capabilities: caller.authorization.iter().collect(),
                    authority: None,
                    launch_id,
                    profile_id: None,
                    confirmed_digest: None,
                    first_message: None,
                    result_id: sandbox_id.to_string(),
                })
            }
            LaunchResourceEffect::CreateWorkspace {
                caller,
                authority,
                sandbox_id,
                request,
                launch_id,
            } => {
                let context = self
                    .context_for_workspace(&request.profile)
                    .ok_or_else(Self::missing_profile)?;
                let existing = self
                    .workspaces
                    .lock()
                    .unwrap()
                    .values()
                    .find(|workspace| {
                        matches!(&workspace.origin, WorkspaceOrigin::Trigger { launch_id: stored } if stored == &launch_id)
                    })
                    .map(|workspace| workspace.id.clone());
                let id = existing.unwrap_or_else(|| WorkspaceId(format!("wks_{launch_id}")));
                if !self.workspaces.lock().unwrap().contains_key(&id) {
                    self.put_workspace(workspace_resource(
                        &context,
                        caller.subject.clone(),
                        sandbox_id,
                        id.clone(),
                        WorkspaceStatus::Ready,
                        WorkspaceOrigin::Trigger {
                            launch_id: launch_id.clone(),
                        },
                    ));
                }
                self.record_result(EffectCall {
                    kind: layer2_core::trigger_domain::LaunchEffectKind::CreateWorkspace,
                    subject: caller.subject,
                    capabilities: caller.authorization.iter().collect(),
                    authority: Some(authority),
                    launch_id,
                    profile_id: Some(request.profile.id.to_string()),
                    confirmed_digest: request.confirmed_digest,
                    first_message: None,
                    result_id: id.to_string(),
                })
            }
            LaunchResourceEffect::CreateAgent {
                caller,
                authority,
                workspace_id,
                request,
                reuse_existing: _,
                initiation: _,
            } => {
                let (trigger_id, launch_id) = match &request.origin {
                    AgentOrigin::Trigger {
                        trigger_id,
                        launch_id,
                    } => (trigger_id.clone(), launch_id.clone()),
                    AgentOrigin::Manual
                    | AgentOrigin::Lifecycle { .. }
                    | AgentOrigin::WorkspaceProfile { .. } => return Err(Self::missing_profile()),
                };
                let context = self
                    .context_for_agent(&request.profile)
                    .ok_or_else(Self::missing_profile)?;
                let sandbox_id = self
                    .workspaces
                    .lock()
                    .unwrap()
                    .get(&workspace_id)
                    .map(|workspace| workspace.sandbox_id.clone())
                    .ok_or_else(Self::missing_profile)?;
                let existing = self
                    .agents
                    .lock()
                    .unwrap()
                    .values()
                    .find(|agent| {
                        matches!(&agent.origin, AgentOrigin::Trigger { launch_id: stored, .. } if stored == &launch_id)
                    })
                    .map(|agent| agent.id.clone());
                let id = existing.unwrap_or_else(|| AgentId(format!("agt_{launch_id}")));
                if !self.agents.lock().unwrap().contains_key(&id) {
                    self.put_agent(agent_resource(
                        &context,
                        caller.subject.clone(),
                        sandbox_id,
                        workspace_id,
                        id.clone(),
                        AgentStatus::Ready,
                        AgentOrigin::Trigger {
                            trigger_id,
                            launch_id: launch_id.clone(),
                        },
                        request.first_message.clone(),
                    ));
                }
                self.record_result(EffectCall {
                    kind: layer2_core::trigger_domain::LaunchEffectKind::CreateAgent,
                    subject: caller.subject,
                    capabilities: caller.authorization.iter().collect(),
                    authority: Some(authority),
                    launch_id,
                    profile_id: Some(request.profile.id.to_string()),
                    confirmed_digest: None,
                    first_message: request.first_message,
                    result_id: id.to_string(),
                })
            }
        }
    }
}

#[derive(Clone)]
pub(super) struct FrozenInputs {
    pub trigger_id: TriggerId,
    pub revision: TriggerRevision,
    pub profiles: ResolvedTriggerProfiles,
}

pub(super) fn frozen_inputs(
    trigger_id: TriggerId,
    suffix: &str,
    revision: u32,
    first_message: &str,
    nested_target: bool,
) -> FrozenInputs {
    let agent_id = layer2_core::AgentProfileId(format!("agp_{suffix}"));
    let workspace_id = layer2_core::WorkspaceProfileId(format!("wkp_{suffix}"));
    let sandbox_id = layer2_core::SandboxProfileId(format!("sbp_{suffix}"));
    let agent_content = AgentProfileContent {
        isolated_agent: false,
        model: Some(format!("model-{suffix}")),
        thinking_effort: ThinkingEffort::Medium,
        permission_level: PermissionLevel::Guarded,
        skills: Vec::new(),
        system_prompt: format!("System prompt {suffix}"),
    };
    let agent = AgentProfileRevision {
        profile_id: agent_id.clone(),
        revision,
        digest: content_digest(&agent_content).unwrap(),
        content: agent_content,
        published_at: stamp(0),
    };
    let agent_ref = layer2_core::profiles::AgentProfileRef::new(agent_id, revision);
    let workspace_content = WorkspaceProfileContent {
        location: WorkspaceLocation::Directory {
            name: format!("project-{suffix}"),
        },
        name: format!("project-{suffix}"),
        setup_commands: Vec::new(),
        agent_profiles: vec![agent_ref.clone()],
    };
    let workspace = WorkspaceProfileRevision {
        profile_id: workspace_id.clone(),
        revision,
        digest: content_digest(&workspace_content).unwrap(),
        content: workspace_content,
        published_at: stamp(0),
    };
    let workspace_ref = WorkspaceProfileRef::new(workspace_id, revision);
    let sandbox_content = SandboxProfileContent {
        runtime_image: None,
        cpu_millis: 1_000,
        memory_mib: 2_048,
        nyxid_services: Vec::new(),
        agent_provider: AgentProvider::Codex,
        model_access: ModelAccess::ChronoaiManaged,
        max_workspaces: 4,
        max_agents_per_workspace: 4,
        workspace_profiles: if nested_target {
            vec![workspace_ref.clone()]
        } else {
            Vec::new()
        },
        software_install_commands: Vec::new(),
        environment_variables: Vec::new(),
    };
    let sandbox = SandboxProfileRevision {
        profile_id: sandbox_id.clone(),
        revision,
        digest: content_digest(&sandbox_content).unwrap(),
        content: sandbox_content,
        published_at: stamp(0),
    };
    let sandbox_ref = SandboxProfileRef::new(sandbox_id, revision);
    let content = TriggerContent {
        presentation: Presentation {
            title: format!("Trigger {suffix}"),
            description: String::new(),
            button_label: ButtonLabel::OpenInCma,
            theme: Theme::Auto,
            signup_hint: None,
        },
        target: TriggerTarget {
            sandbox_profile: sandbox_ref,
            workspace_profile: workspace_ref,
            agent_profile: agent_ref,
            inline: None,
        },
        reuse: ReusePolicy {
            sandbox: SandboxReusePolicy::AlwaysNew,
            workspace: WorkspaceReusePolicy::AlwaysNew,
            agent: AgentReusePolicy::AlwaysNew,
        },
        first_message: first_message.into(),
        quick_messages: Vec::new(),
        consent: TriggerConsent {
            always_review: false,
        },
    };
    let trigger_revision = TriggerRevision::new(revision, content, stamp(0)).unwrap();
    FrozenInputs {
        trigger_id,
        revision: trigger_revision,
        profiles: ResolvedTriggerProfiles {
            sandbox_workspaces: if nested_target {
                vec![workspace.clone()]
            } else {
                Vec::new()
            },
            sandbox,
            workspace,
            agent,
        },
    }
}

pub(super) fn started_launch(
    inputs: &FrozenInputs,
    authority: LaunchAuthorityClass,
    resolution: LaunchResolution,
    services: Vec<ServiceRequirement>,
) -> Launch {
    let requirements = RequirementsSnapshot {
        services,
        checked_at: stamp(0),
    };
    let mut launch = Launch::new(LaunchSeed {
        trigger_id: inputs.trigger_id.clone(),
        revision_digest: inputs.revision.digest.clone(),
        public_id: PublicTriggerId("pub_worker".into()),
        owner: SubjectId::new("launch-owner"),
        authority,
        placement_id: None,
        referrer_origin: None,
        review: ReviewRequirements::build(&requirements, None, false, false).unwrap(),
        requirements,
        trust: LaunchTrustManifest::none(),
        resolution,
        has_first_message: !inputs.revision.content.first_message.is_empty(),
        command_key_hash: "b".repeat(64),
        command_fingerprint: "c".repeat(64),
        created_at: stamp(0),
    })
    .unwrap();
    launch
        .start(LaunchConfirmation {
            surface: LaunchConsentSurface::Landing,
            trust_confirmed_digest: None,
            acknowledgements: Vec::new(),
            confirmed_at: stamp(0),
        })
        .unwrap();
    launch
}

pub(super) fn test_worker(
    store: Arc<TestLaunchStore>,
    driver: Arc<FakeDriver>,
    holder: &str,
) -> Arc<LaunchWorker> {
    LaunchWorker::new(
        store,
        driver,
        LaunchWorkerConfig {
            holder: holder.into(),
            lease_duration: Duration::from_secs(10),
            poll_delay: Duration::from_secs(1),
            retry_delay: Duration::from_secs(1),
        },
    )
}

pub(super) struct Harness {
    pub inputs: FrozenInputs,
    pub store: Arc<TestLaunchStore>,
    pub driver: Arc<FakeDriver>,
    pub worker: Arc<LaunchWorker>,
    pub launch_id: LaunchId,
}

impl Harness {
    pub(super) fn pending() -> Self {
        Self::new(
            LaunchAuthorityClass::Visitor,
            LaunchResolution::pending(),
            Vec::new(),
            false,
        )
    }

    pub(super) fn new(
        authority: LaunchAuthorityClass,
        resolution: LaunchResolution,
        services: Vec<ServiceRequirement>,
        nested_target: bool,
    ) -> Self {
        let inputs = frozen_inputs(
            TriggerId("trg_worker".into()),
            "worker",
            1,
            "Inspect the workspace",
            nested_target,
        );
        let store = Arc::new(TestLaunchStore::default());
        let driver = FakeDriver::new(inputs.clone());
        let launch = started_launch(&inputs, authority, resolution, services);
        let launch_id = launch.id.clone();
        store.insert(launch);
        let worker = test_worker(Arc::clone(&store), Arc::clone(&driver), "launch-worker");
        Self {
            inputs,
            store,
            driver,
            worker,
            launch_id,
        }
    }

    pub(super) async fn run(&self, minute: u64) {
        assert!(self
            .worker
            .run_one(&self.launch_id, time(minute))
            .await
            .unwrap());
    }

    pub(super) fn launch(&self) -> Launch {
        self.store.stored(&self.launch_id)
    }
}

pub(super) fn sandbox_resource(
    context: &ResolvedTriggerProfiles,
    owner: SubjectId,
    id: SandboxId,
    status: SandboxStatus,
    origin: SandboxOrigin,
) -> Sandbox {
    Sandbox {
        id,
        owner,
        name: "launch-sandbox".into(),
        origin,
        profile: SandboxProfileSnapshot::from(&context.sandbox),
        status,
        failure: (status == SandboxStatus::Failed).then(|| TypedFailure {
            class: "provisioning_failed".into(),
            detail: "sandbox provisioning failed".into(),
        }),
        runtime: (status == SandboxStatus::Active).then(|| RuntimeBinding {
            boot_id: None,
            provision_generation: Some(1),
            runtime_id: "runtime-launch".into(),
            runtime_version: "test".into(),
            bound_at: stamp(0),
        }),
        credential: SandboxCredentialState {
            source: Default::default(),
            service_grants: Vec::new(),
            service_scope: None,
            reaches: Vec::new(),
            key_generation: 0,
            delivered: false,
            current: None,
            pending: None,
            revoke_debt: Vec::new(),
            sign_in: SandboxSignInState::NotRequired,
        },
        work: SandboxWorkState::new(SandboxWorkStep::None, stamp(0)),
        limits: SandboxLimits {
            max_workspaces: context.sandbox.content.max_workspaces,
            max_agents_per_workspace: context.sandbox.content.max_agents_per_workspace,
        },
        lifecycle: layer2_core::sandbox::SandboxLifecycleState {
            runtime_generation_high_water: u64::from(status == SandboxStatus::Active),
            ..Default::default()
        },
        last_activity_at: stamp(0),
        created_at: stamp(0),
        updated_at: stamp(0),
        retired_at: (status == SandboxStatus::Retired).then(|| stamp(0)),
        purge_after: None,
        revision: 1,
    }
}

pub(super) fn workspace_resource(
    context: &ResolvedTriggerProfiles,
    owner: SubjectId,
    sandbox_id: SandboxId,
    id: WorkspaceId,
    status: WorkspaceStatus,
    origin: WorkspaceOrigin,
) -> Workspace {
    let name = context.workspace.content.workspace_name().unwrap();
    Workspace {
        id,
        sandbox_id,
        owner,
        origin,
        profile: WorkspaceProfileSnapshot::from(&context.workspace),
        name: name.clone(),
        path: format!("/home/cma/{name}"),
        source: WorkspaceSource::Empty,
        resolved_commit: (status == WorkspaceStatus::Ready).then(|| "d".repeat(40)),
        status,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::None, stamp(0)),
        next_worktree_number: 1,
        created_at: stamp(0),
        ready_at: (status == WorkspaceStatus::Ready).then(|| stamp(0)),
        failed: (status == WorkspaceStatus::Failed).then(|| TypedFailure {
            class: "workspace_failed".into(),
            detail: "workspace provisioning failed".into(),
        }),
        removed_at: (status == WorkspaceStatus::Removed).then(|| stamp(0)),
        revision: 1,
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn agent_resource(
    context: &ResolvedTriggerProfiles,
    owner: SubjectId,
    sandbox_id: SandboxId,
    workspace_id: WorkspaceId,
    id: AgentId,
    status: AgentStatus,
    origin: AgentOrigin,
    first_message: Option<String>,
) -> Agent {
    let creation_key = format!("trigger:{}", content_digest(&origin).unwrap());
    let message = first_message
        .map(|text| FirstMessageRecord::new(creation_key.clone(), text, stamp(0)).unwrap());
    let was_ready = status != AgentStatus::Starting && status != AgentStatus::Failed;
    Agent {
        id: id.clone(),
        worktree_id: WorktreeId(format!("wkt_{}", id.as_str())),
        workspace_id,
        sandbox_id,
        owner,
        creation_key,
        creation_fingerprint: "e".repeat(64),
        profile: AgentProfileSnapshot::from(&context.agent),
        capabilities: RuntimeCapabilities::new([RuntimeCapability::Conversation]),
        status,
        conversation: ConversationState {
            turn_timeout: None,
            preferences: Default::default(),
            managed_guidance_revision: 0,
            observations: Default::default(),
            settings: None,
            conversation_id: was_ready.then(|| format!("conversation-{}", id.as_str())),
            runtime_generation: was_ready.then_some(1),
            last_seq: 1,
            first_message: message,
            active_turn: None,
            operation: None,
            queued_messages: Vec::new(),
        },
        work: AgentWorkState::new(AgentWorkStep::None, stamp(0)),
        failure: (status == AgentStatus::Failed).then(|| TypedFailure {
            class: "agent_failed".into(),
            detail: "agent startup failed".into(),
        }),
        title: Some("Launch agent".into()),
        created_at: stamp(0),
        updated_at: stamp(0),
        ready_at: was_ready.then(|| stamp(0)),
        stopped_at: (status == AgentStatus::Stopped).then(|| stamp(0)),
        requested_stop_reason: None,
        stop_reason: (status == AgentStatus::Stopped).then_some(AgentStopReason::Owner),
        deleted_at: (status == AgentStatus::Deleted).then(|| stamp(0)),
        origin,
        revision: 1,
    }
}

pub(super) fn reused_resolution(
    sandbox: SandboxId,
    workspace: Option<WorkspaceId>,
    agent: Option<AgentId>,
) -> LaunchResolution {
    LaunchResolution {
        sandbox: ResolvedResource::Reused(sandbox),
        workspace: workspace.map_or(ResolvedResource::Pending, ResolvedResource::Reused),
        agent: agent.map_or(ResolvedResource::Pending, ResolvedResource::Reused),
        choice_required: false,
    }
}
