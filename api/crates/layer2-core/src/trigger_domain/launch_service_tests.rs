use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::profiles::{
    AgentProfile, AgentProfileContent, AgentProfileRef, AgentProfileSnapshot, AgentProfileStore,
    AgentProvider, EnvironmentVariable, InMemoryProfileStore, ModelAccess, PermissionLevel,
    ProfileCapacityBounds, ProfileVisibility, SandboxProfile, SandboxProfileContent,
    SandboxProfileRef, SandboxProfileSnapshot, SandboxProfileStore, ThinkingEffort,
    WorkspaceLocation, WorkspaceProfile, WorkspaceProfileContent, WorkspaceProfileRef,
    WorkspaceProfileSnapshot, WorkspaceProfileStore,
};
use crate::sandbox::{
    Agent, AgentOrigin, AgentStatus, AgentWorkState, AgentWorkStep, ConversationState,
    FirstMessageDelivery, FirstMessageRecord, Sandbox, SandboxCredentialState, SandboxLimits,
    SandboxOrigin, SandboxSignInState, SandboxStatus, SandboxWorkState, SandboxWorkStep,
    TypedFailure, Workspace, WorkspaceOrigin, WorkspaceSource, WorkspaceStatus, WorkspaceWorkState,
    WorkspaceWorkStep,
};
use crate::{
    AgentId, Authorization, Caller, Capability, LaunchId, RuntimeCapabilities, RuntimeCapability,
    SandboxId, ServiceId, SubjectId, WorkspaceId, WorktreeId,
};

use super::*;

mod archive;
mod credential_prerequisite;
mod initiation;
mod input_protocol;
mod no_review_consent;
mod prepared_reuse;
mod request_validation;
mod reused_connections;
mod trust;
mod visitor_allowance;

const NOW: &str = "2026-09-05T00:00:00Z";
const PUBLISHED: &str = "2026-09-05T00:01:00Z";
const VISITOR: &str = "visitor";
const PUBLISHER: &str = "publisher";

#[derive(Clone, Copy, Debug)]
enum ExistingContext {
    Nothing,
    SandboxPaused,
    Workspace,
    LiveAgent,
    StoppedAgent,
}

#[derive(Default)]
struct ContextState {
    sandbox: Option<Sandbox>,
    workspace: Option<Workspace>,
    agent: Option<Agent>,
    prepared_agents: std::collections::BTreeMap<AgentId, Agent>,
    sandbox_reads: usize,
    workspace_reads: usize,
    agent_reads: usize,
}

#[derive(Clone, Default)]
struct StubContextReader {
    state: Arc<Mutex<ContextState>>,
}

impl StubContextReader {
    fn replace(
        &self,
        sandbox: Option<Sandbox>,
        workspace: Option<Workspace>,
        agent: Option<Agent>,
    ) {
        let mut state = self.state.lock().unwrap();
        state.sandbox = sandbox;
        state.workspace = workspace;
        state.agent = agent;
        state.sandbox_reads = 0;
        state.workspace_reads = 0;
        state.agent_reads = 0;
    }

    fn reads(&self) -> (usize, usize, usize) {
        let state = self.state.lock().unwrap();
        (
            state.sandbox_reads,
            state.workspace_reads,
            state.agent_reads,
        )
    }
}

#[async_trait]
impl LaunchContextReader for StubContextReader {
    async fn owned_sandbox(
        &self,
        owner: &SubjectId,
        id: &SandboxId,
    ) -> Result<Option<Sandbox>, LaunchResolutionError> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .sandbox
            .as_ref()
            .filter(|sandbox| sandbox.owner == *owner && sandbox.id == *id)
            .cloned())
    }
    async fn owned_agent(
        &self,
        owner: &SubjectId,
        id: &AgentId,
    ) -> Result<Option<Agent>, LaunchResolutionError> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .prepared_agents
            .get(id)
            .filter(|agent| agent.owner == *owner)
            .cloned())
    }

    async fn newest_matching_sandbox(
        &self,
        _owner: &SubjectId,
        _profile_id: &crate::SandboxProfileId,
    ) -> Result<Option<Sandbox>, LaunchResolutionError> {
        let mut state = self.state.lock().unwrap();
        state.sandbox_reads += 1;
        Ok(state.sandbox.clone())
    }

    async fn matching_workspace(
        &self,
        _owner: &SubjectId,
        _sandbox_id: &SandboxId,
        _profile_id: &crate::WorkspaceProfileId,
    ) -> Result<Option<Workspace>, LaunchResolutionError> {
        let mut state = self.state.lock().unwrap();
        state.workspace_reads += 1;
        Ok(state.workspace.clone())
    }

    async fn newest_live_trigger_agent(
        &self,
        _owner: &SubjectId,
        _trigger_id: &TriggerId,
    ) -> Result<Option<Agent>, LaunchResolutionError> {
        let mut state = self.state.lock().unwrap();
        state.agent_reads += 1;
        Ok(state.agent.clone())
    }
}

#[derive(Default)]
struct BrokerState {
    requirements: Vec<ServiceRequirement>,
    calls: usize,
    unavailable: bool,
}

#[derive(Clone, Default)]
struct StubBroker {
    state: Arc<Mutex<BrokerState>>,
}

impl StubBroker {
    fn set(&self, requirements: Vec<ServiceRequirement>) {
        self.state.lock().unwrap().requirements = requirements;
    }

    fn calls(&self) -> usize {
        self.state.lock().unwrap().calls
    }

    fn set_unavailable(&self, unavailable: bool) {
        self.state.lock().unwrap().unavailable = unavailable;
    }
}

#[async_trait]
impl ServiceConnectionBroker for StubBroker {
    async fn requirements(
        &self,
        _owner: &SubjectId,
        services: &[ServiceId],
    ) -> Result<Vec<ServiceRequirement>, ServiceConnectionError> {
        let mut state = self.state.lock().unwrap();
        state.calls += 1;
        if state.unavailable {
            return Err(ServiceConnectionError::Unavailable("offline".into()));
        }
        let mut requirements = state.requirements.clone();
        let public = ServiceId::new(AgentProvider::default().connection_services().managed);
        // These fixtures have an already-connected managed model provider.
        // Individual tests can override it explicitly with a missing state.
        if services.contains(&public)
            && !requirements
                .iter()
                .any(|requirement| requirement.service == public)
        {
            requirements.push(ServiceRequirement {
                service: public,
                state: ServiceRequirementState::Satisfied,
            });
        }
        Ok(requirements)
    }

    async fn create_handoff(
        &self,
        _owner: &SubjectId,
        _service: &ServiceId,
        _intent_id: &str,
    ) -> Result<BrokerHandoff, ServiceConnectionError> {
        Err(ServiceConnectionError::Refused)
    }

    async fn handoff_state(
        &self,
        _owner: &SubjectId,
        _external_id: &str,
        _service: &ServiceId,
    ) -> Result<BrokerHandoffState, ServiceConnectionError> {
        Err(ServiceConnectionError::Refused)
    }
}

struct Fixture {
    profiles: Arc<InMemoryProfileStore>,
    definitions: Arc<InMemoryTriggerStore>,
    launches: Arc<InMemoryLaunchStore>,
    contexts: Arc<StubContextReader>,
    broker: Arc<StubBroker>,
    service: Arc<LaunchService>,
    public: PublicTrigger,
    resolved_profiles: ResolvedTriggerProfiles,
    caller: Caller,
}

