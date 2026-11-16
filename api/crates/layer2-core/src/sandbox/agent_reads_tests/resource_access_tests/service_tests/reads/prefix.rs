use super::*;
use crate::sandbox::store::{AgentReadQueryStore, Page, SandboxStoreError};

struct Probe {
    inner: Arc<InMemorySandboxStore>,
    mode: &'static str,
    proofs: AtomicUsize,
    prefixes: AtomicUsize,
    tails: AtomicUsize,
    gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
}

macro_rules! entity {
    ($store:ident, $entity:ty, $id:ty, $create:ident, $get:ident, $save:ident, $list:ident, $parent:ty) => {
        #[async_trait]
        impl $store for Probe {
            async fn $create(&self, _: $entity) -> Result<(), SandboxStoreError> {
                panic!("read wrote")
            }
            async fn $get(&self, id: &$id) -> Result<Option<$entity>, SandboxStoreError> {
                self.inner.$get(id).await
            }
            async fn $save(&self, _: u64, _: $entity) -> Result<(), SandboxStoreError> {
                panic!("read wrote")
            }
            async fn $list(
                &self,
                _: &$parent,
                _: &PageRequest,
            ) -> Result<Page<$entity>, SandboxStoreError> {
                panic!("read listed")
            }
        }
    };
}
entity!(
    SandboxStore,
    Sandbox,
    SandboxId,
    create_sandbox,
    get_sandbox,
    save_sandbox,
    list_sandboxes,
    SubjectId
);
entity!(
    WorkspaceStore,
    Workspace,
    WorkspaceId,
    create_workspace,
    get_workspace,
    save_workspace,
    list_workspaces,
    SandboxId
);
entity!(
    WorktreeStore,
    Worktree,
    WorktreeId,
    create_worktree,
    get_worktree,
    save_worktree,
    list_worktrees,
    WorkspaceId
);
entity!(
    AgentStore,
    Agent,
    AgentId,
    create_agent,
    get_agent,
    save_agent,
    list_agents,
    SubjectId
);

#[async_trait]
impl AgentReadQueryStore for Probe {
    async fn list_owned_agents(
        &self,
        _: &SubjectId,
        _: &AgentListQuery,
        _: &PageRequest,
    ) -> Result<Page<Agent>, SandboxStoreError> {
        panic!("read listed")
    }
}

#[async_trait]
impl AgentLogStore for Probe {
    async fn append_agent_log(
        &self,
        _: &AgentId,
        _: String,
        _: AgentLogKind,
    ) -> Result<AgentLogEntry, SandboxStoreError> {
        panic!("read appended")
    }
    async fn agent_log(
        &self,
        _: &AgentId,
        _: u64,
        _: u16,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        panic!("unexpected page")
    }
    async fn agent_log_transcript(
        &self,
        _: &Agent,
    ) -> Result<Option<Vec<AgentLogEntry>>, SandboxStoreError> {
        Ok(None)
    }
    async fn agent_log_prefix(
        &self,
        id: &AgentId,
        through: u64,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        self.prefixes.fetch_add(1, Ordering::SeqCst);
        self.inner.agent_log_prefix(id, through).await
    }
    async fn agent_log_entry(
        &self,
        id: &AgentId,
        sequence: u64,
    ) -> Result<Option<AgentLogEntry>, SandboxStoreError> {
        self.tails.fetch_add(1, Ordering::SeqCst);
        self.inner.agent_log_entry(id, sequence).await
    }
    async fn agent_log_prefix_with_tail(
        &self,
        agent: &Agent,
    ) -> Result<Option<Vec<AgentLogEntry>>, SandboxStoreError> {
        self.proofs.fetch_add(1, Ordering::SeqCst);
        if self.mode == "unsupported" {
            return Ok(None);
        }
        if self.mode == "failed" {
            return Err(SandboxStoreError::Unavailable("proof unavailable".into()));
        }
        let mut rows = self
            .inner
            .agent_log_prefix(&agent.id, agent.conversation.last_seq)
            .await?;
        match self.mode {
            "short" => {
                rows.pop();
            }
            "sequence" => rows[1].seq = 1,
            "identity" => rows[1].agent_id = AgentId("foreign".into()),
            "creation" => {
                rows[0].kind = AgentLogKind::ModeChanged {
                    mode: "invalid".into(),
                }
            }
            "fold" => rows[2].kind = AgentLogKind::TurnCompleted { turn_sequence: 999 },
            "valid" => {}
            _ => panic!("unknown proof mode"),
        }
        if let Some((entered, resume)) = &self.gate {
            entered.notify_one();
            resume.notified().await;
        }
        Ok(Some(rows))
    }
}

