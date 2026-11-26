use super::*;
#[path = "response_observation.rs"]
mod observation;
use crate::sandbox::store::{AgentResponseLogStore, AgentResponseLogWindow};

struct Source {
    entries: Vec<AgentLogEntry>,
    mode: &'static str,
    calls: AtomicUsize,
    indexed_calls: AtomicUsize,
    indexed_delay: Duration,
    window_delay: Duration,
    gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
}

#[async_trait::async_trait]
impl AgentResponseLogStore for Source {
    async fn agent_output_reader(
        &self,
        _: &Agent,
        _: Option<u64>,
    ) -> Result<
        Option<Box<dyn crate::sandbox::store::indexed::AgentOutputReader + '_>>,
        SandboxStoreError,
    > {
        self.indexed_calls.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(self.indexed_delay).await;
        match self.mode {
            "indexed-quarantine" => Err(SandboxStoreError::Corrupt("quarantined index".into())),
            "indexed-unavailable" => Err(SandboxStoreError::Unavailable(
                "private source error".into(),
            )),
            _ => Ok(None),
        }
    }

    async fn agent_response_log_window(
        &self,
        agent: &Agent,
        turn: Option<u64>,
    ) -> Result<Option<AgentResponseLogWindow>, SandboxStoreError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(self.window_delay).await;
        if let Some((entered, resume)) = &self.gate {
            entered.notify_one();
            resume.notified().await;
        }
        if self.mode == "fallback" {
            return Ok(None);
        }
        if self.mode == "unavailable" {
            return Err(SandboxStoreError::Unavailable(
                "private source error".into(),
            ));
        }
        if self.mode == "quarantine" {
            return Err(SandboxStoreError::Corrupt("quarantined".into()));
        }
        let mut entries = if turn.is_some() {
            self.entries.clone()
        } else {
            vec![]
        };
        if self.mode == "gap" {
            entries.remove(1);
        }
        if self.mode == "model" {
            if let AgentLogKind::TurnStarted { model, .. } = &mut entries[0].kind {
                *model = None;
            }
        }
        if self.mode == "truncated" {
            entries.pop();
        }
        if self.mode == "foreign" {
            entries[0].agent_id = AgentId("foreign".into());
        }
        Ok(Some(AgentResponseLogWindow {
            through: agent.conversation.last_seq - u64::from(self.mode == "head"),
            turn_sequence: if self.mode == "binding" {
                Some(4)
            } else {
                turn
            },
            entries,
        }))
    }
}

async fn source(store: &InMemorySandboxStore, id: &AgentId, mode: &'static str) -> Source {
    let mut entries = store.agent_log(id, 2, 200).await.unwrap();
    if let AgentLogKind::TurnStarted { model, .. } = &mut entries[0].kind {
        *model = Some("retained-model".into());
    }
    Source {
        entries,
        mode,
        calls: AtomicUsize::new(0),
        indexed_calls: AtomicUsize::new(0),
        indexed_delay: Duration::ZERO,
        window_delay: Duration::ZERO,
        gate: None,
    }
}

