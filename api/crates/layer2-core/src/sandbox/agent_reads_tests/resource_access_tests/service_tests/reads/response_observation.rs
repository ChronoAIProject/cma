use super::*;
#[path = "response_observation_fixture.rs"]
mod fixture;
use crate::sandbox::{AgentEventStreamAuthorization, AgentResponseObservation};
use fixture::CountedAccess;
#[path = "response_observation_log.rs"]
mod log;
use log::CountedLog;
#[path = "response_observation_fences.rs"]
mod fences;
#[path = "response_observation_memo.rs"]
mod memo;

struct Fixture {
    store: Arc<InMemorySandboxStore>,
    agent: Agent,
    authority: Arc<Authority>,
    access: Arc<CountedAccess>,
    source: Arc<Source>,
    logs: Arc<CountedLog>,
    reads: Arc<AgentReadService>,
    held: AgentEventStreamAuthorization,
}

async fn counted(mode: &'static str, shared: bool) -> Fixture {
    let (store, authority, access, selector) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let agent = history(&store).await;
    if shared {
        access
            .change_share(
                &context("owner"),
                Some("request-credential"),
                create(&selector, "grant"),
            )
            .await
            .unwrap();
    }
    let access = Arc::new(CountedAccess {
        inner: store.clone(),
        delays: Mutex::new(vec![]),
        calls: AtomicUsize::new(0),
        gate: None,
    });
    let source = Arc::new(source(&store, &agent.id, mode).await);
    let logs = Arc::new(CountedLog {
        inner: store.clone(),
        calls: AtomicUsize::new(0),
        delay: Duration::ZERO,
        gate: None,
        fail: false,
    });
    let reads = AgentReadService::new(
        logs.clone(),
        Arc::new(StubResources::reporting(SandboxResourceReading::default())),
    )
    .with_resource_access(Arc::new(ResourceAccessService::new(
        access.clone(),
        authority.clone(),
    )))
    .with_response_logs(Some(source.clone()));
    let held = reads
        .authorize_event_stream(
            &context(if shared { "recipient" } else { "owner" }),
            shared.then_some("request-credential"),
            &agent.id,
        )
        .await
        .unwrap();
    access.calls.store(0, Ordering::SeqCst);
    Fixture {
        store,
        agent,
        authority,
        access,
        source,
        logs,
        reads,
        held,
    }
}

impl Fixture {
    async fn observe(&self) -> Result<AgentResponseObservation<()>, AgentReadError> {
        self.reads
            .with_response_observation(&self.held, 3, |_| Box::pin(async { Ok(()) }))
            .await
    }
    fn configure_source(&mut self, update: impl FnOnce(&mut Source)) {
        self.reads = self.reads.with_response_logs(None);
        update(Arc::get_mut(&mut self.source).unwrap());
        self.reads = self.reads.with_response_logs(Some(self.source.clone()));
    }
    fn configure_log(&mut self, update: impl FnOnce(&mut CountedLog)) {
        // Rebuild just the reader composition; retain the exact held grant.
        self.reads = service(self.store.clone());
        update(Arc::get_mut(&mut self.logs).unwrap());
        self.reads = AgentReadService::new(
            self.logs.clone(),
            Arc::new(StubResources::reporting(SandboxResourceReading::default())),
        )
        .with_resource_access(Arc::new(ResourceAccessService::new(
            self.access.clone(),
            self.authority.clone(),
        )))
        .with_response_logs(Some(self.source.clone()));
    }
}

#[tokio::test]
async fn legacy_source_baseline_counts_authority_snapshots() {
    for installed in [false, true] {
        let (store, authority, _, _) = setup(Reply::Role(OrganizationRole::Viewer)).await;
        let agent = history(&store).await;
        let access = Arc::new(CountedAccess {
            inner: store.clone(),
            delays: Mutex::new(vec![]),
            calls: AtomicUsize::new(0),
            gate: None,
        });
        let source = Arc::new(source(&store, &agent.id, "fallback").await);
        let reads = service(store.clone())
            .with_resource_access(Arc::new(ResourceAccessService::new(
                access.clone(),
                authority,
            )))
            .with_response_logs(installed.then_some(source.clone()));
        let held = reads
            .authorize_event_stream(&context("owner"), None, &agent.id)
            .await
            .unwrap();
        access.calls.store(0, Ordering::SeqCst);
        let indexed = reads
            .with_output_observation(&held, Some(3), |_| Box::pin(async { Ok(()) }))
            .await
            .unwrap();
        assert!(indexed.is_none());
        let result = reads
            .response_event_stream_snapshot(&held, Some(3))
            .await
            .unwrap();
        assert_eq!(result.head, agent.conversation.last_seq);
        assert_eq!(
            access.calls.load(Ordering::SeqCst),
            if installed { 6 } else { 4 }
        );
        assert_eq!(source.calls.load(Ordering::SeqCst), usize::from(installed));
        access.calls.store(0, Ordering::SeqCst);
        let combined = reads
            .with_response_observation(&held, 3, |_| Box::pin(async { Ok(()) }))
            .await
            .unwrap();
        let AgentResponseObservation::Legacy(combined) = combined else {
            panic!("legacy source")
        };
        assert_eq!(
            serde_json::to_value(combined).unwrap(),
            serde_json::to_value(result).unwrap()
        );
        assert_eq!(access.calls.load(Ordering::SeqCst), 2);
    }
}

