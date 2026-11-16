//! Inject only at the durable publication boundary; all successful writes still
//! pass the real in-memory store's lifecycle, lease and log validation.
use super::*;
use layer2_core::sandbox::store::*;
use layer2_core::sandbox::{AgentLogEntry, Worktree};

#[derive(Clone, Copy, Debug)]
pub(super) enum Change {
    None,
    Title,
    Log,
    Holder,
    Fence,
    Step,
    Runtime,
    Stop,
    ParentPause,
    ParentRuntime,
    Workspace,
    Worktree,
    WorktreeRevision,
    DelayPastDue,
    RevokeAuthority,
    Expire,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Fault {
    Abort(Change),
    AlwaysAbort,
    Unavailable,
    CommittedUnavailable,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Boundary {
    Any,
    Failure,
    SkillResult,
}

pub(super) struct PublicationStore {
    pub inner: Arc<InMemorySandboxStore>,
    pub attempts: AtomicUsize,
    pub fault: Fault,
    pub boundary: Boundary,
}

impl PublicationStore {
    async fn change(&self, id: &AgentId, change: Change) {
        let mut agent = self.inner.get_agent(id).await.unwrap().unwrap();
        let revision = agent.revision;
        match change {
            Change::None => return,
            Change::RevokeAuthority => {
                layer2_core::agent_ownership::execution_authority()
                    .unwrap()
                    .revoke();
                return;
            }
            Change::Expire => {
                tokio::time::sleep(Duration::from_secs(31)).await;
                return;
            }
            Change::DelayPastDue => {
                tokio::time::sleep(Duration::from_secs(2)).await;
                return;
            }
            Change::Title => agent.title = Some("Concurrent owner title".into()),
            Change::Log => {
                self.inner
                    .append_agent_log(
                        id,
                        agent.updated_at.clone(),
                        AgentLogKind::TitleSet {
                            title: "Concurrent transcript title".into(),
                            source: "owner".into(),
                        },
                    )
                    .await
                    .unwrap();
                agent.conversation.last_seq += 1;
            }
            Change::Holder => agent.work.lease.as_mut().unwrap().holder = "other-AM".into(),
            Change::Fence => {
                agent.work.lease.as_mut().unwrap().fence += 1;
                agent.work.next_fence += 1;
            }
            Change::Step => agent.work.step = AgentWorkStep::CloseProcess,
            Change::Runtime => {
                agent.conversation.runtime_generation = Some(99);
                agent.conversation.conversation_id = Some("other-conversation".into());
            }
            Change::Stop => {
                self.inner
                    .schedule_agent_stop(AgentStopRequest {
                        target: AgentControlTarget::from(&agent),
                        reason: AgentStopReason::Owner,
                        requested_at: agent.updated_at.clone(),
                    })
                    .await
                    .unwrap();
                return;
            }
            Change::ParentPause | Change::ParentRuntime => {
                let mut parent = self
                    .inner
                    .get_sandbox(&agent.sandbox_id)
                    .await
                    .unwrap()
                    .unwrap();
                let revision = parent.revision;
                if matches!(change, Change::ParentPause) {
                    parent.begin_pause(agent.updated_at.clone(), None).unwrap();
                } else {
                    parent.runtime.as_mut().unwrap().runtime_id = "replacement-runtime".into();
                    parent.revision += 1;
                }
                self.inner.save_sandbox(revision, parent).await.unwrap();
                return;
            }
            Change::Workspace => {
                let mut workspace = self
                    .inner
                    .get_workspace(&agent.workspace_id)
                    .await
                    .unwrap()
                    .unwrap();
                let revision = workspace.revision;
                workspace.resolved_commit = Some("replacement-commit".into());
                workspace.revision += 1;
                self.inner
                    .save_workspace(revision, workspace)
                    .await
                    .unwrap();
                return;
            }
            Change::Worktree | Change::WorktreeRevision => {
                let mut worktree = self
                    .inner
                    .get_worktree(&agent.worktree_id)
                    .await
                    .unwrap()
                    .unwrap();
                let revision = worktree.revision;
                if matches!(change, Change::Worktree) {
                    worktree.base_commit = "replacement-worktree-base".into();
                }
                worktree.revision += 1;
                self.inner.save_worktree(revision, worktree).await.unwrap();
                return;
            }
        }
        agent.revision += 1;
        self.inner.save_agent(revision, agent).await.unwrap();
    }
}

#[async_trait::async_trait]
impl AgentWorkerStore for PublicationStore {
    async fn live_agent_connections(
        &self,
        page: &PageRequest,
    ) -> Result<Page<AgentConnectionRecoveryCandidate>, SandboxStoreError> {
        self.inner.live_agent_connections(page).await
    }
    async fn schedule_agent_connection_recovery(
        &self,
        request: AgentConnectionRecoveryRequest,
    ) -> Result<AgentConnectionRecoveryOutcome, SandboxStoreError> {
        self.inner.schedule_agent_connection_recovery(request).await
    }
    async fn due_agent_work(
        &self,
        now: &str,
        limit: u16,
    ) -> Result<Vec<AgentId>, SandboxStoreError> {
        self.inner.due_agent_work(now, limit).await
    }
    async fn claim_agent_work(
        &self,
        id: &AgentId,
        holder: &str,
        now: String,
        lease_expires_at: String,
    ) -> Result<Option<Agent>, SandboxStoreError> {
        self.inner
            .claim_agent_work(id, holder, now, lease_expires_at)
            .await
    }
    async fn commit_agent_work(
        &self,
        mutation: AgentWorkMutation,
    ) -> Result<(), SandboxStoreError> {
        let selected = match self.boundary {
            Boundary::Any => true,
            Boundary::Failure => mutation
                .agent_logs
                .iter()
                .any(|log| matches!(log, AgentLogKind::WorkStepFailed { .. })),
            Boundary::SkillResult => mutation
                .agent_logs
                .iter()
                .any(|log| matches!(log, AgentLogKind::SkillInstallationChanged { .. })),
        };
        if !selected {
            return self.inner.commit_agent_work(mutation).await;
        }
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
        if matches!(self.fault, Fault::AlwaysAbort) {
            eprintln!(
                "M3_PUBLICATION_ABORT step={:?} boundary={:?} attempt={}",
                mutation.agent.work.step,
                self.boundary,
                attempt + 1
            );
            return Err(SandboxStoreError::Conflict);
        }
        if attempt == 0 {
            match self.fault {
                Fault::Abort(change) => {
                    self.change(&mutation.agent.id, change).await;
                    // Printed after setup succeeds, immediately before the
                    // selected known-aborted result. No IDs or payloads escape.
                    eprintln!(
                        "M3_PUBLICATION_ABORT step={:?} boundary={:?} attempt={}",
                        mutation.agent.work.step,
                        self.boundary,
                        attempt + 1
                    );
                    return Err(SandboxStoreError::Conflict);
                }
                Fault::Unavailable => {
                    return Err(SandboxStoreError::Unavailable(
                        "publication result unknown".into(),
                    ))
                }
                Fault::CommittedUnavailable => {
                    self.inner.commit_agent_work(mutation).await?;
                    return Err(SandboxStoreError::Unavailable(
                        "commit acknowledgement lost".into(),
                    ));
                }
                Fault::AlwaysAbort => unreachable!(),
            }
        }
        self.inner.commit_agent_work(mutation).await
    }
}

// Only the six lifecycle interfaces are decorated. Admission, operations and
// recording retain the shared real store, rather than a second fake database.
macro_rules! forward {
    ($trait:ident { $(fn $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty;)* }) => {
        #[async_trait::async_trait]
        impl $trait for PublicationStore {
            $(async fn $name(&self, $($arg: $ty),*) -> $ret {
                self.inner.$name($($arg),*).await
            })*
        }
    };
}

forward!(SandboxStore {
    fn create_sandbox(sandbox: Sandbox) -> Result<(), SandboxStoreError>;
    fn get_sandbox(id: &SandboxId) -> Result<Option<Sandbox>, SandboxStoreError>;
    fn save_sandbox(expected_revision: u64, sandbox: Sandbox) -> Result<(), SandboxStoreError>;
    fn list_sandboxes(owner: &SubjectId, page: &PageRequest) -> Result<Page<Sandbox>, SandboxStoreError>;
});
forward!(WorkspaceStore {
    fn create_workspace(workspace: Workspace) -> Result<(), SandboxStoreError>;
    fn get_workspace(id: &WorkspaceId) -> Result<Option<Workspace>, SandboxStoreError>;
    fn save_workspace(expected_revision: u64, workspace: Workspace) -> Result<(), SandboxStoreError>;
    fn list_workspaces(sandbox_id: &SandboxId, page: &PageRequest) -> Result<Page<Workspace>, SandboxStoreError>;
});
forward!(WorktreeStore {
    fn create_worktree(worktree: Worktree) -> Result<(), SandboxStoreError>;
    fn get_worktree(id: &WorktreeId) -> Result<Option<Worktree>, SandboxStoreError>;
    fn save_worktree(expected_revision: u64, worktree: Worktree) -> Result<(), SandboxStoreError>;
    fn list_worktrees(workspace_id: &WorkspaceId, page: &PageRequest) -> Result<Page<Worktree>, SandboxStoreError>;
});
forward!(AgentStore {
    fn create_agent(agent: Agent) -> Result<(), SandboxStoreError>;
    fn get_agent(id: &AgentId) -> Result<Option<Agent>, SandboxStoreError>;
    fn save_agent(expected_revision: u64, agent: Agent) -> Result<(), SandboxStoreError>;
    fn list_agents(owner: &SubjectId, page: &PageRequest) -> Result<Page<Agent>, SandboxStoreError>;
    fn require_legacy_first_message(agent: &Agent) -> Result<(), SandboxStoreError>;
});
forward!(AgentLogStore {
    fn append_agent_log(agent_id: &AgentId, appended_at: String, kind: AgentLogKind) -> Result<AgentLogEntry, SandboxStoreError>;
    fn agent_log(agent_id: &AgentId, after: u64, limit: u16) -> Result<Vec<AgentLogEntry>, SandboxStoreError>;
});