impl Fixture {
    fn request(&self, key: impl Into<String>) -> CreateOrResumeLaunch {
        CreateOrResumeLaunch {
            placement_id: None,
            referrer_origin: None,
            start_if_ready: false,
            choice: None,
            idempotency_key: key.into(),
        }
    }

    fn service_with_policy(&self, policy: LaunchPolicy) -> Arc<LaunchService> {
        LaunchService::new(
            self.definitions.clone(),
            self.profiles.clone(),
            self.launches.clone(),
            self.contexts.clone(),
            self.broker.clone(),
            Arc::new(policy),
            Arc::new(crate::sandbox::FixedSandboxCredentialPrerequisite::Ready),
        )
    }

    async fn stats(&self) -> TriggerStats {
        self.definitions
            .trigger(&self.public.trigger_id, &self.public.owner)
            .await
            .unwrap()
            .unwrap()
            .stats
    }

    fn set_context(&self, scenario: ExistingContext) {
        let sandbox_status = if matches!(scenario, ExistingContext::SandboxPaused) {
            SandboxStatus::Paused
        } else {
            SandboxStatus::Active
        };
        let sandbox = (!matches!(scenario, ExistingContext::Nothing))
            .then(|| sandbox(&self.resolved_profiles, sandbox_status));
        let workspace = matches!(
            scenario,
            ExistingContext::Workspace | ExistingContext::LiveAgent | ExistingContext::StoppedAgent
        )
        .then(|| workspace(&self.resolved_profiles));
        let agent = matches!(scenario, ExistingContext::LiveAgent).then(|| {
            agent(
                &self.resolved_profiles,
                &self.public.trigger_id,
                AgentStatus::Ready,
            )
        });
        self.contexts.replace(sandbox, workspace, agent);
    }
}

#[tokio::test]
async fn resolution_matrix_covers_every_policy_and_existing_context() {
    let sandbox_policies = [
        SandboxReusePolicy::ReuseExisting,
        SandboxReusePolicy::AlwaysNew,
    ];
    let workspace_policies = [
        WorkspaceReusePolicy::ReuseExisting,
        WorkspaceReusePolicy::AlwaysNew,
    ];
    let agent_policies = [
        AgentReusePolicy::ResumeExisting,
        AgentReusePolicy::AlwaysNew,
        AgentReusePolicy::Ask,
    ];
    let contexts = [
        ExistingContext::Nothing,
        ExistingContext::SandboxPaused,
        ExistingContext::Workspace,
        ExistingContext::LiveAgent,
        ExistingContext::StoppedAgent,
    ];

    for sandbox_policy in sandbox_policies {
        for workspace_policy in workspace_policies {
            for agent_policy in agent_policies {
                let reuse = ReusePolicy {
                    sandbox: sandbox_policy,
                    workspace: workspace_policy,
                    agent: agent_policy,
                };
                let fixture = fixture(reuse, VISITOR, false, false, vec![]).await;
                for (index, existing) in contexts.into_iter().enumerate() {
                    fixture.set_context(existing);
                    let request = fixture.request(format!(
                        "matrix-{sandbox_policy:?}-{workspace_policy:?}-{agent_policy:?}-{index}"
                    ));
                    let result = fixture
                        .service
                        .create_or_resume(&fixture.caller, &fixture.public.public_id, request)
                        .await;
                    let live_agent = matches!(existing, ExistingContext::LiveAgent);
                    if agent_policy == AgentReusePolicy::Ask && live_agent {
                        assert!(matches!(
                            result,
                            Err(LaunchServiceError::RepeatChoiceRequired { .. })
                        ));
                        continue;
                    }
                    let launch = result.unwrap().launch;
                    let resumes_agent =
                        live_agent && agent_policy == AgentReusePolicy::ResumeExisting;
                    let has_sandbox = resumes_agent
                        || (sandbox_policy == SandboxReusePolicy::ReuseExisting
                            && !matches!(existing, ExistingContext::Nothing));
                    let has_workspace = resumes_agent
                        || (has_sandbox
                            && workspace_policy == WorkspaceReusePolicy::ReuseExisting
                            && matches!(
                                existing,
                                ExistingContext::Workspace
                                    | ExistingContext::LiveAgent
                                    | ExistingContext::StoppedAgent
                            ));
                    assert_eq!(launch.resolution.sandbox.is_pending(), !has_sandbox);
                    assert_eq!(launch.resolution.workspace.is_pending(), !has_workspace);
                    assert_eq!(launch.resolution.agent.is_pending(), !resumes_agent);
                    assert_eq!(launch.state, LaunchState::Draft);
                }
            }
        }
    }
}

#[tokio::test]
async fn authorization_disabled_and_trigger_availability_fail_before_launch_work() {
    let fixture = fixture(default_reuse(), VISITOR, false, false, vec![]).await;
    let denied = Caller::new(SubjectId::new(VISITOR), Authorization::none());
    let unknown = PublicTriggerId("pub_missing".into());
    assert_eq!(
        fixture
            .service
            .create_or_resume(&denied, &unknown, fixture.request("denied"))
            .await
            .unwrap_err(),
        LaunchServiceError::Forbidden
    );
    assert_eq!(fixture.contexts.reads(), (0, 0, 0));
    assert_eq!(fixture.broker.calls(), 0);

    let disabled = fixture.service_with_policy(LaunchPolicy {
        enabled: false,
        orchestration_paused: false,
        visitor_launches_per_day: 5,
    });
    assert_eq!(
        disabled
            .create_or_resume(
                &fixture.caller,
                &fixture.public.public_id,
                fixture.request("disabled"),
            )
            .await
            .unwrap_err(),
        LaunchServiceError::Disabled
    );
    assert_eq!(
        fixture
            .service
            .create_or_resume(&fixture.caller, &unknown, fixture.request("missing"))
            .await
            .unwrap_err(),
        LaunchServiceError::TriggerNotFound
    );
    fixture
        .definitions
        .retire_trigger(
            &fixture.public.trigger_id,
            &fixture.public.owner,
            "2026-09-05T00:03:00Z".into(),
        )
        .await
        .unwrap();
    assert_eq!(
        fixture
            .service
            .create_or_resume(
                &fixture.caller,
                &fixture.public.public_id,
                fixture.request("retired"),
            )
            .await
            .unwrap_err(),
        LaunchServiceError::TriggerRetired
    );
}