#[tokio::test]
async fn bounded_response_source_preserves_shared_redaction_and_unbound_state() {
    let (store, _, access, selector) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let agent = history(&store).await;
    access
        .change_share(
            &context("owner"),
            Some("request-credential"),
            create(&selector, "grant"),
        )
        .await
        .unwrap();
    let source = Arc::new(source(&store, &agent.id, "ready").await);
    let reads = service(store.clone())
        .with_response_logs(Some(source.clone()))
        .with_resource_access(Arc::new(access));
    let shared = reads
        .authorize_event_stream(&context("recipient"), Some("request-credential"), &agent.id)
        .await
        .unwrap();
    let snapshot = reads
        .response_event_stream_snapshot(&shared, Some(3))
        .await
        .unwrap();
    assert_eq!(snapshot.head, agent.conversation.last_seq);
    assert_eq!(snapshot.history.first().unwrap().seq, 3);
    for entry in &snapshot.history {
        assert!(!serde_json::to_string(&entry.kind)
            .unwrap()
            .contains("private-"));
    }
    let owner = reads
        .authorize_event_stream(&context("owner"), None, &agent.id)
        .await
        .unwrap();
    let private = reads
        .response_event_stream_snapshot(&owner, Some(3))
        .await
        .unwrap();
    assert!(private
        .history
        .iter()
        .any(|entry| serde_json::to_string(&entry.kind)
            .unwrap()
            .contains("private-")));
    let queued = reads
        .response_event_stream_snapshot(&owner, None)
        .await
        .unwrap();
    assert!(queued.history.is_empty());
    assert_eq!(queued.initial_model, agent.profile.content.model);
    // Unbound queued responses use the metadata-only projection and do not
    // need a response-source read; only the two bounded snapshots hit Mongo.
    assert_eq!(source.calls.load(Ordering::SeqCst), 2);
    assert_eq!(store.get_agent(&agent.id).await.unwrap().unwrap(), agent);
}

#[tokio::test]
async fn malformed_or_quarantined_response_proof_never_becomes_a_partial_success() {
    for mode in [
        "head",
        "binding",
        "gap",
        "model",
        "truncated",
        "foreign",
        "quarantine",
    ] {
        let (store, _, access, _) = setup(Reply::Role(OrganizationRole::Viewer)).await;
        let agent = history(&store).await;
        let original = store.agent_log(&agent.id, 0, 200).await.unwrap();
        let source = Arc::new(source(&store, &agent.id, mode).await);
        let reads = service(store.clone())
            .with_resource_access(Arc::new(access))
            .with_response_logs(Some(source));
        let held = reads
            .authorize_event_stream(&context("owner"), None, &agent.id)
            .await
            .unwrap();
        assert!(
            reads
                .response_event_stream_snapshot(&held, Some(3))
                .await
                .is_err(),
            "{mode}"
        );
        assert_eq!(store.agent_log(&agent.id, 0, 200).await.unwrap(), original);
    }
}

#[tokio::test]
async fn unsupported_response_source_keeps_the_complete_legacy_projection() {
    let (store, _, access, _) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let agent = history(&store).await;
    let reads = service(store.clone()).with_resource_access(Arc::new(access));
    let held = reads
        .authorize_event_stream(&context("owner"), None, &agent.id)
        .await
        .unwrap();
    let baseline = reads.event_stream_snapshot(&held).await.unwrap();
    let source = Arc::new(source(&store, &agent.id, "fallback").await);
    let reads = reads.with_response_logs(Some(source.clone()));
    let fallback = reads
        .response_event_stream_snapshot(&held, Some(3))
        .await
        .unwrap();
    assert_eq!(fallback.head, baseline.head);
    assert_eq!(fallback.history.len(), baseline.history.len());
    for (actual, expected) in fallback.history.iter().zip(&baseline.history) {
        assert_eq!((actual.seq, &actual.kind), (expected.seq, &expected.kind));
    }
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn revocation_while_response_source_waits_blocks_release_and_does_not_renew_authority() {
    let (store, authority, access, selector) = setup(Reply::Role(OrganizationRole::Viewer)).await;
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
    let mut source = source(&store, &agent.id, "ready").await;
    source.gate = Some((entered.clone(), resume.clone()));
    let reads = service(store.clone())
        .with_resource_access(Arc::new(access.clone()))
        .with_response_logs(Some(Arc::new(source)));
    let held = reads
        .authorize_event_stream(&context("recipient"), Some("request-credential"), &agent.id)
        .await
        .unwrap();
    let calls = authority.calls.load(Ordering::SeqCst);
    let (result, ()) = tokio::join!(
        reads.response_event_stream_snapshot(&held, Some(3)),
        async {
            entered.notified().await;
            access
                .change_share(&context("owner"), None, revoke(&selector, &share))
                .await
                .unwrap();
            resume.notify_one();
        }
    );
    assert!(result.is_err());
    assert_eq!(authority.calls.load(Ordering::SeqCst), calls);
}