#[tokio::test]
async fn combined_legacy_sources_keep_bytes_redaction_and_read_only_counts() {
    for mode in ["ready", "fallback"] {
        for shared in [false, true] {
            let f = counted(mode, shared).await;
            let target = ResourceTarget::Agent {
                agent_id: f.agent.id.clone(),
            };
            let before = f.store.resource_access_snapshot(&target).await.unwrap();
            let log = f.store.agent_log(&f.agent.id, 0, 200).await.unwrap();
            let expected = f
                .reads
                .response_event_stream_snapshot(&f.held, Some(3))
                .await
                .unwrap();
            f.access.calls.store(0, Ordering::SeqCst);
            f.source.calls.store(0, Ordering::SeqCst);
            f.logs.calls.store(0, Ordering::SeqCst);
            let AgentResponseObservation::Legacy(actual) = f.observe().await.unwrap() else {
                panic!("legacy source")
            };
            assert_eq!(
                serde_json::to_vec(&actual).unwrap(),
                serde_json::to_vec(&expected).unwrap()
            );
            assert_eq!(f.access.calls.load(Ordering::SeqCst), 2);
            assert_eq!(f.source.indexed_calls.load(Ordering::SeqCst), 1);
            assert_eq!(f.source.calls.load(Ordering::SeqCst), 1);
            assert_eq!(
                f.logs.calls.load(Ordering::SeqCst),
                usize::from(mode == "fallback")
            );
            assert_eq!(
                f.store.resource_access_snapshot(&target).await.unwrap(),
                before
            );
            assert_eq!(f.store.agent_log(&f.agent.id, 0, 200).await.unwrap(), log);
        }
    }
}

#[tokio::test]
async fn combined_corruption_and_dependency_failures_never_select_another_source() {
    for mode in [
        "indexed-quarantine",
        "indexed-unavailable",
        "unavailable",
        "head",
        "binding",
        "gap",
        "model",
        "truncated",
        "foreign",
        "quarantine",
    ] {
        let f = counted(mode, false).await;
        assert!(
            matches!(
                f.observe().await,
                Err(AgentReadError::Corrupt(_) | AgentReadError::Unavailable(_))
            ),
            "{mode}"
        );
        assert_eq!(f.access.calls.load(Ordering::SeqCst), 2, "{mode}");
        assert_eq!(
            f.source.calls.load(Ordering::SeqCst),
            usize::from(!mode.starts_with("indexed-")),
            "{mode}"
        );
        assert_eq!(f.logs.calls.load(Ordering::SeqCst), 0, "{mode}");
    }
}

#[tokio::test]
async fn combined_complete_source_still_checks_the_full_committed_history() {
    let f = counted("fallback", false).await;
    let mut agent = f.agent.clone();
    let revision = agent.revision;
    agent.revision += 1;
    agent.conversation.last_seq += 1;
    f.store.save_agent(revision, agent).await.unwrap();
    assert!(matches!(f.observe().await, Err(AgentReadError::Corrupt(_))));
    assert_eq!(f.access.calls.load(Ordering::SeqCst), 2);
    assert_eq!(f.logs.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn unbound_response_keeps_two_authority_reads_and_zero_source_reads() {
    let f = counted("quarantine", false).await;
    let snapshot = f
        .reads
        .response_event_stream_snapshot(&f.held, None)
        .await
        .unwrap();
    assert!(snapshot.history.is_empty());
    assert_eq!(snapshot.head, f.agent.conversation.last_seq);
    assert_eq!(f.access.calls.load(Ordering::SeqCst), 2);
    assert_eq!(f.source.indexed_calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.source.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.logs.calls.load(Ordering::SeqCst), 0);
}
