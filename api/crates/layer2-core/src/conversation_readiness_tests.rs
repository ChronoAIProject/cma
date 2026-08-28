//! Bounded readiness for a session's conversation plane.

use std::{sync::Arc, time::Duration};

use crate::commit_fault_support::CommitFaultStore;
use crate::conversation_support::{conversing, until_sent};
use crate::event::{Event, EventKind};
use crate::service::SessionService;
use crate::session::{SessionSpec, SessionStatus};
use crate::store::{SessionRecord, SessionStore};
use crate::test_support::{
    await_settled, service_over_with_environments, spec, user, FakeAdapter, FakeStore,
};
use crate::{Authorization, Caller, ConversationReadinessError, CoreError};

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
    let service = conversation_service_over(store, Arc::clone(&adapter));
    (service, adapter)
}

fn conversation_service_over(
    store: Arc<CommitFaultStore>,
    adapter: Arc<FakeAdapter>,
) -> Arc<SessionService> {
    let profiles =
        crate::test_support::environments_defaulting_to(crate::environment::AgentConfiguration {
            kernel: crate::test_support::kernel(),
            mode: crate::options::SessionMode::Conversation,
            ..Default::default()
        });
    service_over_with_environments(store, adapter, profiles)
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
async fn readiness_confirms_a_hidden_runtime_lease_advance_before_publication() {
    let store = Arc::new(FakeStore::default());
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
    let service = crate::test_support::service_with_environments(
        Arc::clone(&store),
        Arc::clone(&adapter),
        profiles,
    );
    let caller = user("hidden-readiness-advance-owner");

    store.lose_runtime_advance_responses(1);
    let created = service
        .create_session(&caller, conversation_spec())
        .await
        .expect("session is recorded");
    service
        .await_conversable(&caller, &created.id, Duration::from_secs(2))
        .await
        .expect("the hidden readiness receipt does not prevent publication");

    let durable = store
        .get(&created.id)
        .await
        .expect("the session is readable")
        .expect("the session remains durable");
    let authority = store
        .runtime_authority(&created.id)
        .await
        .expect("runtime authority is readable")
        .expect("runtime authority remains durable");
    let entries = service.entries.read().await;
    let entry = entries
        .get(&created.id)
        .expect("the ready runtime is locally published");
    assert!(entry.publication_complete);
    assert_eq!(
        entry.runtime.as_ref().map(|runtime| runtime.0.as_str()),
        durable.runtime_id.as_deref()
    );
    assert_eq!(entry.runtime_generation, authority.generation);
    drop(entries);
    assert_eq!(adapter.running_count().await, 1);
    assert!(
        store.active_session_lease(&created.id).await.is_none(),
        "the confirmed ready boundary must be released after publication"
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

#[tokio::test]
async fn recovered_rotation_refuses_readiness_before_and_after_ready_is_durable() {
    let inner = Arc::new(FakeStore::default());
    let store = Arc::new(CommitFaultStore::new(Arc::clone(&inner), is_ready_commit));
    let (service, _) = conversation_service(store);
    let caller = user("recovered-rotation-readiness-owner");
    let created = service
        .create_session(&caller, conversation_spec())
        .await
        .expect("session is recorded");
    await_settled(&service, &caller, &created.id).await;

    service.entries.write().await.remove(&created.id);
    assert!(service.begin_start(&created.id).await);
    service
        .rotations_in_progress
        .write()
        .await
        .insert(created.id.clone());

    for status in [SessionStatus::Provisioning, SessionStatus::Ready] {
        let mut record = inner
            .get(&created.id)
            .await
            .expect("recovery state is readable")
            .expect("session remains durable");
        record.session.status = status;
        inner
            .seed(&record)
            .await
            .expect("recovery boundary is durable");

        let result = tokio::time::timeout(
            Duration::from_millis(100),
            service.await_conversable(&caller, &created.id, Duration::from_secs(2)),
        )
        .await
        .expect("effective rotation is refused without waiting");
        assert_eq!(
            result,
            Err(ConversationReadinessError::Refused(
                CoreError::SessionRotating
            )),
            "durable {status:?} remains an effective rotation until live publication"
        );
    }

    service.finish_start(&created.id).await;
}

#[tokio::test]
async fn a_live_conversation_precedes_a_trailing_start_claim() {
    let store = Arc::new(CommitFaultStore::new(
        Arc::new(FakeStore::default()),
        is_ready_commit,
    ));
    let (service, _) = conversation_service(store);
    let caller = user("published-owner");
    let created = service
        .create_session(&caller, conversation_spec())
        .await
        .expect("session is recorded");
    await_settled(&service, &caller, &created.id).await;
    assert!(
        service
            .entries
            .read()
            .await
            .get(&created.id)
            .and_then(|entry| entry.conversation.as_ref())
            .is_some(),
        "the conversation is live before the trailing claim is modeled"
    );
    assert!(service.begin_start(&created.id).await);

    let result = service
        .await_conversable(&caller, &created.id, Duration::from_millis(20))
        .await;
    service.finish_start(&created.id).await;

    assert_eq!(result, Ok(()));
}

#[tokio::test]
async fn an_owner_without_session_entitlement_cannot_use_conversation_input() {
    let store = Arc::new(CommitFaultStore::new(
        Arc::new(FakeStore::default()),
        is_ready_commit,
    ));
    let (service, _) = conversation_service(store);
    let owner = user("unentitled-owner");
    let created = service
        .create_session(&owner, conversation_spec())
        .await
        .expect("session is recorded");
    await_settled(&service, &owner, &created.id).await;
    let unentitled = Caller::new(owner.subject.clone(), Authorization::none());

    assert_eq!(
        service
            .await_conversable(&unentitled, &created.id, Duration::from_secs(1))
            .await,
        Err(ConversationReadinessError::Refused(CoreError::Forbidden))
    );
    assert_eq!(
        service
            .send_prompt(&unentitled, &created.id, "not authorized".into())
            .await,
        Err(CoreError::Forbidden)
    );
}

#[tokio::test]
async fn concurrent_readoption_waiters_open_one_adapter_channel() {
    let store = Arc::new(CommitFaultStore::new(
        Arc::new(FakeStore::default()),
        is_ready_commit,
    ));
    let (original, adapter) = conversation_service(Arc::clone(&store));
    let caller = user("concurrent-readoption-owner");
    let created = original
        .create_session(&caller, conversation_spec())
        .await
        .expect("session is recorded");
    await_settled(&original, &caller, &created.id).await;
    let opens_before_readoption = adapter.rejoined.lock().await.len();
    let replacement = conversation_service_over(Arc::clone(&store), Arc::clone(&adapter));

    let (first, second) = tokio::join!(
        replacement.await_conversable(&caller, &created.id, Duration::from_secs(1)),
        replacement.await_conversable(&caller, &created.id, Duration::from_secs(1)),
    );

    assert_eq!(first, Ok(()));
    assert_eq!(second, Ok(()));
    assert_eq!(
        adapter.rejoined.lock().await.len(),
        opens_before_readoption + 1,
        "concurrent waiters must share one readoption attempt"
    );
}

#[tokio::test]
async fn timed_out_readoption_continues_and_retry_does_not_open_again() {
    let store = Arc::new(CommitFaultStore::new(
        Arc::new(FakeStore::default()),
        is_ready_commit,
    ));
    let adapter = Arc::new(FakeAdapter {
        capabilities: conversing(),
        sign_in_on_open: true,
        ..FakeAdapter::default()
    });
    let original = conversation_service_over(Arc::clone(&store), Arc::clone(&adapter));
    let caller = user("timed-readoption-owner");
    let created = original
        .create_session(&caller, conversation_spec())
        .await
        .expect("session is recorded");
    await_settled(&original, &caller, &created.id).await;
    let opens_before_readoption = adapter.rejoined.lock().await.len();
    let replacement = conversation_service_over(store, Arc::clone(&adapter));

    assert_eq!(
        replacement
            .await_conversable(&caller, &created.id, Duration::from_millis(20))
            .await,
        Err(ConversationReadinessError::ProvisioningTimedOut)
    );
    assert_eq!(
        adapter.rejoined.lock().await.len(),
        opens_before_readoption + 1,
        "the adapter open began before the readiness wait expired"
    );

    assert_eq!(
        replacement
            .await_conversable(&caller, &created.id, Duration::from_secs(1))
            .await,
        Ok(())
    );
    assert_eq!(
        adapter.rejoined.lock().await.len(),
        opens_before_readoption + 1,
        "retry must wait for the owned readoption instead of opening again"
    );
}

fn conversation_spec() -> SessionSpec {
    SessionSpec {
        requested_mode: Some(crate::options::SessionMode::Conversation),
        ..spec()
    }
}