#[tokio::test]
async fn nested_resolution_queries_only_applicable_context_and_requirements() {
    let always_new = fixture(
        ReusePolicy {
            sandbox: SandboxReusePolicy::AlwaysNew,
            workspace: WorkspaceReusePolicy::ReuseExisting,
            agent: AgentReusePolicy::AlwaysNew,
        },
        VISITOR,
        false,
        false,
        vec![ServiceId::new("github")],
    )
    .await;
    always_new.broker.set(vec![requirement(
        "github",
        ServiceRequirementState::Satisfied,
    )]);
    always_new.set_context(ExistingContext::LiveAgent);
    always_new
        .service
        .create_or_resume(
            &always_new.caller,
            &always_new.public.public_id,
            always_new.request("always-new"),
        )
        .await
        .unwrap();
    assert_eq!(always_new.contexts.reads(), (0, 0, 0));
    assert_eq!(always_new.broker.calls(), 1);

    let reused = fixture(
        default_reuse(),
        VISITOR,
        true,
        false,
        vec![ServiceId::new("github")],
    )
    .await;
    reused.set_context(ExistingContext::SandboxPaused);
    let launch = reused
        .service
        .create_or_resume(
            &reused.caller,
            &reused.public.public_id,
            reused.request("reused"),
        )
        .await
        .unwrap()
        .launch;
    assert_eq!(
        launch.requirements.services,
        vec![
            requirement("github", ServiceRequirementState::Satisfied),
            requirement("chrono-llm-public", ServiceRequirementState::Satisfied)
        ]
    );
    assert_eq!(reused.contexts.reads(), (1, 1, 1));
    assert_eq!(reused.broker.calls(), 1);
}

#[tokio::test]
async fn immediate_start_and_command_replay_are_atomic_and_stable() {
    let fixture = fixture(default_reuse(), PUBLISHER, false, false, vec![]).await;
    let mut request = fixture.request("same-command");
    request.start_if_ready = true;
    let first = fixture
        .service
        .create_or_resume(&fixture.caller, &fixture.public.public_id, request.clone())
        .await
        .unwrap();
    assert!(first.created);
    assert!(first.accepted());
    assert_eq!(first.launch.state, LaunchState::Resolving);
    assert_eq!(
        first.launch.confirmations[0].surface,
        LaunchConsentSurface::Landing
    );
    assert_eq!(first.launch.work.step, LaunchWorkStep::ResolveSandbox);
    assert_eq!(
        fixture.stats().await,
        TriggerStats {
            launches_total: 1,
            agents_ready_total: 0,
        }
    );

    let replay = fixture
        .service
        .create_or_resume(&fixture.caller, &fixture.public.public_id, request.clone())
        .await
        .unwrap();
    assert!(!replay.created);
    assert_eq!(replay.launch, first.launch);
    assert_eq!(
        fixture.stats().await,
        TriggerStats {
            launches_total: 1,
            agents_ready_total: 0,
        }
    );
    request.start_if_ready = false;
    assert_eq!(
        fixture
            .service
            .create_or_resume(&fixture.caller, &fixture.public.public_id, request)
            .await
            .unwrap_err(),
        LaunchServiceError::IdempotencyKeyReused
    );

    let draft = fixture
        .service
        .create_or_resume(
            &fixture.caller,
            &fixture.public.public_id,
            fixture.request("draft-command"),
        )
        .await
        .unwrap();
    assert!(!draft.accepted());
    assert_eq!(draft.launch.state, LaunchState::Draft);
}

#[tokio::test]
async fn admission_freezes_authority_without_replay_escalation() {
    let fixture = fixture(default_reuse(), PUBLISHER, false, false, vec![]).await;
    let visitor = fixture
        .service
        .create_or_resume(
            &fixture.caller,
            &fixture.public.public_id,
            fixture.request("visitor-authority"),
        )
        .await
        .unwrap()
        .launch;
    assert_eq!(visitor.authority, LaunchAuthorityClass::Visitor);

    let creator = Caller::new(
        SubjectId::new(VISITOR),
        Authorization::new([Capability::CreateSandbox]),
    );
    let creator_launch = fixture
        .service
        .create_or_resume(
            &creator,
            &fixture.public.public_id,
            fixture.request("creator-authority"),
        )
        .await
        .unwrap()
        .launch;
    assert_eq!(
        creator_launch.authority,
        LaunchAuthorityClass::SandboxCreator
    );

    let expanded = Caller::new(
        SubjectId::new(VISITOR),
        Authorization::new([Capability::LaunchTriggers, Capability::CreateSandbox]),
    );
    let replay = fixture
        .service
        .create_or_resume(
            &expanded,
            &fixture.public.public_id,
            fixture.request("visitor-authority"),
        )
        .await
        .unwrap()
        .launch;
    assert_eq!(replay.id, visitor.id);
    assert_eq!(replay.authority, LaunchAuthorityClass::Visitor);
}

#[tokio::test]
async fn exact_replay_precedes_mutable_launch_dependencies() {
    let disabled = fixture(default_reuse(), VISITOR, false, false, vec![]).await;
    let request = disabled.request("replay-disabled");
    let admitted = disabled
        .service
        .create_or_resume(
            &disabled.caller,
            &disabled.public.public_id,
            request.clone(),
        )
        .await
        .unwrap()
        .launch;
    let disabled_service = disabled.service_with_policy(LaunchPolicy {
        enabled: false,
        orchestration_paused: false,
        visitor_launches_per_day: 5,
    });
    let replay = disabled_service
        .create_or_resume(
            &disabled.caller,
            &disabled.public.public_id,
            request.clone(),
        )
        .await
        .unwrap();
    assert!(!replay.created);
    assert_eq!(replay.launch, admitted);
    let mut changed = request;
    changed.start_if_ready = true;
    assert_eq!(
        disabled_service
            .create_or_resume(&disabled.caller, &disabled.public.public_id, changed)
            .await
            .unwrap_err(),
        LaunchServiceError::IdempotencyKeyReused
    );

    let retired = fixture(default_reuse(), VISITOR, false, false, vec![]).await;
    let request = retired.request("replay-retired");
    let admitted = retired
        .service
        .create_or_resume(&retired.caller, &retired.public.public_id, request.clone())
        .await
        .unwrap()
        .launch;
    retired
        .definitions
        .retire_trigger(
            &retired.public.trigger_id,
            &retired.public.owner,
            "2026-09-05T00:03:00Z".into(),
        )
        .await
        .unwrap();
    assert_eq!(
        retired
            .service
            .create_or_resume(&retired.caller, &retired.public.public_id, request)
            .await
            .unwrap()
            .launch,
        admitted
    );

    let ask = fixture(
        ReusePolicy {
            sandbox: SandboxReusePolicy::ReuseExisting,
            workspace: WorkspaceReusePolicy::ReuseExisting,
            agent: AgentReusePolicy::Ask,
        },
        VISITOR,
        false,
        false,
        vec![],
    )
    .await;
    let request = ask.request("replay-context-change");
    let admitted = ask
        .service
        .create_or_resume(&ask.caller, &ask.public.public_id, request.clone())
        .await
        .unwrap()
        .launch;
    ask.set_context(ExistingContext::LiveAgent);
    assert_eq!(
        ask.service
            .create_or_resume(&ask.caller, &ask.public.public_id, request)
            .await
            .unwrap()
            .launch,
        admitted
    );

    let unavailable = fixture(
        default_reuse(),
        VISITOR,
        false,
        false,
        vec![ServiceId::new("github")],
    )
    .await;
    let request = unavailable.request("replay-provider-failure");
    let admitted = unavailable
        .service
        .create_or_resume(
            &unavailable.caller,
            &unavailable.public.public_id,
            request.clone(),
        )
        .await
        .unwrap()
        .launch;
    unavailable.broker.set_unavailable(true);
    assert_eq!(
        unavailable
            .service
            .create_or_resume(&unavailable.caller, &unavailable.public.public_id, request,)
            .await
            .unwrap()
            .launch,
        admitted
    );
}

