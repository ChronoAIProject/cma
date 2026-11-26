use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use layer3_adapter::{
    AdapterError, ContentBlock, ConversationInput, ConversationInputSender, ConversationUpdate,
    PromptResponse, TurnStopReason,
};

use crate::profiles::{AgentProfileContent, AgentProfileSnapshot, PermissionLevel, ThinkingEffort};
use crate::sandbox::store::{
    AgentConversationMutation, AgentConversationStore, AgentLogStore, AgentStore, Page,
    PageRequest, SandboxCheckpointMutation, SandboxLifecycleExecutionStore,
    SandboxLifecycleMutation, SandboxStore, SandboxStoreError,
};
use crate::{
    AgentId, AgentProfileId, Authorization, Caller, Capability, RuntimeCapabilities,
    RuntimeCapability, SandboxId, SubjectId, WorkspaceId, WorktreeId,
};

use super::super::*;

pub(super) const NOW: &str = "2026-09-05T00:00:00Z";
pub(super) const LATER: &str = "2026-09-05T00:00:01Z";
pub(super) const LATEST: &str = "2026-09-05T00:00:02Z";
pub(super) const ACTIVE_TURN: u64 = 7;
pub(super) const APPROVAL_ID: &str = "approval-one";
pub(super) const APPROVAL_LOCATOR: &str = "opaque/runtime/locator";
pub(super) const WORKER: &str = "worker-one";

pub(super) const ALL_AGENT_STATUSES: [AgentStatus; 8] = [
    AgentStatus::Starting,
    AgentStatus::Ready,
    AgentStatus::Working,
    AgentStatus::WaitingApproval,
    AgentStatus::Paused,
    AgentStatus::Stopped,
    AgentStatus::Deleted,
    AgentStatus::Failed,
];

pub(super) fn owner() -> Caller {
    Caller::new(
        SubjectId::new("owner"),
        Authorization::new([Capability::UseOwnSandboxes]),
    )
}

pub(super) fn unauthorized_owner() -> Caller {
    Caller::new(SubjectId::new("owner"), Authorization::none())
}

pub(super) fn other_owner() -> Caller {
    Caller::new(
        SubjectId::new("other"),
        Authorization::new([Capability::UseOwnSandboxes]),
    )
}

