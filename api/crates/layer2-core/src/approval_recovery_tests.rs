//! Approval durability across control-plane replacement.
//!
//! Requests and resolutions are already durable. These tests establish that a
//! replacement process derives the runtime entitlement and answerable set from
//! those facts instead of from memory the previous process took with it.

use std::io::Write;
use std::sync::{Arc, Mutex as StdMutex};

use layer3_adapter::{ConversationInput, PermissionDecision};

use crate::conversation::ApprovalDecision;
use crate::conversation_support::{asking, conversing, until_logged, until_sent};
use crate::error::CoreError;
use crate::event::EventKind;
use crate::service::SessionService;
use crate::test_support::{await_settled, service, spec, user, FakeAdapter, FakeStore};

#[derive(Clone, Default)]
struct LogBuffer(Arc<StdMutex<Vec<u8>>>);

struct LogWriter(Arc<StdMutex<Vec<u8>>>);

impl Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("log buffer is not poisoned")
            .extend(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = LogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        LogWriter(Arc::clone(&self.0))
    }
}

fn replacement_service(store: Arc<FakeStore>, adapter: Arc<FakeAdapter>) -> Arc<SessionService> {
    let profiles =
        crate::test_support::environments_defaulting_to(crate::environment::AgentConfiguration {
            kernel: crate::test_support::kernel(),
            mode: crate::options::SessionMode::Conversation,
            ..Default::default()
        });
    crate::test_support::service_with_environments(store, adapter, profiles)
}

async fn ask(
    service: &Arc<SessionService>,
    caller: &crate::authz::Caller,
    id: &crate::ids::SessionId,
    adapter: &Arc<FakeAdapter>,
    request_id: &str,
) {
    {
        let held = adapter.conversation.lock().await;
        held.as_ref()
            .expect("the conversation is open")
            .updates
            .send(layer3_adapter::ConversationUpdate::Asked(asking(
                request_id,
                "run a command",
            )))
            .await
            .expect("the control plane is listening");
    }
    assert!(
        until_logged(service, caller, id, |kind| matches!(
            kind,
            EventKind::ApprovalRequested { request_id: logged, .. } if logged == request_id
        ))
        .await,
        "approval {request_id} must be durable before the process is replaced"
    );
}

#[tokio::test]
async fn terminal_readoption_does_not_depend_on_conversation_history() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let owner = user("terminal-history-outage-owner");
    let first = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = first.create_session(&owner, spec()).await.expect("created");
    await_settled(&first, &owner, &created.id).await;

    drop(first);
    *store.broken_event_reads.lock().await = true;
    let replacement = service(Arc::clone(&store), Arc::clone(&adapter));
    let recovered = replacement.reconcile().await;

    assert_eq!(recovered.readopted_sessions, 1);
    assert_eq!(recovered.deferred_sessions, 0);
    assert_eq!(recovered.reprovisioned_sessions, 0);
    assert!(
        adapter.terminated.lock().await.is_empty(),
        "conversation history availability must not affect a terminal runtime"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn only_outstanding_approvals_are_answerable_after_a_restart() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter {
        capabilities: conversing(),
        ..FakeAdapter::default()
    });
    let alice = user("approval-restart-owner");
    let first = replacement_service(Arc::clone(&store), Arc::clone(&adapter));
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;

    ask(
        &first,
        &alice,
        &created.id,
        &adapter,
        "resolved-before-restart",
    )
    .await;
    ask(
        &first,
        &alice,
        &created.id,
        &adapter,
        "pending-across-restart",
    )
    .await;
    first
        .resolve_approval(
            &alice,
            &created.id,
            "resolved-before-restart".into(),
            ApprovalDecision::Deny,
        )
        .await
        .expect("the first approval is answered before restart");
    assert!(
        until_logged(&first, &alice, &created.id, |kind| matches!(
            kind,
            EventKind::ApprovalResolved { request_id, .. }
                if request_id == "resolved-before-restart"
        ))
        .await
    );
    let waiting = first
        .get_session(&alice, &created.id)
        .await
        .expect("the waiting session is readable");
    assert!(
        waiting.wants_runtime(),
        "an idle approval wait still owns its blocked runtime"
    );

    drop(first);
    let replacement = replacement_service(Arc::clone(&store), Arc::clone(&adapter));
    let original_runtime = store
        .sessions
        .lock()
        .await
        .get(&created.id.0)
        .expect("the durable session remains")
        .runtime_id
        .clone();
    let rejoin_attempts = adapter.rejoined.lock().await.len();
    *store.broken_event_reads.lock().await = true;
    let deferred = replacement.reconcile().await;
    assert_eq!(deferred.deferred_sessions, 1);
    assert_eq!(deferred.readopted_sessions, 0);
    assert_eq!(deferred.reprovisioned_sessions, 0);
    assert_eq!(adapter.rejoined.lock().await.len(), rejoin_attempts);
    assert_eq!(
        store
            .sessions
            .lock()
            .await
            .get(&created.id.0)
            .expect("the durable session remains")
            .runtime_id
            .clone(),
        original_runtime,
        "an unavailable history must not orphan the runtime by clearing its identity"
    );
    assert!(
        adapter.terminated.lock().await.is_empty(),
        "an unavailable history cannot justify runtime teardown"
    );

    *store.broken_event_reads.lock().await = false;
    let logs = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(logs.clone())
        .finish();
    let _subscriber = tracing::subscriber::set_default(subscriber);
    let recovered = replacement.reconcile().await;
    assert_eq!(recovered.orphaned_runtimes, 0);
    assert_eq!(recovered.readopted_sessions, 1);
    assert!(
        adapter.terminated.lock().await.is_empty(),
        "reconciliation must not reclaim a runtime waiting for its owner"
    );
    let output = String::from_utf8(logs.0.lock().expect("log buffer is not poisoned").clone())
        .expect("logs are utf-8");
    assert!(
        output.contains("pending approvals restored during readoption")
            && output.contains("count=1"),
        "readoption must report the restored count: {output}"
    );
    assert!(
        !output.contains("pending-across-restart") && !output.contains("run a command"),
        "readoption logs must not carry approval identifiers or payloads: {output}"
    );

    for refused in ["resolved-before-restart", "never-requested"] {
        assert_eq!(
            replacement
                .resolve_approval(&alice, &created.id, refused.into(), ApprovalDecision::Allow,)
                .await,
            Err(CoreError::InvalidState),
            "{refused} must not be resurrected as pending"
        );
    }
    let before_answer = {
        let held = adapter.conversation.lock().await;
        held.as_ref()
            .expect("the conversation was readopted")
            .sent()
            .await
    };
    assert!(
        before_answer.is_empty(),
        "refused resolutions must reach neither the runtime nor its permission gate"
    );

    replacement
        .resolve_approval(
            &alice,
            &created.id,
            "pending-across-restart".into(),
            ApprovalDecision::Allow,
        )
        .await
        .expect("the outstanding approval remains answerable");
    let sent = until_sent(&adapter, |input| {
        matches!(input, ConversationInput::PermissionAnswer { .. })
    })
    .await;
    assert!(sent.iter().any(|input| matches!(
        input,
        ConversationInput::PermissionAnswer { id, decision }
            if id == "pending-across-restart" && *decision == PermissionDecision::Allow
    )));
    assert!(
        until_logged(&replacement, &alice, &created.id, |kind| matches!(
            kind,
            EventKind::ApprovalResolved {
                request_id,
                allowed: true,
            } if request_id == "pending-across-restart"
        ))
        .await,
        "the post-restart decision must join the same durable history"
    );
}
