use super::*;
use crate::sandbox::store::{AgentReadQueryStore, Page, SandboxStoreError};

struct Probe {
    inner: Arc<InMemorySandboxStore>,
    supplied: Option<Page<Agent>>,
    lineage_reads: AtomicUsize,
    fail_lineage: bool,
}

macro_rules! entity {
    ($store:ident, $entity:ty, $id:ty, $create:ident, $get:ident, $save:ident, $list:ident, $parent:ty) => {
        #[async_trait]
        impl $store for Probe {
            async fn $create(&self, _: $entity) -> Result<(), SandboxStoreError> {
                panic!("list mutated a record")
            }
            async fn $get(&self, id: &$id) -> Result<Option<$entity>, SandboxStoreError> {
                self.lineage_reads.fetch_add(1, Ordering::SeqCst);
                if self.fail_lineage {
                    return Err(SandboxStoreError::Unavailable("lineage unavailable".into()));
                }
                self.inner.$get(id).await
            }
            async fn $save(&self, _: u64, _: $entity) -> Result<(), SandboxStoreError> {
                panic!("list mutated a record")
            }
            async fn $list(
                &self,
                _: &$parent,
                _: &PageRequest,
            ) -> Result<Page<$entity>, SandboxStoreError> {
                panic!("unexpected inventory")
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
        owner: &SubjectId,
        query: &AgentListQuery,
        page: &PageRequest,
    ) -> Result<Page<Agent>, SandboxStoreError> {
        match &self.supplied {
            Some(page) => Ok(page.clone()),
            None => self.inner.list_owned_agents(owner, query, page).await,
        }
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
        panic!("list appended history")
    }
    async fn agent_log(
        &self,
        _: &AgentId,
        _: u64,
        _: u16,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        panic!("list read history")
    }
}

impl Probe {
    fn new(inner: Arc<InMemorySandboxStore>) -> Self {
        Self {
            inner,
            supplied: None,
            lineage_reads: AtomicUsize::new(0),
            fail_lineage: false,
        }
    }
    fn reads(self: &Arc<Self>, authorized: bool) -> Arc<AgentReadService> {
        let reads = AgentReadService::new(
            self.clone(),
            Arc::new(StubResources::reporting(SandboxResourceReading::default())),
        );
        if authorized {
            reads.with_resource_access(Arc::new(ResourceAccessService::new(
                self.inner.clone(),
                Arc::new(Authority {
                    reply: Mutex::new(Reply::Outage),
                    calls: AtomicUsize::new(0),
                    gate: None,
                }),
            )))
        } else {
            reads
        }
    }
    fn count(&self) -> usize {
        self.lineage_reads.load(Ordering::SeqCst)
    }
}

#[tokio::test]
async fn authorized_pages_preserve_results_and_avoid_three_parent_reads_per_row() {
    let (store, _, _, _) = setup(Reply::Outage).await;
    insert_hierarchy(
        &store,
        "agent2",
        "worktree2",
        "workspace",
        "sandbox",
        "owner",
        AgentStatus::Starting,
    )
    .await;
    let current = Arc::new(Probe::new(store.clone()));
    let legacy = Arc::new(Probe::new(store));
    let owner = caller("owner", true);
    let query = AgentListQuery::default();
    let mut page = PageRequest::new(1, None).unwrap();
    for _ in 0..2 {
        let expected = legacy
            .reads(false)
            .list(&owner, &query, &page)
            .await
            .unwrap();
        let actual = current
            .reads(true)
            .list(&owner, &query, &page)
            .await
            .unwrap();
        assert_eq!(actual, expected);
        page.cursor = actual.next;
    }
    assert!(page.cursor.is_none());
    assert_eq!(legacy.count(), 6);
    println!(
        "two stable list rows: legacy parent gets={}, authorized parent gets={}",
        legacy.count(),
        current.count()
    );
    assert_eq!(current.count(), 0);
}

#[tokio::test]
async fn changed_list_row_keeps_fallback_validation_and_dependency_failures() {
    let (store, _, _, _) = setup(Reply::Outage).await;
    let old = store
        .get_agent(&AgentId("agent".into()))
        .await
        .unwrap()
        .unwrap();
    let mut changed = old.clone();
    changed.title = Some("new title after page selection".into());
    changed.revision += 1;
    store.save_agent(old.revision, changed).await.unwrap();
    let page = Page {
        items: vec![old],
        next: None,
    };
    for fail in [false, true] {
        let mut probe = Probe::new(store.clone());
        probe.supplied = Some(page.clone());
        probe.fail_lineage = fail;
        let probe = Arc::new(probe);
        let result = probe
            .reads(true)
            .list(
                &caller("owner", true),
                &AgentListQuery::default(),
                &PageRequest::default(),
            )
            .await;
        if fail {
            assert!(matches!(result, Err(AgentReadError::Unavailable(_))));
            assert_eq!(probe.count(), 1);
        } else {
            assert_eq!(result.unwrap(), page);
            assert_eq!(probe.count(), 3);
        }
    }
}

#[tokio::test]
async fn current_owner_ancestry_and_conversation_refusals_never_fall_back() {
    for fault in [
        "owner",
        "worktree_owner",
        "worktree_agent",
        "workspace",
        "sandbox",
        "missing_worktree",
        "conversation",
    ] {
        let store = Arc::new(InMemorySandboxStore::default());
        let owner = if fault == "owner" { "other" } else { "owner" };
        let root = sandbox("sandbox", owner);
        let mut space = workspace("workspace", "sandbox", owner);
        let mut tree = worktree("worktree", "workspace", "sandbox", "agent", owner);
        let mut current = agent(
            "agent",
            "worktree",
            "workspace",
            "sandbox",
            owner,
            AgentStatus::Starting,
        );
        match fault {
            "worktree_owner" => tree.owner = SubjectId::new("other"),
            "worktree_agent" => tree.agent_id = None,
            "workspace" => tree.workspace_id = WorkspaceId("different".into()),
            "sandbox" => space.sandbox_id = SandboxId("different".into()),
            "conversation" => current.conversation.runtime_generation = Some(1),
            _ => {}
        }
        store.create_sandbox(root).await.unwrap();
        store.create_workspace(space).await.unwrap();
        if fault != "missing_worktree" {
            store.create_worktree(tree).await.unwrap();
        }
        store.create_agent(current.clone()).await.unwrap();
        let mut probe = Probe::new(store);
        // A stale/invalid storage result must still pass current authorization.
        current.owner = SubjectId::new("owner");
        probe.supplied = Some(Page {
            items: vec![current],
            next: None,
        });
        let probe = Arc::new(probe);
        assert!(
            probe
                .reads(true)
                .list(
                    &caller("owner", true),
                    &AgentListQuery::default(),
                    &PageRequest::default(),
                )
                .await
                .is_err(),
            "accepted {fault}"
        );
        assert_eq!(probe.count(), 0, "authorization failure fell back: {fault}");
    }
}

#[tokio::test]
async fn matching_authorized_rows_still_enforce_filter_order_and_cursor() {
    let (store, _, _, _) = setup(Reply::Outage).await;
    let row = store
        .get_agent(&AgentId("agent".into()))
        .await
        .unwrap()
        .unwrap();
    for fault in [
        "filter",
        "order",
        "cursor",
        "limit",
        "owner",
        "conversation",
    ] {
        let mut probe = Probe::new(store.clone());
        let mut page = Page {
            items: vec![row.clone()],
            next: None,
        };
        let mut query = AgentListQuery::default();
        let mut request = PageRequest::new(1, None).unwrap();
        match fault {
            "filter" => query.status = Some(AgentStatus::Deleted),
            "order" => request.cursor = Some(row.id.to_string()),
            "cursor" => page.next = Some("wrong".into()),
            "limit" => page.items.push(row.clone()),
            "owner" => page.items[0].owner = SubjectId::new("other"),
            "conversation" => page.items[0].conversation.runtime_generation = Some(1),
            _ => unreachable!(),
        }
        probe.supplied = Some(page);
        let probe = Arc::new(probe);
        assert!(
            matches!(
                probe
                    .reads(true)
                    .list(&caller("owner", true), &query, &request)
                    .await,
                Err(AgentReadError::Corrupt(_))
            ),
            "accepted {fault}"
        );
    }
}
