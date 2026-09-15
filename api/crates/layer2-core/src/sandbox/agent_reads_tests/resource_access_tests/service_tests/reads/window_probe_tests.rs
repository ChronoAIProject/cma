use super::*;
use crate::sandbox::store::{AgentReadQueryStore, Page, SandboxStoreError};

struct Probe {
    inner: Arc<InMemorySandboxStore>,
    calls: Mutex<Vec<(u64, u16)>>,
    gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
}

macro_rules! delegate_entity {
    ($store:ident, $entity:ty, $id:ty, $create:ident, $get:ident, $save:ident, $list:ident, $parent:ty) => {
        #[async_trait]
        impl $store for Probe {
            async fn $create(&self, _: $entity) -> Result<(), SandboxStoreError> {
                unreachable!("read path mutated an entity")
            }
            async fn $get(&self, id: &$id) -> Result<Option<$entity>, SandboxStoreError> {
                self.inner.$get(id).await
            }
            async fn $save(&self, _: u64, _: $entity) -> Result<(), SandboxStoreError> {
                unreachable!("read path mutated an entity")
            }
            async fn $list(
                &self,
                _: &$parent,
                _: &PageRequest,
            ) -> Result<Page<$entity>, SandboxStoreError> {
                unreachable!("event window listed entities")
            }
        }
    };
}
delegate_entity!(
    SandboxStore,
    Sandbox,
    SandboxId,
    create_sandbox,
    get_sandbox,
    save_sandbox,
    list_sandboxes,
    SubjectId
);
delegate_entity!(
    WorkspaceStore,
    Workspace,
    WorkspaceId,
    create_workspace,
    get_workspace,
    save_workspace,
    list_workspaces,
    SandboxId
);
delegate_entity!(
    WorktreeStore,
    Worktree,
    WorktreeId,
    create_worktree,
    get_worktree,
    save_worktree,
    list_worktrees,
    WorkspaceId
);
delegate_entity!(
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
        unreachable!("event window listed Agents")
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
        unreachable!("event window appended history")
    }
    async fn agent_log(
        &self,
        id: &AgentId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        self.calls.lock().unwrap().push((after, limit));
        if after == 0 {
            if let Some((entered, resume)) = &self.gate {
                entered.notify_one();
                resume.notified().await;
                return Err(SandboxStoreError::Unavailable(
                    "private dependency failure".into(),
                ));
            }
        }
        self.inner.agent_log(id, after, limit).await
    }
    async fn agent_log_prefix(
        &self,
        _: &AgentId,
        _: u64,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        panic!("bounded replay must never fetch the complete prefix")
    }
}

fn reads(probe: Arc<Probe>, access: ResourceAccessService) -> Arc<AgentReadService> {
    AgentReadService::new(
        probe,
        Arc::new(StubResources::reporting(SandboxResourceReading::default())),
    )
    .with_resource_access(Arc::new(access))
}

#[tokio::test]
async fn bounded_event_windows_never_read_the_preceding_prefix_or_an_unbounded_batch() {
    let (store, _, access, _) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let agent = history(&store).await;
    let probe = Arc::new(Probe {
        inner: store,
        calls: Mutex::new(vec![]),
        gate: None,
    });
    let reads = reads(probe.clone(), access);
    let held = reads
        .authorize_event_stream(&context("owner"), None, &agent.id)
        .await
        .unwrap();
    let page = reads.event_stream_window(&held, 3, None, 2).await.unwrap();
    assert_eq!(page.entries.len(), 2);
    assert_eq!(
        *probe.calls.lock().unwrap(),
        vec![(3, 1), (4, 1), (agent.conversation.last_seq, 1)]
    );
    probe.calls.lock().unwrap().clear();
    let page = reads
        .event_stream_window(&held, agent.conversation.last_seq, None, 200)
        .await
        .unwrap();
    assert!(page.entries.is_empty());
    assert_eq!(
        *probe.calls.lock().unwrap(),
        vec![(agent.conversation.last_seq, 1)]
    );
}

#[tokio::test]
async fn bounded_event_windows_stop_fetching_before_large_rows_fill_the_requested_count() {
    let (store, _, access, _) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let agent = history(&store).await;
    for _ in 0..5 {
        store
            .append_agent_log(
                &agent.id,
                NOW.into(),
                AgentLogKind::AgentMessage {
                    content: "x".repeat(1024 * 1024),
                },
            )
            .await
            .unwrap();
    }
    let mut current = agent.clone();
    current.conversation.last_seq += 5;
    current.revision += 1;
    store
        .save_agent(agent.revision, current.clone())
        .await
        .unwrap();
    let probe = Arc::new(Probe {
        inner: store,
        calls: Mutex::new(vec![]),
        gate: None,
    });
    let reads = reads(probe.clone(), access);
    let held = reads
        .authorize_event_stream(&context("owner"), None, &agent.id)
        .await
        .unwrap();
    let page = reads
        .event_stream_window(&held, agent.conversation.last_seq, None, 200)
        .await
        .unwrap();
    assert_eq!(page.entries.len(), 3);
    let calls = probe.calls.lock().unwrap();
    assert_eq!(
        calls.len(),
        5,
        "only three accepted rows, one deferred row, and the aggregate tail probe"
    );
    assert!(calls.iter().all(|(_, limit)| *limit == 1));
    assert!(!calls.contains(&(agent.conversation.last_seq + 4, 1)));
}

#[tokio::test]
async fn bounded_event_windows_fence_dependency_failures_after_policy_changes_during_io() {
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
    let probe = Arc::new(Probe {
        inner: store,
        calls: Mutex::new(vec![]),
        gate: Some((entered.clone(), resume.clone())),
    });
    let reads = reads(probe, access.clone());
    let held = reads
        .authorize_event_stream(&context("recipient"), Some("request-credential"), &agent.id)
        .await
        .unwrap();
    let pending = tokio::spawn(async move { reads.event_stream_window(&held, 0, None, 1).await });
    entered.notified().await;
    access
        .change_share(&context("owner"), None, revoke(&selector, &share))
        .await
        .unwrap();
    resume.notify_one();
    assert!(matches!(
        pending.await.unwrap(),
        Err(AgentReadError::Access(_))
    ));
}

#[tokio::test(start_paused = true)]
async fn bounded_event_windows_share_one_fifteen_second_io_deadline() {
    let (store, _, access, _) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let agent = history(&store).await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let probe = Arc::new(Probe {
        inner: store,
        calls: Mutex::new(vec![]),
        gate: Some((entered.clone(), Arc::new(tokio::sync::Notify::new()))),
    });
    let reads = reads(probe, access);
    let held = reads
        .authorize_event_stream(&context("owner"), None, &agent.id)
        .await
        .unwrap();
    let pending = tokio::spawn(async move { reads.event_stream_window(&held, 0, None, 200).await });
    entered.notified().await;
    tokio::time::advance(Duration::from_secs(15)).await;
    assert!(matches!(
        pending.await.unwrap(),
        Err(AgentReadError::Unavailable(_))
    ));
}