impl Probe {
    fn new(inner: Arc<InMemorySandboxStore>, mode: &'static str) -> Self {
        Self {
            inner,
            mode,
            proofs: AtomicUsize::new(0),
            prefixes: AtomicUsize::new(0),
            tails: AtomicUsize::new(0),
            gate: None,
        }
    }
    fn reads(self: &Arc<Self>, access: ResourceAccessService) -> Arc<AgentReadService> {
        AgentReadService::new(
            self.clone(),
            Arc::new(StubResources::reporting(SandboxResourceReading::default())),
        )
        .with_resource_access(Arc::new(access))
    }
    fn counts(&self) -> (usize, usize, usize) {
        (
            self.proofs.load(Ordering::SeqCst),
            self.prefixes.load(Ordering::SeqCst),
            self.tails.load(Ordering::SeqCst),
        )
    }
}

#[tokio::test]
async fn complete_prefix_proof_preserves_transcript_usage_and_unsupported_fallback() {
    let (store, _, access, _) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let agent = history(&store).await;
    let owner = context("owner");
    let legacy = Arc::new(Probe::new(store.clone(), "unsupported"));
    let reads = legacy.reads(access.clone());
    let transcript = reads
        .transcript_authorized(&owner, None, &agent.id)
        .await
        .unwrap();
    let usage = reads
        .usage_authorized(&owner, None, &agent.id)
        .await
        .unwrap();
    assert_eq!(legacy.counts(), (2, 2, 2));
    let proven = Arc::new(Probe::new(store, "valid"));
    let reads = proven.reads(access);
    assert_eq!(
        reads
            .transcript_authorized(&owner, None, &agent.id)
            .await
            .unwrap(),
        transcript
    );
    assert_eq!(
        reads
            .usage_authorized(&owner, None, &agent.id)
            .await
            .unwrap(),
        usage
    );
    assert_eq!(proven.counts(), (2, 0, 0));
    let futures = (0..20).map(|_| reads.transcript_authorized(&owner, None, &agent.id));
    for result in futures_util::future::join_all(futures).await {
        assert_eq!(result.unwrap(), transcript);
    }
    assert_eq!(
        proven.counts(),
        (22, 0, 0),
        "each request obtains its own proof"
    );
}

#[tokio::test]
async fn complete_prefix_failed_proof_and_invalid_output_never_fall_back() {
    let (store, _, access, _) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let agent = history(&store).await;
    for mode in [
        "failed", "short", "sequence", "identity", "creation", "fold",
    ] {
        let probe = Arc::new(Probe::new(store.clone(), mode));
        let reads = probe.reads(access.clone());
        assert!(
            reads
                .transcript_authorized(&context("owner"), None, &agent.id)
                .await
                .is_err(),
            "{mode}"
        );
        assert!(
            reads
                .usage_authorized(&context("owner"), None, &agent.id)
                .await
                .is_err(),
            "{mode}"
        );
        assert_eq!(probe.counts(), (2, 0, 0), "{mode}");
    }
}

#[tokio::test]
async fn complete_prefix_keeps_release_revocation_and_cancellation_fences() {
    for cancel in [false, true] {
        let (store, _, access, selector) = setup(Reply::Role(OrganizationRole::Viewer)).await;
        let agent = history(&store).await;
        let share = access
            .change_share(
                &context("owner"),
                Some("request-credential"),
                create(&selector, "grant"),
            )
            .await
            .unwrap()
            .share;
        let entered = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        let mut probe = Probe::new(store, "valid");
        probe.gate = Some((entered.clone(), resume.clone()));
        let probe = Arc::new(probe);
        let reads = probe.reads(access.clone());
        let task = tokio::spawn(async move {
            reads
                .transcript_authorized(&context("recipient"), Some("request-credential"), &agent.id)
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .unwrap();
        if cancel {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            access
                .change_share(&context("owner"), None, revoke(&selector, &share))
                .await
                .unwrap();
            resume.notify_one();
            assert!(matches!(
                task.await.unwrap(),
                Err(AgentReadError::Access(ResourceAccessError::Store(
                    ResourceAccessStoreError::Conflict
                )))
            ));
        }
        assert_eq!(probe.counts(), (1, 0, 0));
    }
}