pub(super) fn test_agent(status: AgentStatus, first_message: Option<&str>) -> Agent {
    Agent {
        id: AgentId("agt_operations".into()),
        worktree_id: WorktreeId("wkt_operations".into()),
        workspace_id: WorkspaceId("wks_operations".into()),
        sandbox_id: SandboxId("sbx_operations".into()),
        owner: SubjectId::new("owner"),
        creation_key: "manual:operations".into(),
        creation_fingerprint: "a".repeat(64),
        profile: AgentProfileSnapshot {
            id: AgentProfileId("agp_operations".into()),
            revision: 1,
            digest: "b".repeat(64),
            content: AgentProfileContent {
                isolated_agent: false,
                model: Some("model-a".into()),
                thinking_effort: ThinkingEffort::High,
                permission_level: PermissionLevel::Guarded,
                skills: Vec::new(),
                system_prompt: String::new(),
            },
        },
        capabilities: full_capabilities(),
        status,
        conversation: ConversationState {
            turn_timeout: None,
            preferences: Default::default(),
            managed_guidance_revision: 0,
            observations: Default::default(),
            settings: None,
            conversation_id: Some("conversation-operations".into()),
            runtime_generation: Some(3),
            last_seq: 0,
            first_message: first_message.map(|text| {
                FirstMessageRecord::new("origin-one", text, NOW).expect("valid first message")
            }),
            active_turn: matches!(status, AgentStatus::Working | AgentStatus::WaitingApproval)
                .then_some(ACTIVE_TURN),
            operation: None,
            queued_messages: Vec::new(),
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

pub(super) async fn seed_active_parent(store: &InMemorySandboxStore) -> Sandbox {
    let mut sandbox = crate::sandbox::lifecycle_test_support::sandbox();
    sandbox.id = SandboxId("sbx_operations".into());
    sandbox.revision = 1;
    store.create_sandbox(sandbox.clone()).await.unwrap();
    sandbox
}

pub(super) fn authorize_first_message_work(agent: &mut Agent, fence: u64) {
    agent.work.step = AgentWorkStep::DispatchFirstMessage;
    agent.work.next_fence = fence + 1;
    agent.work.lease = Some(AgentWorkLease {
        holder: WORKER.into(),
        fence,
        expires_at: "2026-09-05T00:05:00Z".into(),
    });
}

pub(super) fn work_authority(fence: u64) -> AgentWorkAuthority {
    AgentWorkAuthority::new(WORKER.into(), fence).unwrap()
}

pub(super) fn operation_record(intent: AgentOperationIntent) -> AgentOperationRecord {
    AgentOperationRecord::new(1, 3, ACTIVE_TURN, intent, NOW.into()).unwrap()
}

pub(super) fn full_capabilities() -> RuntimeCapabilities {
    RuntimeCapabilities::new([
        RuntimeCapability::Conversation,
        RuntimeCapability::Steer,
        RuntimeCapability::Interrupt,
    ])
}

pub(super) fn prompt_text(content: &[ContentBlock]) -> &str {
    match content {
        [ContentBlock::Text(text)] => &text.text,
        _ => panic!("expected one text block"),
    }
}

pub(super) fn recording_sender(
    result: Result<(), AdapterError>,
) -> (ConversationInputSender, Arc<Mutex<Vec<ConversationInput>>>) {
    let inputs = Arc::new(Mutex::new(Vec::new()));
    let captured = inputs.clone();
    let sender = ConversationInputSender::spawn(4, move |input| {
        captured.lock().unwrap().push(input);
        let result = result.clone();
        async move { result }
    });
    (sender, inputs)
}

pub(super) fn fence_checking_sender(
    store: Arc<InMemorySandboxStore>,
    id: AgentId,
    fence_seen: Arc<AtomicBool>,
    inputs: Arc<Mutex<Vec<ConversationInput>>>,
) -> ConversationInputSender {
    ConversationInputSender::spawn(2, move |input| {
        let store = store.clone();
        let id = id.clone();
        let fence_seen = fence_seen.clone();
        let inputs = inputs.clone();
        async move {
            inputs.lock().unwrap().push(input);
            let agent = store
                .get_agent(&id)
                .await
                .map_err(|error| AdapterError::Rejected(error.to_string()))?
                .ok_or_else(|| AdapterError::Rejected("agent absent".into()))?;
            let fenced =
                agent.status == AgentStatus::Working
                    && agent.conversation.operation.as_ref().is_some_and(|record| {
                        record.state == AgentOperationDelivery::DispatchFenced
                    });
            fence_seen.store(fenced, Ordering::SeqCst);
            fenced
                .then_some(())
                .ok_or_else(|| AdapterError::Rejected("durable fence absent".into()))
        }
    })
}

pub(super) fn completing_sender(
    store: Arc<InMemorySandboxStore>,
    id: AgentId,
    cancelled: bool,
) -> ConversationInputSender {
    let recorder = DurableAgentUpdateRecorder::new(store);
    ConversationInputSender::spawn(2, move |_| {
        let recorder = recorder.clone();
        let id = id.clone();
        async move {
            let reason = if cancelled {
                TurnStopReason::Cancelled
            } else {
                TurnStopReason::EndTurn
            };
            recorder
                .record_update(
                    &id,
                    3,
                    &mut AgentMessageAssembly::default(),
                    ConversationUpdate::TurnEnded(PromptResponse::new(reason)),
                    LATEST.into(),
                )
                .await
                .map_err(|error| AdapterError::Rejected(error.to_string()))?;
            Ok(())
        }
    })
}

pub(super) async fn seed_waiting_approval(store: &InMemorySandboxStore) -> Agent {
    seed_active_parent(store).await;
    let working = test_agent(AgentStatus::Working, None);
    store.create_agent(working.clone()).await.unwrap();
    let mut waiting = working.clone();
    waiting
        .transition(AgentStatus::WaitingApproval, LATER.into(), None)
        .unwrap();
    waiting.conversation.last_seq = 2;
    store
        .commit_agent_conversation(AgentConversationMutation {
            settings_fence: None,
            command_claim: None,
            execution_fence: None,
            trigger_execution_fence: None,
            admission_guard: None,
            expected_agent_revision: working.revision,
            expected_last_seq: 0,
            runtime_generation: 3,
            agent: waiting.clone(),
            appended_at: LATER.into(),
            agent_logs: vec![
                AgentLogKind::ApprovalRequested {
                    approval_id: APPROVAL_ID.into(),
                    action: Some("write file".into()),
                    detail: Some("src/lib.rs".into()),
                    runtime_locator: Some(APPROVAL_LOCATOR.into()),
                },
                AgentLogKind::StatusChanged {
                    from: AgentStatus::Working,
                    to: AgentStatus::WaitingApproval,
                },
            ],
        })
        .await
        .unwrap();
    waiting
}

pub(super) struct TestRuntimeRegistry {
    capabilities: RuntimeCapabilities,
    sender: ConversationInputSender,
    resolutions: AtomicUsize,
    fail_on_resolution: Option<usize>,
}

impl TestRuntimeRegistry {
    pub(super) fn new(
        capabilities: RuntimeCapabilities,
        sender: ConversationInputSender,
    ) -> Arc<Self> {
        Arc::new(Self {
            capabilities,
            sender,
            resolutions: AtomicUsize::new(0),
            fail_on_resolution: None,
        })
    }

    pub(super) fn fail_on_resolution(self: Arc<Self>, resolution: usize) -> Arc<Self> {
        Arc::new(Self {
            capabilities: self.capabilities.clone(),
            sender: self.sender.clone(),
            resolutions: AtomicUsize::new(0),
            fail_on_resolution: Some(resolution),
        })
    }

    pub(super) fn resolutions(&self) -> usize {
        self.resolutions.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl AgentRuntimeRegistry for TestRuntimeRegistry {
    async fn resolve(
        &self,
        target: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        let resolution = self.resolutions.fetch_add(1, Ordering::SeqCst) + 1;
        if self.fail_on_resolution == Some(resolution) {
            return Err(AgentRuntimeError::Unavailable("registry changed".into()));
        }
        if target.agent_id != AgentId("agt_operations".into())
            || target.conversation_id != "conversation-operations"
            || target.runtime_generation != 3
        {
            return Err(AgentRuntimeError::StaleGeneration);
        }
        Ok(AgentRuntimeConnection {
            capabilities: self.capabilities.clone(),
            input: self.sender.clone(),
        })
    }

    async fn fence(
        &self,
        _agent_id: &AgentId,
        _authority_fence: u64,
    ) -> Result<(), AgentRuntimeError> {
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct RejectingConversationStore {
    pub(super) inner: InMemorySandboxStore,
    pub(super) reject_commits: AtomicBool,
    pub(super) conflict_commits: AtomicBool,
    pub(super) commit_attempts: AtomicUsize,
}

#[async_trait]
impl AgentLogStore for RejectingConversationStore {
    async fn append_agent_log(
        &self,
        agent_id: &AgentId,
        appended_at: String,
        kind: AgentLogKind,
    ) -> Result<AgentLogEntry, SandboxStoreError> {
        self.inner
            .append_agent_log(agent_id, appended_at, kind)
            .await
    }

    async fn agent_log(
        &self,
        agent_id: &AgentId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        self.inner.agent_log(agent_id, after, limit).await
    }
}

#[async_trait]
impl AgentStore for RejectingConversationStore {
    async fn create_agent(&self, agent: Agent) -> Result<(), SandboxStoreError> {
        self.inner.create_agent(agent).await
    }

    async fn get_agent(&self, id: &AgentId) -> Result<Option<Agent>, SandboxStoreError> {
        self.inner.get_agent(id).await
    }

    async fn save_agent(
        &self,
        expected_revision: u64,
        agent: Agent,
    ) -> Result<(), SandboxStoreError> {
        self.inner.save_agent(expected_revision, agent).await
    }

    async fn list_agents(
        &self,
        owner: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<Agent>, SandboxStoreError> {
        self.inner.list_agents(owner, page).await
    }
}

#[async_trait]
impl SandboxStore for RejectingConversationStore {
    async fn create_sandbox(&self, sandbox: Sandbox) -> Result<(), SandboxStoreError> {
        self.inner.create_sandbox(sandbox).await
    }

    async fn get_sandbox(&self, id: &SandboxId) -> Result<Option<Sandbox>, SandboxStoreError> {
        self.inner.get_sandbox(id).await
    }

    async fn save_sandbox(
        &self,
        expected_revision: u64,
        sandbox: Sandbox,
    ) -> Result<(), SandboxStoreError> {
        self.inner.save_sandbox(expected_revision, sandbox).await
    }

    async fn list_sandboxes(
        &self,
        owner: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<Sandbox>, SandboxStoreError> {
        self.inner.list_sandboxes(owner, page).await
    }
}

#[async_trait]
impl SandboxLifecycleExecutionStore for RejectingConversationStore {
    async fn sandbox_agents(
        &self,
        sandbox_id: &SandboxId,
        page: &PageRequest,
    ) -> Result<Page<Agent>, SandboxStoreError> {
        self.inner.sandbox_agents(sandbox_id, page).await
    }

    async fn publish_sandbox_checkpoint(
        &self,
        mutation: SandboxCheckpointMutation,
    ) -> Result<(), SandboxStoreError> {
        self.inner.publish_sandbox_checkpoint(mutation).await
    }

    async fn commit_sandbox_lifecycle(
        &self,
        mutation: SandboxLifecycleMutation,
    ) -> Result<(), SandboxStoreError> {
        self.inner.commit_sandbox_lifecycle(mutation).await
    }
}

#[path = "fixtures_conversation_store.rs"]
mod conversation_store;
