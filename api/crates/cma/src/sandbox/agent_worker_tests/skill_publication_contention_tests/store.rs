//! Inject only at the durable publication boundary; all successful writes still
//! pass the real in-memory store's lifecycle, lease and log validation.
use super::*;
use layer2_core::sandbox::store::*;
use layer2_core::sandbox::{AgentLogEntry, Worktree};

#[derive(Clone, Copy, Debug)]
pub(super) enum Change {
    None,
    Title,
    PreferenceTitle,
    Settings,
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
    LeaseDeadline,
    Intent,
    Material,
    Delete,
    Expire,
    OwnerReplacement,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Fault {
    Abort(Change),
    AfterPin(Change),
    AlwaysAbort,
    Unavailable,
    CommittedUnavailable,
    Reconciled,
    ReadbackUnavailable,
}

pub(super) struct PublicationStore {
    pub inner: Arc<InMemorySandboxStore>,
    pub attempts: AtomicUsize,
    pub fault: Fault,
    pub ownership: Option<Arc<layer2_core::agent_ownership::test_support::MemoryOwnershipStore>>,
}

mod change;

#[async_trait::async_trait]
impl AgentWorkerStore for PublicationStore {
    async fn due_agent_work_filtered(
        &self,
        now: &str,
        limit: u16,
        filter: AgentWorkDiscovery<'_>,
    ) -> Result<Vec<AgentId>, SandboxStoreError> {
        self.inner.due_agent_work_filtered(now, limit, filter).await
    }
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
        let id = mutation.agent.id.clone();
        self.inner.commit_agent_work(mutation).await?;
        if let Fault::AfterPin(change) = self.fault {
            self.change(&id, change).await;
        }
        Ok(())
    }
    async fn commit_agent_skill_result(
        &self,
        mutation: AgentWorkMutation,
        guard: AgentSkillResultGuard,
    ) -> Result<(), SandboxStoreError> {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
        if matches!(self.fault, Fault::AlwaysAbort) {
            eprintln!(
                "SKILL_PUBLICATION_ABORT step={:?} attempt={}",
                mutation.agent.work.step,
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
                        "SKILL_PUBLICATION_ABORT step={:?} attempt={}",
                        mutation.agent.work.step,
                        attempt + 1
                    );
                    return Err(SandboxStoreError::Conflict);
                }
                Fault::ReadbackUnavailable => return Err(SandboxStoreError::Conflict),
                Fault::Reconciled => {
                    self.inner
                        .commit_agent_skill_result(mutation.clone(), guard.clone())
                        .await?;
                    assert_eq!(
                        self.inner.get_agent(&mutation.agent.id).await?,
                        Some(mutation.agent.clone())
                    );
                    let logs = self
                        .inner
                        .agent_log(
                            &mutation.agent.id,
                            mutation.agent.conversation.last_seq - mutation.agent_logs.len() as u64,
                            100,
                        )
                        .await?;
                    assert_eq!(
                        logs.iter()
                            .map(|entry| entry.kind.clone())
                            .collect::<Vec<_>>(),
                        mutation.agent_logs
                    );
                    return Ok(());
                }
                Fault::Unavailable => {
                    return Err(SandboxStoreError::Unavailable(
                        "publication result unknown".into(),
                    ))
                }
                Fault::CommittedUnavailable => {
                    self.inner
                        .commit_agent_skill_result(mutation, guard)
                        .await?;
                    return Err(SandboxStoreError::Unavailable(
                        "commit acknowledgement lost".into(),
                    ));
                }
                Fault::AfterPin(_) => {}
                Fault::AlwaysAbort => unreachable!(),
            }
        }
        self.inner.commit_agent_skill_result(mutation, guard).await
    }
}

mod forward;
