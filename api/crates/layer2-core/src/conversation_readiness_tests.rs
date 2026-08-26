//! Bounded readiness for a session's conversation plane.

use std::{sync::Arc, time::Duration};

use crate::commit_fault_support::CommitFaultStore;
use crate::conversation_support::{conversing, until_sent};
use crate::event::{Event, EventKind};
use crate::service::SessionService;
use crate::session::{SessionSpec, SessionStatus};
use crate::store::{SessionRecord, SessionStore};
use crate::test_support::{service_over_with_environments, spec, user, FakeAdapter, FakeStore};
use crate::{ConversationReadinessError, CoreError};

fn is_ready_commit(_record: &SessionRecord, events: &[Event]) -> bool {
    events.iter().any(|event| {
        matches!(
            event.kind,
            EventKind::StatusChanged {
                status: SessionStatus::Ready,
                ..
            }
        )
    })
}

fn conversation_service(store: Arc<CommitFaultStore>) -> (Arc<SessionService>, Arc<FakeAdapter>) {
    let adapter = Arc::new(FakeAdapter {
        capabilities: conversing(),
        ..FakeAdapter::default()
    });
    let profiles =
        crate::test_support::environments_defaulting_to(crate::environment::AgentConfiguration {
            kernel: crate::test_support::kernel(),
            mode: crate::options::SessionMode::Conversation,
            ..Default::default()
        });
    (
        service_over_with_environments(store, Arc::clone(&adapter), profiles),
        adapter,
    )
}

#[tokio::test]
async fn readiness_spans_the_durable_ready_and_live_publication_boundary() {
    let inner = Arc::new(FakeStore::default());
    let store = Arc::new(CommitFaultStore::new(Arc::clone(&inner), is_ready_commit));
    store.pause_after_next();
    let (service, adapter) = conversation_service(Arc::clone(&store));
    let caller = user("readiness-owner");
    let created = service
        .create_session(&caller, conversation_spec())
        .await
        .expect("session is recorded");
    store.wait_until_held().await;

    assert_eq!(
        inner
            .get(&created.id)
            .await
            .unwrap()
            .expect("ready record is durable")
            .session
            .status,
        SessionStatus::Ready
    );
    assert!(!service.entries.read().await.contains_key(&created.id));

    let mut waiting =
        Box::pin(service.await_conversable(&caller, &created.id, Duration::from_secs(2)));
    assert!(
        tokio::time::timeout(Duration::from_millis(20), waiting.as_mut())
            .await
            .is_err(),
        "a durable status alone must not report a usable conversation"
    );

    store.release();
    tokio::time::timeout(Duration::from_secs(2), waiting)
        .await
        .expect("live publication stays within the test bound")
        .expect("published conversation becomes usable");
    service
        .send_prompt(&caller, &created.id, "after readiness".into())
        .await
        .expect("the ready conversation accepts its prompt");
    let sent = until_sent(&adapter, |input| {
        matches!(input, layer3_adapter::ConversationInput::Prompt(_))
    })
    .await;
    assert_eq!(
        sent.iter()
            .filter(|input| matches!(input, layer3_adapter::ConversationInput::Prompt(_)))
            .count(),
        1,
        "readiness must not open or deliver a turn by itself"
    );
}

#[tokio::test]
async fn provisioning_that_misses_the_bound_is_typed_as_a_timeout() {
    let inner = Arc::new(FakeStore::default());
    let store = Arc::new(CommitFaultStore::new(inner, is_ready_commit));
    store.pause_next();
    let (service, _) = conversation_service(Arc::clone(&store));
    let caller = user("timeout-owner");
    let created = service
        .create_session(&caller, conversation_spec())
        .await
        .expect("session is recorded");
    store.wait_until_held().await;

    let result = service
        .await_conversable(&caller, &created.id, Duration::from_millis(20))
        .await;
    assert_eq!(
        result,
        Err(ConversationReadinessError::ProvisioningTimedOut)
    );
    assert!(!service.entries.read().await.contains_key(&created.id));

    store.release();
}

#[tokio::test]
async fn readiness_preserves_the_conversation_operation_ownership_boundary() {
    let store = Arc::new(CommitFaultStore::new(
        Arc::new(FakeStore::default()),
        is_ready_commit,
    ));
    let (service, _) = conversation_service(store);
    let owner = user("readiness-owner");
    let other = user("different-owner");
    let created = service
        .create_session(&owner, conversation_spec())
        .await
        .expect("session is recorded");

    assert_eq!(
        service
            .await_conversable(&other, &created.id, Duration::from_secs(1))
            .await,
        Err(ConversationReadinessError::Refused(CoreError::Forbidden))
    );
}

fn conversation_spec() -> SessionSpec {
    SessionSpec {
        requested_mode: Some(crate::options::SessionMode::Conversation),
        ..spec()
    }
}