#[tokio::test]
async fn repeat_choice_returns_bounded_context_and_records_both_valid_branches() {
    let fixture = fixture(
        ReusePolicy {
            sandbox: SandboxReusePolicy::ReuseExisting,
            workspace: WorkspaceReusePolicy::ReuseExisting,
            agent: AgentReusePolicy::Ask,
        },
        VISITOR,
        false,
        false,
        vec![],
    )
    .await;
    fixture.set_context(ExistingContext::LiveAgent);
    let error = fixture
        .service
        .create_or_resume(
            &fixture.caller,
            &fixture.public.public_id,
            fixture.request("choice-required"),
        )
        .await
        .unwrap_err();
    let LaunchServiceError::RepeatChoiceRequired { existing } = error else {
        panic!("expected repeat choice");
    };
    assert_eq!(existing.first_message_excerpt.chars().count(), 140);
    assert_eq!(existing.title.as_deref(), Some("Existing Agent"));

    let mut new_request = fixture.request("choice-new");
    new_request.choice = Some(RepeatChoice::New);
    let new_launch = fixture
        .service
        .create_or_resume(&fixture.caller, &fixture.public.public_id, new_request)
        .await
        .unwrap()
        .launch;
    assert_eq!(new_launch.state, LaunchState::Draft);
    assert!(matches!(
        new_launch.resolution.sandbox,
        ResolvedResource::Reused(_)
    ));
    assert!(matches!(
        new_launch.resolution.workspace,
        ResolvedResource::Reused(_)
    ));
    assert!(new_launch.resolution.agent.is_pending());

    let mut resume_request = fixture.request("choice-resume");
    resume_request.choice = Some(RepeatChoice::Resume);
    resume_request.start_if_ready = true;
    let resumed = fixture
        .service
        .create_or_resume(
            &fixture.caller,
            &fixture.public.public_id,
            resume_request.clone(),
        )
        .await
        .unwrap()
        .launch;
    assert_eq!(resumed.state, LaunchState::Resolving);
    assert_eq!(resumed.first_message, FirstMessageDelivery::Completed);
    assert_eq!(resumed.confirmations.len(), 1);
    assert_eq!(
        resumed.confirmations[0].surface,
        LaunchConsentSurface::Landing
    );
    assert_eq!(resumed.work.step, LaunchWorkStep::ResolveSandbox);
    assert!(resumed.ready_at.is_none());
    assert!(resumed.review.needs.is_empty());
    assert_eq!(fixture.broker.calls(), 2);
    assert_eq!(
        fixture.stats().await,
        TriggerStats {
            launches_total: 2,
            agents_ready_total: 0,
        }
    );
    let replay = fixture
        .service
        .create_or_resume(&fixture.caller, &fixture.public.public_id, resume_request)
        .await
        .unwrap();
    assert!(!replay.created);
    assert_eq!(replay.launch, resumed);
    assert_eq!(
        fixture.stats().await,
        TriggerStats {
            launches_total: 2,
            agents_ready_total: 0,
        }
    );

    fixture.set_context(ExistingContext::Nothing);
    let mut stale_new = fixture.request("choice-new-after-stop");
    stale_new.choice = Some(RepeatChoice::New);
    let launch = fixture
        .service
        .create_or_resume(&fixture.caller, &fixture.public.public_id, stale_new)
        .await
        .unwrap()
        .launch;
    assert!(launch.resolution.agent.is_pending());

    let mut stale_resume = fixture.request("choice-resume-after-stop");
    stale_resume.choice = Some(RepeatChoice::Resume);
    assert!(matches!(
        fixture
            .service
            .create_or_resume(&fixture.caller, &fixture.public.public_id, stale_resume,)
            .await,
        Err(LaunchServiceError::InvalidRequest(_))
    ));
}

#[tokio::test]
async fn ask_new_without_an_agent_applies_each_parent_policy() {
    for sandbox_policy in [
        SandboxReusePolicy::ReuseExisting,
        SandboxReusePolicy::AlwaysNew,
    ] {
        for workspace_policy in [
            WorkspaceReusePolicy::ReuseExisting,
            WorkspaceReusePolicy::AlwaysNew,
        ] {
            let fixture = fixture(
                ReusePolicy {
                    sandbox: sandbox_policy,
                    workspace: workspace_policy,
                    agent: AgentReusePolicy::Ask,
                },
                VISITOR,
                false,
                false,
                vec![],
            )
            .await;
            fixture.set_context(ExistingContext::Workspace);
            let mut request =
                fixture.request(format!("ask-new-{sandbox_policy:?}-{workspace_policy:?}"));
            request.choice = Some(RepeatChoice::New);
            let launch = fixture
                .service
                .create_or_resume(&fixture.caller, &fixture.public.public_id, request)
                .await
                .unwrap()
                .launch;
            assert_eq!(
                launch.resolution.sandbox.is_pending(),
                sandbox_policy == SandboxReusePolicy::AlwaysNew
            );
            assert_eq!(
                launch.resolution.workspace.is_pending(),
                sandbox_policy == SandboxReusePolicy::AlwaysNew
                    || workspace_policy == WorkspaceReusePolicy::AlwaysNew
            );
            assert!(launch.resolution.agent.is_pending());
        }
    }
}

#[tokio::test]
async fn context_corruption_and_inapplicable_choices_fail_closed() {
    let fixture = fixture(default_reuse(), VISITOR, false, false, vec![]).await;
    let mut bad_sandbox = sandbox(&fixture.resolved_profiles, SandboxStatus::Active);
    bad_sandbox.owner = SubjectId::new("other");
    fixture.contexts.replace(Some(bad_sandbox), None, None);
    assert!(matches!(
        fixture
            .service
            .create_or_resume(
                &fixture.caller,
                &fixture.public.public_id,
                fixture.request("bad-sandbox"),
            )
            .await,
        Err(LaunchServiceError::Corrupt(_))
    ));

    let good_sandbox = sandbox(&fixture.resolved_profiles, SandboxStatus::Active);
    let mut bad_workspace = workspace(&fixture.resolved_profiles);
    bad_workspace.profile.id = crate::WorkspaceProfileId("wkp_other".into());
    fixture
        .contexts
        .replace(Some(good_sandbox.clone()), Some(bad_workspace), None);
    assert!(matches!(
        fixture
            .service
            .create_or_resume(
                &fixture.caller,
                &fixture.public.public_id,
                fixture.request("bad-workspace"),
            )
            .await,
        Err(LaunchServiceError::Corrupt(_))
    ));

    let good_workspace = workspace(&fixture.resolved_profiles);
    let mut bad_agent = agent(
        &fixture.resolved_profiles,
        &fixture.public.trigger_id,
        AgentStatus::Stopped,
    );
    bad_agent.workspace_id = good_workspace.id.clone();
    fixture
        .contexts
        .replace(Some(good_sandbox), Some(good_workspace), Some(bad_agent));
    assert!(matches!(
        fixture
            .service
            .create_or_resume(
                &fixture.caller,
                &fixture.public.public_id,
                fixture.request("bad-agent"),
            )
            .await,
        Err(LaunchServiceError::Corrupt(_))
    ));

    let mut choice = fixture.request("invalid-choice");
    choice.choice = Some(RepeatChoice::New);
    fixture.set_context(ExistingContext::Nothing);
    assert!(matches!(
        fixture
            .service
            .create_or_resume(&fixture.caller, &fixture.public.public_id, choice)
            .await,
        Err(LaunchServiceError::InvalidRequest(_))
    ));
}

