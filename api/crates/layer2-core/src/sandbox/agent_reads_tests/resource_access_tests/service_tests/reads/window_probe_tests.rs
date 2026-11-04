use super::*;
use crate::sandbox::store::{AgentReadQueryStore, Page, SandboxStoreError};

struct Probe {
    inner: Arc<InMemorySandboxStore>,
    calls: Mutex<Vec<(u64, u16)>>,
    range_calls: AtomicUsize,
    scan_visits: AtomicUsize,
    gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
    fail_after_gate: bool,
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
        if let Some(agent) = self.inner.get_agent(id).await? {
            // Model MongoSandboxStore's complete-log validation cost for the
            // reproduction: every legacy point read visits the full history.
            self.scan_visits
                .fetch_add(agent.conversation.last_seq as usize, Ordering::SeqCst);
        }
        self.inner.agent_log(id, after, limit).await
    }
    async fn agent_log_range(
        &self,
        id: &AgentId,
        after: u64,
        through: u64,
        limit: u16,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        self.range_calls.fetch_add(1, Ordering::SeqCst);
        if after == 0 {
            if let Some((entered, resume)) = &self.gate {
                entered.notify_one();
                resume.notified().await;
                if self.fail_after_gate {
                    return Err(SandboxStoreError::Unavailable(
                        "private dependency failure".into(),
                    ));
                }
            }
        }
        if let Some(agent) = self.inner.get_agent(id).await? {
            // A range page performs exactly one complete snapshot validation.
            self.scan_visits
                .fetch_add(agent.conversation.last_seq as usize, Ordering::SeqCst);
        }
        self.inner.agent_log_range(id, after, through, limit).await
    }
    async fn agent_log_entry(
        &self,
        id: &AgentId,
        sequence: u64,
    ) -> Result<Option<AgentLogEntry>, SandboxStoreError> {
        self.inner.agent_log_entry(id, sequence).await
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
        range_calls: AtomicUsize::new(0),
        scan_visits: AtomicUsize::new(0),
        gate: None,
        fail_after_gate: false,
    });
    let reads = reads(probe.clone(), access);
    let held = reads
        .authorize_event_stream(&context("owner"), None, &agent.id)
        .await
        .unwrap();
    let page = reads.event_stream_window(&held, 3, None, 2).await.unwrap();
    assert_eq!(page.entries.len(), 2);
    assert_eq!(*probe.calls.lock().unwrap(), Vec::<(u64, u16)>::new());
    assert_eq!(probe.range_calls.load(Ordering::SeqCst), 1);
    probe.calls.lock().unwrap().clear();
    let page = reads
        .event_stream_window(&held, agent.conversation.last_seq, None, 200)
        .await
        .unwrap();
    assert!(page.entries.is_empty());
    assert_eq!(*probe.calls.lock().unwrap(), Vec::<(u64, u16)>::new());
    assert_eq!(probe.range_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn production_shaped_history_validates_once_per_page_at_100_and_200_rows() {
    let (store, _, access, _) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let initial = store
        .get_agent(&AgentId("agent".into()))
        .await
        .unwrap()
        .unwrap();
    let kinds = (0..1_285)
        .map(|index| AgentLogKind::AgentMessage {
            content: if index % 127 == 0 {
                "large-event-".to_owned() + &"x".repeat(4 * 1024)
            } else {
                format!("event-{index}")
            },
        })
        .collect();
    let agent = replace_history(
        &store,
        initial,
        kinds,
        Some("production-shaped"),
        Some(7),
        None,
    )
    .await;
    assert_eq!(agent.conversation.last_seq, 1_286);
    let probe = Arc::new(Probe {
        inner: store,
        calls: Mutex::new(vec![]),
        range_calls: AtomicUsize::new(0),
        scan_visits: AtomicUsize::new(0),
        gate: None,
        fail_after_gate: false,
    });
    let reads = reads(probe.clone(), access);
    let held = reads
        .authorize_event_stream(&context("owner"), None, &agent.id)
        .await
        .unwrap();
    for offset in 0..100 {
        probe.agent_log(&agent.id, offset, 1).await.unwrap();
    }
    assert_eq!(probe.calls.lock().unwrap().len(), 100);
    assert_eq!(probe.scan_visits.load(Ordering::SeqCst), 100 * 1_286);
    probe.calls.lock().unwrap().clear();
    probe.scan_visits.store(0, Ordering::SeqCst);
    assert_eq!(
        reads
            .event_stream_window(&held, 0, None, 100)
            .await
            .unwrap()
            .entries
            .len(),
        100
    );
    assert_eq!(
        reads
            .event_stream_window(&held, 0, None, 200)
            .await
            .unwrap()
            .entries
            .len(),
        200
    );
    assert_eq!(
        reads
            .event_stream_window(&held, 1_186, Some(1_286), 200)
            .await
            .unwrap()
            .entries
            .len(),
        100
    );
    assert!(reads
        .event_stream_window(&held, 1_286, Some(1_286), 200)
        .await
        .unwrap()
        .entries
        .is_empty());
    assert_eq!(probe.range_calls.load(Ordering::SeqCst), 4);
    assert_eq!(probe.scan_visits.load(Ordering::SeqCst), 4 * 1_286);
    assert!(probe.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn bounded_event_windows_keep_the_captured_head_when_append_races_range_read() {
    let (store, _, access, _) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let agent = history(&store).await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    let probe = Arc::new(Probe {
        inner: store.clone(),
        calls: Mutex::new(vec![]),
        range_calls: AtomicUsize::new(0),
        scan_visits: AtomicUsize::new(0),
        gate: Some((entered.clone(), resume.clone())),
        fail_after_gate: false,
    });
    let reads = reads(probe.clone(), access);
    let held = reads
        .authorize_event_stream(&context("owner"), None, &agent.id)
        .await
        .unwrap();
    let pending = tokio::spawn(async move { reads.event_stream_window(&held, 0, None, 2).await });
    entered.notified().await;

    store
        .append_agent_log(
            &agent.id,
            NOW.into(),
            AgentLogKind::TitleSet {
                title: "concurrent append".into(),
                source: "owner".into(),
            },
        )
        .await
        .unwrap();
    let mut current = agent.clone();
    current.conversation.last_seq += 1;
    current.revision += 1;
    store.save_agent(agent.revision, current).await.unwrap();
    resume.notify_one();

    let page = pending.await.unwrap().unwrap();
    assert_eq!(page.through, agent.conversation.last_seq);
    assert_eq!(page.entries.len(), 2);
    assert_eq!(probe.range_calls.load(Ordering::SeqCst), 1);
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
        range_calls: AtomicUsize::new(0),
        scan_visits: AtomicUsize::new(0),
        gate: None,
        fail_after_gate: false,
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
        0,
        "the range read and tail check avoid point scans"
    );
    assert_eq!(probe.range_calls.load(Ordering::SeqCst), 1);
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
        range_calls: AtomicUsize::new(0),
        scan_visits: AtomicUsize::new(0),
        gate: Some((entered.clone(), resume.clone())),
        fail_after_gate: true,
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
        range_calls: AtomicUsize::new(0),
        scan_visits: AtomicUsize::new(0),
        gate: Some((entered.clone(), Arc::new(tokio::sync::Notify::new()))),
        fail_after_gate: false,
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
