use super::*;
use crate::sandbox::title_scheduler::AgentTitleScheduler;
use crate::store::request_limits::MongoRequestLimits;

pub(crate) fn scheduled_title_fixture() -> (Sandbox, Agent) {
    (sandbox(), agent())
}

pub(crate) fn start_title_scheduler(
    service: Arc<AgentTitleService>,
    store: Arc<dyn AgentLifecycleStore>,
    sandbox: &Sandbox,
    budget: Arc<MongoRequestLimits>,
    origin: String,
    shutdown: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    let credentials = Arc::new(Credentials(
        Mutex::new(Some(CredentialMaterial {
            source: Default::default(),
            github_route: None,
            sandbox_id: sandbox.id.clone(),
            owner: sandbox.owner.clone(),
            authority: sandbox.credential.current.clone().unwrap(),
            value: SecretValue::new(KEY.into()),
        })),
        AtomicUsize::new(0),
    ));
    let worker = AgentTitleWorker::new(
        service,
        store,
        credentials,
        Arc::new(PlatformTitleEndpoint {
            platform_base_url: origin,
        }),
        MODEL.into(),
        budget,
    );
    AgentTitleScheduler::new(worker).spawn(shutdown)
}

pub(in crate::sandbox) fn sandbox() -> Sandbox {
    Sandbox {
        id: SandboxId("sbx_title".into()),
        owner: SubjectId::new("owner"),
        name: "titles".into(),
        origin: SandboxOrigin::Manual,
        profile: SandboxProfileSnapshot {
            id: SandboxProfileId("sbp_title".into()),
            revision: 1,
            digest: "a".repeat(64),
            always_on: None,
            content: SandboxProfileContent {
                runtime_image: None,
                cpu_millis: 1000,
                memory_mib: 2048,
                nyxid_services: vec![ServiceId::new("chrono-llm")],
                agent_provider: AgentProvider::Codex,
                model_access: ModelAccess::ChronoaiManaged,
                max_workspaces: 1,
                max_agents_per_workspace: 4,
                workspace_profiles: vec![],
                software_install_commands: vec![],
                environment_variables: vec![],
            },
        },
        status: SandboxStatus::Active,
        failure: None,
        runtime: Some(RuntimeBinding {
            boot_id: None,
            provision_generation: Some(1),
            runtime_id: "runtime-title".into(),
            runtime_version: "cma-runtime:dev".into(),
            bound_at: NOW.into(),
        }),
        credential: SandboxCredentialState {
            source: Default::default(),
            service_grants: Vec::new(),
            service_scope: None,
            reaches: vec![ServiceId::new("chrono-llm")],
            key_generation: 3,
            delivered: true,
            current: Some(SandboxCredentialAuthority {
                generation: 3,
                provider_key_id: "title-key-3".into(),
                provider_state_version: 3,
                issued_at: NOW.into(),
                expires_at: "2036-09-05T00:00:00Z".into(),
                grant_fingerprint: "b".repeat(64),
            }),
            pending: None,
            revoke_debt: vec![],
            sign_in: SandboxSignInState::NotRequired,
        },
        work: SandboxWorkState::new(SandboxWorkStep::None, NOW.into()),
        limits: SandboxLimits {
            max_workspaces: 1,
            max_agents_per_workspace: 4,
        },
        lifecycle: SandboxLifecycleState {
            runtime_generation_high_water: 1,
            ..Default::default()
        },
        last_activity_at: NOW.into(),
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
        purge_after: None,
        revision: 1,
    }
}

pub(in crate::sandbox) fn agent() -> Agent {
    Agent {
        id: AgentId("agt_title".into()),
        worktree_id: WorktreeId("wkt_title".into()),
        workspace_id: WorkspaceId("wks_title".into()),
        sandbox_id: SandboxId("sbx_title".into()),
        owner: SubjectId::new("owner"),
        creation_key: "title-agent".into(),
        creation_fingerprint: "a".repeat(64),
        profile: AgentProfileSnapshot {
            id: AgentProfileId("agp_title".into()),
            revision: 1,
            digest: "a".repeat(64),
            content: AgentProfileContent {
                isolated_agent: false,
                model: Some("agent-selected-model".into()),
                thinking_effort: ThinkingEffort::Medium,
                permission_level: PermissionLevel::Guarded,
                skills: vec![],
                system_prompt: String::new(),
            },
        },
        capabilities: RuntimeCapabilities::new([RuntimeCapability::Conversation]),
        status: AgentStatus::Ready,
        conversation: ConversationState {
            turn_timeout: None,
            preferences: Default::default(),
            managed_guidance_revision: 0,
            observations: Default::default(),
            settings: None,
            conversation_id: Some("conversation-title".into()),
            runtime_generation: Some(1),
            last_seq: 0,
            first_message: None,
            active_turn: None,
            operation: None,
            queued_messages: vec![],
        },
        work: AgentWorkState::new(AgentWorkStep::None, NOW.into()),
        failure: None,
        title: None,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        ready_at: Some(NOW.into()),
        stopped_at: None,
        requested_stop_reason: None,
        stop_reason: None,
        deleted_at: None,
        origin: AgentOrigin::Manual,
        revision: 1,
    }
}