#[tokio::test]
async fn reuse_matches_parent_profile_ids_and_global_trigger_origin_across_revisions() {
    let parent_fixture = fixture(
        ReusePolicy {
            sandbox: SandboxReusePolicy::ReuseExisting,
            workspace: WorkspaceReusePolicy::ReuseExisting,
            agent: AgentReusePolicy::AlwaysNew,
        },
        VISITOR,
        false,
        false,
        vec![],
    )
    .await;
    let mut old_sandbox = sandbox(&parent_fixture.resolved_profiles, SandboxStatus::Failed);
    old_sandbox.profile.revision = 99;
    old_sandbox.failure = Some(TypedFailure {
        class: "service_not_granted".into(),
        detail: "github".into(),
    });
    let mut old_workspace = workspace(&parent_fixture.resolved_profiles);
    old_workspace.profile.revision = 88;
    old_workspace.status = WorkspaceStatus::Failed;
    old_workspace.failed = Some(TypedFailure {
        class: "terminal_validation".into(),
        detail: "setup failed".into(),
    });
    parent_fixture
        .contexts
        .replace(Some(old_sandbox), Some(old_workspace), None);
    let reused = parent_fixture
        .service
        .create_or_resume(
            &parent_fixture.caller,
            &parent_fixture.public.public_id,
            parent_fixture.request("old-parent-revisions"),
        )
        .await
        .unwrap()
        .launch;
    assert!(matches!(
        reused.resolution.sandbox,
        ResolvedResource::Reused(_)
    ));
    assert!(matches!(
        reused.resolution.workspace,
        ResolvedResource::Reused(_)
    ));

    let agent_fixture = fixture(
        ReusePolicy {
            sandbox: SandboxReusePolicy::AlwaysNew,
            workspace: WorkspaceReusePolicy::AlwaysNew,
            agent: AgentReusePolicy::ResumeExisting,
        },
        VISITOR,
        false,
        false,
        vec![],
    )
    .await;
    let mut old_agent = agent(
        &agent_fixture.resolved_profiles,
        &agent_fixture.public.trigger_id,
        AgentStatus::Ready,
    );
    old_agent.profile.id = crate::AgentProfileId("agp_previous_target".into());
    old_agent.profile.revision = 41;
    old_agent.sandbox_id = SandboxId("sbx_previous_launch".into());
    old_agent.workspace_id = WorkspaceId("wks_previous_launch".into());
    let mut old_parent = sandbox(&agent_fixture.resolved_profiles, SandboxStatus::Active);
    old_parent.id = old_agent.sandbox_id.clone();
    agent_fixture
        .contexts
        .replace(Some(old_parent), None, Some(old_agent));
    let mut request = agent_fixture.request("global-agent");
    request.start_if_ready = true;
    let resumed = agent_fixture
        .service
        .create_or_resume(
            &agent_fixture.caller,
            &agent_fixture.public.public_id,
            request,
        )
        .await
        .unwrap()
        .launch;
    assert_eq!(resumed.state, LaunchState::Resolving);
    assert_eq!(
        resumed.resolution.sandbox.id().unwrap().as_str(),
        "sbx_previous_launch"
    );
    assert_eq!(
        resumed.resolution.workspace.id().unwrap().as_str(),
        "wks_previous_launch"
    );
    assert_eq!(agent_fixture.contexts.reads(), (0, 0, 1));
}

#[tokio::test]
async fn existing_agent_still_requires_start_consent_and_owner_review() {
    for always_review in [false, true] {
        let fixture = fixture(default_reuse(), VISITOR, false, always_review, vec![]).await;
        fixture.set_context(ExistingContext::LiveAgent);
        let mut request = fixture.request(format!("existing-review-{always_review}"));
        request.start_if_ready = always_review;
        let draft = fixture
            .service
            .create_or_resume(&fixture.caller, &fixture.public.public_id, request)
            .await
            .unwrap()
            .launch;
        assert_eq!(draft.state, LaunchState::Draft);
        assert_eq!(draft.first_message, FirstMessageDelivery::Completed);
        assert_eq!(
            draft.review.needs,
            always_review
                .then_some(ReviewNeed::OwnerRequiresReview)
                .into_iter()
                .collect::<Vec<_>>()
        );
        let ready = fixture
            .service
            .start(
                &fixture.caller,
                &draft.id,
                StartLaunch {
                    trust_confirmed_digest: None,
                    acknowledgements: vec![],
                },
            )
            .await
            .unwrap();
        assert_eq!(ready.state, LaunchState::Resolving);
        assert_eq!(ready.confirmations[0].surface, LaunchConsentSurface::Review);
        assert_eq!(ready.work.step, LaunchWorkStep::ResolveSandbox);
        assert_eq!(
            fixture.stats().await,
            TriggerStats {
                launches_total: 1,
                agents_ready_total: 0,
            }
        );
    }
}

#[tokio::test]
async fn admitted_drafts_start_from_their_frozen_revision_after_retirement() {
    let pending = fixture(default_reuse(), VISITOR, false, false, vec![]).await;
    let draft = pending
        .service
        .create_or_resume(
            &pending.caller,
            &pending.public.public_id,
            pending.request("retired-start-pending"),
        )
        .await
        .unwrap()
        .launch;
    pending
        .definitions
        .retire_trigger(
            &pending.public.trigger_id,
            &pending.public.owner,
            "2026-09-05T00:03:00Z".into(),
        )
        .await
        .unwrap();
    let started = pending
        .service
        .start(
            &pending.caller,
            &draft.id,
            StartLaunch {
                trust_confirmed_digest: None,
                acknowledgements: vec![],
            },
        )
        .await
        .unwrap();
    assert_eq!(started.state, LaunchState::Resolving);
    assert_eq!(started.revision_digest, draft.revision_digest);
    assert_eq!(
        pending
            .service
            .create_or_resume(
                &pending.caller,
                &pending.public.public_id,
                pending.request("new-after-retirement"),
            )
            .await
            .unwrap_err(),
        LaunchServiceError::TriggerRetired
    );

    let existing = fixture(default_reuse(), VISITOR, false, false, vec![]).await;
    existing.set_context(ExistingContext::LiveAgent);
    let draft = existing
        .service
        .create_or_resume(
            &existing.caller,
            &existing.public.public_id,
            existing.request("retired-start-existing"),
        )
        .await
        .unwrap()
        .launch;
    existing
        .definitions
        .retire_trigger(
            &existing.public.trigger_id,
            &existing.public.owner,
            "2026-09-05T00:03:00Z".into(),
        )
        .await
        .unwrap();
    let ready = existing
        .service
        .start(
            &existing.caller,
            &draft.id,
            StartLaunch {
                trust_confirmed_digest: None,
                acknowledgements: vec![],
            },
        )
        .await
        .unwrap();
    assert_eq!(ready.state, LaunchState::Resolving);
    assert_eq!(ready.revision_digest, draft.revision_digest);
}

#[tokio::test]
async fn admitted_draft_refreshes_review_from_its_frozen_revision_after_republish() {
    let fixture = fixture(default_reuse(), PUBLISHER, true, false, vec![]).await;
    let draft = fixture
        .service
        .create_or_resume(
            &fixture.caller,
            &fixture.public.public_id,
            fixture.request("republished-start"),
        )
        .await
        .unwrap()
        .launch;
    assert_eq!(draft.review.needs.len(), 1);
    assert!(matches!(
        draft.review.needs[0],
        ReviewNeed::TrustConfirmation { .. }
    ));

    let mut changed = fixture.public.revision.content.clone();
    changed.consent.always_review = true;
    fixture
        .definitions
        .save_draft(
            &fixture.public.trigger_id,
            &fixture.public.owner,
            1,
            changed,
            "2026-09-05T00:02:00Z".into(),
        )
        .await
        .unwrap();
    let republished = fixture
        .definitions
        .publish_trigger(
            &fixture.public.trigger_id,
            &fixture.public.owner,
            2,
            "2026-09-05T00:03:00Z".into(),
        )
        .await
        .unwrap();
    assert_ne!(draft.revision_digest, republished.revision.digest);

    assert_eq!(
        fixture
            .service
            .start(
                &fixture.caller,
                &draft.id,
                StartLaunch {
                    trust_confirmed_digest: Some("b".repeat(64)),
                    acknowledgements: vec![],
                },
            )
            .await
            .unwrap_err(),
        LaunchServiceError::TrustConfirmationMismatch
    );
    let refreshed = fixture
        .service
        .read(&fixture.caller, &draft.id)
        .await
        .unwrap();
    assert_eq!(refreshed.revision_digest, draft.revision_digest);
    assert_eq!(refreshed.review.needs, draft.review.needs);
}

#[tokio::test]
async fn start_rechecks_requirements_and_requires_the_exact_trust_digest() {
    let fixture = fixture(
        default_reuse(),
        PUBLISHER,
        true,
        false,
        vec![
            ServiceId::new("github"),
            ServiceId::new("slack"),
            ServiceId::new("docs"),
        ],
    )
    .await;
    fixture.broker.set(vec![
        requirement("github", ServiceRequirementState::Missing),
        requirement("slack", ServiceRequirementState::Inactive),
        requirement("docs", ServiceRequirementState::Satisfied),
    ]);
    let draft = fixture
        .service
        .create_or_resume(
            &fixture.caller,
            &fixture.public.public_id,
            fixture.request("start-later"),
        )
        .await
        .unwrap()
        .launch;
    fixture.broker.set(vec![
        requirement("github", ServiceRequirementState::Satisfied),
        requirement("slack", ServiceRequirementState::Satisfied),
        requirement("docs", ServiceRequirementState::Satisfied),
    ]);
    assert_eq!(
        fixture
            .service
            .start(
                &fixture.caller,
                &draft.id,
                StartLaunch {
                    trust_confirmed_digest: Some("b".repeat(64)),
                    acknowledgements: vec![],
                },
            )
            .await
            .unwrap_err(),
        LaunchServiceError::TrustConfirmationMismatch
    );
    let refreshed = fixture
        .service
        .read(&fixture.caller, &draft.id)
        .await
        .unwrap();
    assert!(refreshed
        .requirements
        .services
        .iter()
        .all(|requirement| requirement.state == ServiceRequirementState::Satisfied));
    assert_eq!(refreshed.state, LaunchState::Draft);
    let started = fixture
        .service
        .start(
            &fixture.caller,
            &draft.id,
            StartLaunch {
                trust_confirmed_digest: Some(fixture.resolved_profiles.sandbox.digest.clone()),
                acknowledgements: vec!["reviewed".into()],
            },
        )
        .await
        .unwrap();
    assert_eq!(started.state, LaunchState::Resolving);
    assert_eq!(
        started.requirements.services[0].state,
        ServiceRequirementState::Satisfied
    );
    assert_eq!(
        started.confirmations[0].surface,
        LaunchConsentSurface::Review
    );
    assert_eq!(fixture.broker.calls(), 3);
    assert_eq!(
        fixture
            .service
            .events(&fixture.caller, &draft.id, draft.event_head, None)
            .await
            .unwrap()
            .into_iter()
            .map(|event| event.kind)
            .collect::<Vec<_>>(),
        vec![
            LaunchEventKind::RequirementChanged {
                service: ServiceId::new("github"),
                state: ServiceRequirementState::Satisfied,
            },
            LaunchEventKind::RequirementChanged {
                service: ServiceId::new("slack"),
                state: ServiceRequirementState::Satisfied,
            },
            LaunchEventKind::StepChanged {
                step: started
                    .steps
                    .iter()
                    .find(|step| step.key == LaunchStepKey::CheckingRequirements)
                    .unwrap()
                    .clone(),
            },
            LaunchEventKind::ConfirmationRecorded,
            LaunchEventKind::StateChanged {
                from: LaunchState::Draft,
                to: LaunchState::Resolving,
            },
        ]
    );
}

#[tokio::test]
async fn partial_requirement_refresh_is_durable_before_start_is_refused() {
    let fixture = fixture(
        default_reuse(),
        VISITOR,
        false,
        false,
        vec![ServiceId::new("github"), ServiceId::new("slack")],
    )
    .await;
    fixture.broker.set(vec![
        requirement("github", ServiceRequirementState::Missing),
        requirement("slack", ServiceRequirementState::Missing),
    ]);
    let draft = fixture
        .service
        .create_or_resume(
            &fixture.caller,
            &fixture.public.public_id,
            fixture.request("partial-requirements"),
        )
        .await
        .unwrap()
        .launch;
    fixture.broker.set(vec![
        requirement("github", ServiceRequirementState::Satisfied),
        requirement("slack", ServiceRequirementState::Missing),
    ]);
    assert_eq!(
        fixture
            .service
            .start(
                &fixture.caller,
                &draft.id,
                StartLaunch {
                    trust_confirmed_digest: None,
                    acknowledgements: vec![],
                },
            )
            .await
            .unwrap_err(),
        LaunchServiceError::RequirementsNotMet {
            services: vec![ServiceId::new("slack")],
        }
    );
    let refreshed = fixture
        .service
        .read(&fixture.caller, &draft.id)
        .await
        .unwrap();
    assert_eq!(
        refreshed.requirements.services[0].state,
        ServiceRequirementState::Satisfied
    );
    assert_eq!(
        refreshed.review.needs,
        vec![ReviewNeed::Connection {
            service: ServiceId::new("slack"),
        }]
    );
    assert_eq!(
        fixture
            .service
            .events(&fixture.caller, &draft.id, draft.event_head, None)
            .await
            .unwrap()
            .into_iter()
            .map(|event| event.kind)
            .collect::<Vec<_>>(),
        vec![LaunchEventKind::RequirementChanged {
            service: ServiceId::new("github"),
            state: ServiceRequirementState::Satisfied,
        }]
    );
}

#[tokio::test]
async fn owner_operations_need_ownership_not_current_launch_authority() {
    let fixture = fixture(default_reuse(), VISITOR, false, false, vec![]).await;
    let launch = fixture
        .service
        .create_or_resume(
            &fixture.caller,
            &fixture.public.public_id,
            fixture.request("cancel"),
        )
        .await
        .unwrap()
        .launch;
    let owner = Caller::new(SubjectId::new(VISITOR), Authorization::none());
    assert_eq!(
        fixture.service.read(&owner, &launch.id).await.unwrap(),
        launch
    );
    assert_eq!(
        fixture
            .service
            .requirements(&owner, &launch.id)
            .await
            .unwrap(),
        launch.requirements
    );
    let foreign = Caller::new(SubjectId::new("other"), Authorization::none());
    assert_eq!(
        fixture
            .service
            .read(&foreign, &launch.id)
            .await
            .unwrap_err(),
        LaunchServiceError::NotFound
    );
    assert_eq!(
        fixture
            .service
            .read(&owner, &LaunchId("lch_missing".into()))
            .await
            .unwrap_err(),
        LaunchServiceError::NotFound
    );
    assert_eq!(
        fixture
            .service
            .start(
                &foreign,
                &launch.id,
                StartLaunch {
                    trust_confirmed_digest: None,
                    acknowledgements: Vec::new(),
                },
            )
            .await
            .unwrap_err(),
        LaunchServiceError::NotFound
    );
    assert_eq!(
        fixture
            .service
            .cancel(&foreign, &launch.id)
            .await
            .unwrap_err(),
        LaunchServiceError::NotFound
    );
    assert_eq!(
        fixture
            .service
            .events(&foreign, &launch.id, 0, None)
            .await
            .unwrap_err(),
        LaunchServiceError::NotFound
    );
    assert_eq!(
        fixture
            .service
            .events(&owner, &launch.id, launch.event_head + 1, None)
            .await
            .unwrap_err(),
        LaunchServiceError::InvalidCursor
    );
    for limit in [0, MAX_LAUNCH_EVENT_LIMIT + 1] {
        assert_eq!(
            fixture
                .service
                .events(&owner, &launch.id, 0, Some(limit))
                .await
                .unwrap_err(),
            LaunchServiceError::InvalidCursor
        );
    }
    let started = fixture
        .service
        .start(
            &owner,
            &launch.id,
            StartLaunch {
                trust_confirmed_digest: None,
                acknowledgements: Vec::new(),
            },
        )
        .await
        .unwrap();
    assert_eq!(started.state, LaunchState::Resolving);
    let cancelled = fixture.service.cancel(&owner, &launch.id).await.unwrap();
    assert_eq!(cancelled.state, LaunchState::Cancelled);
    assert_eq!(
        fixture
            .service
            .cancel(&owner, &launch.id)
            .await
            .unwrap_err(),
        LaunchServiceError::NotStartable
    );
}

fn default_reuse() -> ReusePolicy {
    ReusePolicy {
        sandbox: SandboxReusePolicy::ReuseExisting,
        workspace: WorkspaceReusePolicy::ReuseExisting,
        agent: AgentReusePolicy::ResumeExisting,
    }
}

async fn fixture(
    reuse: ReusePolicy,
    sandbox_owner: &str,
    unsafe_sandbox: bool,
    always_review: bool,
    services: Vec<ServiceId>,
) -> Fixture {
    fixture_with_workspace_setup(
        reuse,
        sandbox_owner,
        unsafe_sandbox,
        always_review,
        services,
        false,
    )
    .await
}

async fn fixture_with_workspace_setup(
    reuse: ReusePolicy,
    sandbox_owner: &str,
    unsafe_sandbox: bool,
    always_review: bool,
    services: Vec<ServiceId>,
    workspace_setup: bool,
) -> Fixture {
    fixture_with_workspace_configuration(
        reuse,
        sandbox_owner,
        unsafe_sandbox,
        always_review,
        services,
        workspace_setup,
        true,
        false,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn fixture_with_workspace_configuration(
    reuse: ReusePolicy,
    sandbox_owner: &str,
    unsafe_sandbox: bool,
    always_review: bool,
    services: Vec<ServiceId>,
    workspace_setup: bool,
    workspace_nested: bool,
    repository_workspace: bool,
) -> Fixture {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let agent_reference = publish_agent(&profiles).await;
    let workspace_reference = publish_workspace(
        &profiles,
        agent_reference.clone(),
        workspace_setup,
        repository_workspace,
    )
    .await;
    let sandbox_reference = publish_sandbox(
        &profiles,
        sandbox_owner,
        unsafe_sandbox,
        services.clone(),
        workspace_reference.clone(),
        workspace_nested,
    )
    .await;
    let target = TriggerTarget {
        sandbox_profile: sandbox_reference,
        workspace_profile: workspace_reference,
        agent_profile: agent_reference,
        inline: None,
    };
    let resolver: Arc<dyn TriggerProfileResolver> = profiles.clone();
    let definitions = Arc::new(InMemoryTriggerStore::new(resolver));
    let trigger = Trigger::new(
        TriggerId::generate(),
        SubjectId::new(PUBLISHER),
        TriggerContent {
            presentation: Presentation {
                title: "Launch a proof agent".into(),
                description: String::new(),
                button_label: ButtonLabel::OpenInCma,
                theme: Theme::Auto,
                signup_hint: None,
            },
            target,
            reuse,
            first_message: "a".repeat(160),
            quick_messages: Vec::new(),
            consent: TriggerConsent { always_review },
        },
        NOW.into(),
    )
    .unwrap();
    let trigger_id = trigger.id.clone();
    definitions.create_trigger(trigger).await.unwrap();
    let public = definitions
        .publish_trigger(&trigger_id, &SubjectId::new(PUBLISHER), 1, PUBLISHED.into())
        .await
        .unwrap();
    let resolved_profiles = profiles
        .read_trigger_profiles(&public.revision.content.target)
        .await
        .unwrap();
    let launches = Arc::new(InMemoryLaunchStore::new(definitions.clone()));
    let contexts = Arc::new(StubContextReader::default());
    let broker = Arc::new(StubBroker::default());
    broker.set(
        services
            .iter()
            .map(|service| ServiceRequirement {
                service: service.clone(),
                state: ServiceRequirementState::Satisfied,
            })
            .collect(),
    );
    let policy = Arc::new(LaunchPolicy {
        enabled: true,
        orchestration_paused: false,
        visitor_launches_per_day: 5,
    });
    let service = LaunchService::new(
        definitions.clone(),
        profiles.clone(),
        launches.clone(),
        contexts.clone(),
        broker.clone(),
        policy,
        Arc::new(crate::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    );
    Fixture {
        profiles,
        definitions,
        launches,
        contexts,
        broker,
        service,
        public,
        resolved_profiles,
        caller: launcher(VISITOR),
    }
}

async fn publish_agent(profiles: &InMemoryProfileStore) -> AgentProfileRef {
    let profile = AgentProfile {
        internal: false,
        id: crate::AgentProfileId("agp_launch_agent".into()),
        owner: SubjectId::new(PUBLISHER),
        name: "Launch Agent".into(),
        description: String::new(),
        visibility: ProfileVisibility::Public,
        forked_from: None,
        uses_count: 0,
        draft: AgentProfileContent {
            isolated_agent: false,
            model: None,
            thinking_effort: ThinkingEffort::Medium,
            permission_level: PermissionLevel::Guarded,
            skills: vec![],
            system_prompt: String::new(),
        },
        draft_version: 1,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
    };
    let id = profile.id.clone();
    profiles.create_agent_profile(profile).await.unwrap();
    profiles
        .publish_agent_profile(&id, 1, PUBLISHED.into())
        .await
        .unwrap();
    AgentProfileRef::new(id, 1)
}

async fn publish_workspace(
    profiles: &InMemoryProfileStore,
    agent: AgentProfileRef,
    setup: bool,
    repository: bool,
) -> WorkspaceProfileRef {
    let (location, name) = if repository {
        (
            WorkspaceLocation::GitRepository {
                url: "https://github.com/chronoai/launch-repo.git".into(),
                reference: None,
            },
            String::new(),
        )
    } else {
        (
            WorkspaceLocation::Directory {
                name: "launch".into(),
            },
            "launch".into(),
        )
    };
    let profile = WorkspaceProfile {
        internal: false,
        id: crate::WorkspaceProfileId("wkp_launch_workspace".into()),
        owner: SubjectId::new(PUBLISHER),
        name: "Launch Workspace".into(),
        description: String::new(),
        visibility: ProfileVisibility::Public,
        forked_from: None,
        uses_count: 0,
        draft: WorkspaceProfileContent {
            location,
            name,
            setup_commands: setup
                .then(|| "prepare-workspace".into())
                .into_iter()
                .collect(),
            agent_profiles: vec![agent],
        },
        draft_version: 1,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
    };
    let id = profile.id.clone();
    profiles.create_workspace_profile(profile).await.unwrap();
    profiles
        .publish_workspace_profile(&id, 1, PUBLISHED.into())
        .await
        .unwrap();
    WorkspaceProfileRef::new(id, 1)
}

async fn publish_sandbox(
    profiles: &InMemoryProfileStore,
    owner: &str,
    unsafe_profile: bool,
    services: Vec<ServiceId>,
    workspace: WorkspaceProfileRef,
    workspace_nested: bool,
) -> SandboxProfileRef {
    let profile = SandboxProfile {
        internal: false,
        availability: Default::default(),
        id: crate::SandboxProfileId("sbp_launch_sandbox".into()),
        owner: SubjectId::new(owner),
        name: "Launch Sandbox".into(),
        description: String::new(),
        visibility: ProfileVisibility::Public,
        forked_from: None,
        uses_count: 0,
        draft: SandboxProfileContent {
            runtime_image: None,
            cpu_millis: 1_000,
            memory_mib: 2_048,
            nyxid_services: services,
            agent_provider: AgentProvider::default(),
            model_access: ModelAccess::ChronoaiManaged,
            max_workspaces: 4,
            max_agents_per_workspace: 4,
            workspace_profiles: workspace_nested.then_some(workspace).into_iter().collect(),
            software_install_commands: unsafe_profile
                .then(|| "install-tool".into())
                .into_iter()
                .collect(),
            environment_variables: unsafe_profile
                .then(|| EnvironmentVariable {
                    name: "TOOL_PATH".into(),
                    value: "/opt/tool".into(),
                })
                .into_iter()
                .collect(),
        },
        draft_version: 1,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
    };
    let id = profile.id.clone();
    profiles.create_sandbox_profile(profile).await.unwrap();
    profiles
        .publish_sandbox_profile(
            &id,
            1,
            PUBLISHED.into(),
            ProfileCapacityBounds {
                min_cpu_millis: 100,
                max_cpu_millis: 8_000,
                min_memory_mib: 128,
                max_memory_mib: 16_384,
            },
            &[],
        )
        .await
        .unwrap();
    SandboxProfileRef::new(id, 1)
}

fn sandbox(profiles: &ResolvedTriggerProfiles, status: SandboxStatus) -> Sandbox {
    Sandbox {
        id: SandboxId("sbx_launch".into()),
        owner: SubjectId::new(VISITOR),
        name: "launch".into(),
        origin: SandboxOrigin::Manual,
        profile: SandboxProfileSnapshot::from(&profiles.sandbox),
        status,
        failure: None,
        runtime: None,
        credential: SandboxCredentialState {
            source: Default::default(),
            service_grants: Vec::new(),
            service_scope: None,
            reaches: profiles.sandbox.content.nyxid_services.clone(),
            key_generation: 0,
            delivered: false,
            current: None,
            pending: None,
            revoke_debt: vec![],
            sign_in: SandboxSignInState::NotRequired,
        },
        work: SandboxWorkState::new(SandboxWorkStep::None, NOW.into()),
        limits: SandboxLimits {
            max_workspaces: 4,
            max_agents_per_workspace: 4,
        },
        lifecycle: Default::default(),
        last_activity_at: NOW.into(),
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
        purge_after: None,
        revision: 1,
    }
}

fn workspace(profiles: &ResolvedTriggerProfiles) -> Workspace {
    Workspace {
        id: WorkspaceId("wks_launch".into()),
        sandbox_id: SandboxId("sbx_launch".into()),
        owner: SubjectId::new(VISITOR),
        origin: WorkspaceOrigin::SandboxProfile { index: 0 },
        profile: WorkspaceProfileSnapshot::from(&profiles.workspace),
        name: "launch".into(),
        path: "/home/cma/launch".into(),
        source: WorkspaceSource::Empty,
        resolved_commit: Some("a".repeat(40)),
        status: WorkspaceStatus::Ready,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::None, NOW.into()),
        next_worktree_number: 1,
        created_at: NOW.into(),
        ready_at: Some(NOW.into()),
        failed: None,
        removed_at: None,
        revision: 1,
    }
}

fn agent(profiles: &ResolvedTriggerProfiles, trigger_id: &TriggerId, status: AgentStatus) -> Agent {
    Agent {
        id: AgentId("agt_launch".into()),
        worktree_id: WorktreeId("wkt_launch".into()),
        workspace_id: WorkspaceId("wks_launch".into()),
        sandbox_id: SandboxId("sbx_launch".into()),
        owner: SubjectId::new(VISITOR),
        creation_key: "trigger-key".into(),
        creation_fingerprint: "a".repeat(64),
        profile: AgentProfileSnapshot::from(&profiles.agent),
        capabilities: RuntimeCapabilities::new([RuntimeCapability::Conversation]),
        status,
        conversation: ConversationState {
            turn_timeout: None,
            preferences: Default::default(),
            managed_guidance_revision: 0,
            observations: Default::default(),
            settings: None,
            conversation_id: Some("conversation".into()),
            runtime_generation: Some(1),
            last_seq: 1,
            first_message: Some(FirstMessageRecord::new("existing", "e".repeat(160), NOW).unwrap()),
            active_turn: None,
            operation: None,
            queued_messages: vec![],
        },
        work: AgentWorkState::new(AgentWorkStep::None, NOW.into()),
        failure: None,
        title: Some("Existing Agent".into()),
        created_at: NOW.into(),
        updated_at: NOW.into(),
        ready_at: Some(NOW.into()),
        stopped_at: (status == AgentStatus::Stopped).then(|| NOW.into()),
        requested_stop_reason: None,
        stop_reason: None,
        deleted_at: None,
        origin: AgentOrigin::Trigger {
            trigger_id: trigger_id.to_string(),
            launch_id: "lch_existing".into(),
        },
        revision: 1,
    }
}

fn launcher(subject: &str) -> Caller {
    Caller::new(
        SubjectId::new(subject),
        Authorization::new([Capability::LaunchTriggers]),
    )
}

fn requirement(service: &str, state: ServiceRequirementState) -> ServiceRequirement {
    ServiceRequirement {
        service: ServiceId::new(service),
        state,
    }
}
