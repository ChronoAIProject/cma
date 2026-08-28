//! Resource readings crossing into, and staying within, the core.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use layer3_adapter::{KernelAdapter, ResourceSample};
use tokio::sync::broadcast::error::TryRecvError;

use crate::authz::{Authorization, Caller};
use crate::conversation_support::chunk;
use crate::error::CoreError;
use crate::ids::{SessionId, SubjectId};
use crate::options::SessionMode;
use crate::resources::{reading_from, ResourceReading, ResourceStatus};
use crate::service::SessionService;
use crate::session::{Session, SessionStatus};
use crate::store::SessionStore;
use crate::test_support::{admin, await_settled, service, spec, user, FakeAdapter, FakeStore};

fn capable_fake() -> FakeAdapter {
    let mut fake = FakeAdapter::default();
    fake.capabilities.resources = true;
    fake
}

fn capable_adapter() -> Arc<FakeAdapter> {
    Arc::new(capable_fake())
}

fn conversation_adapter() -> Arc<FakeAdapter> {
    let mut fake = capable_fake();
    fake.capabilities.conversation = true;
    Arc::new(fake)
}

fn sample(n: u64) -> ResourceSample {
    ResourceSample {
        cpu_millis_used: Some(n as u32 + 1),
        cpu_millis_limit: Some(n as u32 + 2),
        memory_bytes_used: Some(n + 3),
        memory_bytes_limit: Some((n + 3) * 2),
        memory_bytes_peak: Some(n + 5),
        stall_permille: Some(n as u16 + 6),
        ceiling_hits: Some(n + 7),
        allocation_failures: Some(n + 8),
        kills: Some(n + 9),
        received_bytes: Some(n + 10),
        sent_bytes: Some(n + 11),
        received_bytes_per_second: Some(n + 12),
        sent_bytes_per_second: Some(n + 13),
        interval_ms: Some(n + 14),
    }
}

fn reading(n: u64) -> ResourceReading {
    reading_from(sample(n))
}

async fn ready(
    service: &Arc<SessionService>,
    caller: &Caller,
    mode: Option<SessionMode>,
) -> Session {
    let mut request = spec();
    request.requested_mode = mode;
    let created = service
        .create_session(caller, request)
        .await
        .expect("created");
    await_settled(service, caller, &created.id).await
}

async fn await_start_finished(service: &Arc<SessionService>, id: &SessionId) {
    for _ in 0..400 {
        if !service.starting.read().await.contains(id) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("session bring-up did not finish");
}

async fn resource_sender(adapter: &Arc<FakeAdapter>) -> tokio::sync::mpsc::Sender<ResourceSample> {
    for _ in 0..400 {
        if let Some(sender) = adapter.resource_sink.lock().await.clone() {
            return sender;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("resource watch was not opened");
}

async fn await_watch_calls(adapter: &Arc<FakeAdapter>, wanted: usize) {
    for _ in 0..400 {
        if adapter.resource_watch_calls.load(Ordering::SeqCst) >= wanted {
            tokio::task::yield_now().await;
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("resource watch was not attempted {wanted} times");
}

async fn await_held(
    service: &Arc<SessionService>,
    id: &SessionId,
    expected: &ResourceReading,
) -> crate::HeldReading {
    for _ in 0..400 {
        if let Some(held) = service.held_reading(id).await {
            if &held.reading == expected {
                return held;
            }
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("expected resource reading was not held");
}

async fn await_status(
    service: &Arc<SessionService>,
    caller: &Caller,
    id: &SessionId,
    wanted: fn(&ResourceStatus) -> bool,
) -> ResourceStatus {
    for _ in 0..400 {
        let status = service
            .last_reading(caller, id)
            .await
            .expect("point read succeeds");
        if wanted(&status) {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("resource status never reached the expected state");
}

#[test]
fn reading_translation_moves_all_fourteen_fields_without_inventing_values() {
    let mut reported = sample(20);
    reported.cpu_millis_limit = None;
    reported.memory_bytes_used = None;
    reported.received_bytes_per_second = None;

    assert_eq!(
        reading_from(reported),
        ResourceReading {
            cpu_millis_used: Some(21),
            cpu_millis_limit: None,
            memory_bytes_used: None,
            memory_bytes_limit: Some(46),
            memory_bytes_peak: Some(25),
            stall_permille: Some(26),
            ceiling_hits: Some(27),
            allocation_failures: Some(28),
            kills: Some(29),
            received_bytes: Some(30),
            sent_bytes: Some(31),
            received_bytes_per_second: None,
            sent_bytes_per_second: Some(33),
            interval_ms: Some(34),
        }
    );
}

#[tokio::test]
async fn ungranted_sessions_refuse_resource_reads_by_name_after_authorization() {
    let store = Arc::new(FakeStore::default());
    let service = service(store, Arc::new(FakeAdapter::default()));
    let alice = user("alice");
    let session = ready(&service, &alice, None).await;

    assert!(matches!(
        service.subscribe_resources(&alice, &session.id).await,
        Err(CoreError::CapabilityUnsupported(ref name)) if name == "resources"
    ));
    assert!(matches!(
        service.last_reading(&alice, &session.id).await,
        Err(CoreError::CapabilityUnsupported(ref name)) if name == "resources"
    ));

    let stranger = Caller::new(SubjectId("mallory".into()), Authorization::none());
    assert!(matches!(
        service.subscribe_resources(&stranger, &session.id).await,
        Err(CoreError::Forbidden)
    ));
    assert!(matches!(
        service.last_reading(&stranger, &session.id).await,
        Err(CoreError::Forbidden)
    ));
}

#[tokio::test]
async fn subscribe_live_on_a_terminal_session_still_requires_conversation() {
    let store = Arc::new(FakeStore::default());
    let service = service(store, capable_adapter());
    let alice = user("alice");
    let session = ready(&service, &alice, None).await;

    assert!(matches!(
        service.subscribe_live(&alice, &session.id).await,
        Err(CoreError::CapabilityUnsupported(ref name)) if name == "conversation"
    ));
}

#[tokio::test]
async fn read_all_sessions_can_subscribe_without_event_log_permission() {
    let store = Arc::new(FakeStore::default());
    let service = service(Arc::clone(&store), capable_adapter());
    let alice = user("alice");
    let session = ready(&service, &alice, None).await;
    let reader = admin();

    service
        .subscribe_resources(&reader, &session.id)
        .await
        .expect("session visibility is enough for current readings");
    assert!(matches!(
        service.events_since(&reader, &session.id, None).await,
        Err(CoreError::Forbidden)
    ));
}

#[tokio::test]
async fn a_resource_subscriber_receives_readings_in_order() {
    let store = Arc::new(FakeStore::default());
    let adapter = capable_adapter();
    let service = service(store, Arc::clone(&adapter));
    let alice = user("alice");
    let session = ready(&service, &alice, None).await;
    let mut receiver = service
        .subscribe_resources(&alice, &session.id)
        .await
        .expect("subscribed");
    let sender = resource_sender(&adapter).await;

    for n in 1..=3 {
        sender.send(sample(n)).await.expect("watch remains open");
    }
    for n in 1..=3 {
        let received = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .expect("reading arrived")
            .expect("subscriber kept pace");
        assert_eq!(received, reading(n));
    }
}

#[tokio::test]
async fn ten_readings_retain_only_the_tenth() {
    let store = Arc::new(FakeStore::default());
    let adapter = capable_adapter();
    let service = service(store, Arc::clone(&adapter));
    let alice = user("alice");
    let session = ready(&service, &alice, None).await;
    let sender = resource_sender(&adapter).await;

    for n in 1..=10 {
        sender.send(sample(n)).await.expect("watch remains open");
    }
    let held = await_held(&service, &session.id, &reading(10)).await;
    assert_eq!(held.reading, reading(10));
    assert!(!held.observed_at.is_empty());
}

#[tokio::test]
async fn point_reads_distinguish_held_awaiting_and_a_missing_entry() {
    let store = Arc::new(FakeStore::default());
    let adapter = capable_adapter();
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));
    let alice = user("alice");
    let session = ready(&svc, &alice, None).await;
    let sender = resource_sender(&adapter).await;

    assert_eq!(
        svc.last_reading(&alice, &session.id).await,
        Ok(ResourceStatus::Awaiting)
    );
    let restarted = service(store, capable_adapter());
    assert_eq!(restarted.held_reading(&session.id).await, None);
    assert_eq!(
        restarted.last_reading(&alice, &session.id).await,
        Ok(ResourceStatus::Awaiting)
    );

    sender.send(sample(40)).await.expect("watch remains open");
    await_held(&svc, &session.id, &reading(40)).await;
    assert!(matches!(
        svc.last_reading(&alice, &session.id).await,
        Ok(ResourceStatus::Held(ref held)) if held.reading == reading(40)
    ));
}

#[tokio::test]
async fn unsupported_and_transient_watches_do_not_fail_session_bring_up() {
    let refused = Arc::new(FakeAdapter {
        refuse_watch: true,
        ..capable_fake()
    });
    let refused_service = service(Arc::new(FakeStore::default()), Arc::clone(&refused));
    let alice = user("alice");
    let unreported = ready(&refused_service, &alice, None).await;
    await_watch_calls(&refused, 1).await;
    assert_eq!(unreported.status, SessionStatus::Ready);
    await_status(&refused_service, &alice, &unreported.id, |status| {
        matches!(status, ResourceStatus::Unreported)
    })
    .await;

    let failed = Arc::new(FakeAdapter {
        fail_watch: true,
        ..capable_fake()
    });
    let failed_service = service(Arc::new(FakeStore::default()), Arc::clone(&failed));
    let awaiting = ready(&failed_service, &alice, None).await;
    await_watch_calls(&failed, 1).await;
    assert_eq!(awaiting.status, SessionStatus::Ready);
    assert_eq!(
        failed_service.last_reading(&alice, &awaiting.id).await,
        Ok(ResourceStatus::Awaiting)
    );
}

#[tokio::test]
async fn resource_readings_never_reach_the_durable_log() {
    let store = Arc::new(FakeStore::default());
    let adapter = capable_adapter();
    let service = service(Arc::clone(&store), Arc::clone(&adapter));
    let alice = user("alice");
    let session = ready(&service, &alice, None).await;
    await_start_finished(&service, &session.id).await;
    let before_events = serde_json::to_value(
        service
            .events_since(&alice, &session.id, None)
            .await
            .expect("readable"),
    )
    .expect("events serialize");
    let before_seq = service
        .get_session(&alice, &session.id)
        .await
        .expect("readable")
        .last_seq;
    let sender = resource_sender(&adapter).await;

    for n in 1..=100 {
        sender.send(sample(n)).await.expect("watch remains open");
    }
    await_held(&service, &session.id, &reading(100)).await;

    let after_events = serde_json::to_value(
        service
            .events_since(&alice, &session.id, None)
            .await
            .expect("readable"),
    )
    .expect("events serialize");
    assert_eq!(after_events, before_events);
    assert_eq!(
        service
            .get_session(&alice, &session.id)
            .await
            .expect("readable")
            .last_seq,
        before_seq
    );
}

#[tokio::test]
async fn resource_and_live_frame_channels_have_independent_lag_budgets() {
    let store = Arc::new(FakeStore::default());
    let adapter = conversation_adapter();
    let service = service(store, Arc::clone(&adapter));
    let alice = user("alice");
    let session = ready(&service, &alice, Some(SessionMode::Conversation)).await;
    let mut live = service
        .subscribe_live(&alice, &session.id)
        .await
        .expect("conversation is watchable");
    let sender = resource_sender(&adapter).await;

    for n in 1..=600 {
        sender.send(sample(n)).await.expect("watch remains open");
    }
    await_held(&service, &session.id, &reading(600)).await;
    assert!(matches!(live.try_recv(), Err(TryRecvError::Empty)));

    let mut resources = service
        .subscribe_resources(&alice, &session.id)
        .await
        .expect("resources are watchable");
    adapter
        .conversation
        .lock()
        .await
        .as_ref()
        .expect("conversation opened")
        .updates
        .send(chunk("hello"))
        .await
        .expect("core is listening");
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), live.recv()).await,
        Ok(Ok(crate::event::LiveFrame::MessageDelta(ref text))) if text == "hello"
    ));
    assert!(matches!(resources.try_recv(), Err(TryRecvError::Empty)));
}

#[tokio::test]
async fn an_old_resource_capability_is_settled_and_written_exactly_once() {
    let store = Arc::new(FakeStore::default());
    let adapter = capable_adapter();
    let service = service(Arc::clone(&store), Arc::clone(&adapter));
    let alice = user("alice");
    let session = ready(&service, &alice, None).await;
    await_start_finished(&service, &session.id).await;

    let mut old = store
        .get(&session.id)
        .await
        .expect("readable")
        .expect("present");
    old.session.capabilities.resources = None;
    store.seed(&old).await.expect("old record written");
    let before = store.upserts.load(Ordering::SeqCst);
    let adapter_port: Arc<dyn KernelAdapter> = adapter;

    let first = service
        .settle_capabilities(&old.session, &adapter_port)
        .await
        .expect("settled");
    let after_first = store.upserts.load(Ordering::SeqCst);
    let second = service
        .settle_capabilities(&old.session, &adapter_port)
        .await
        .expect("idempotent");
    let after_second = store.upserts.load(Ordering::SeqCst);

    assert_eq!(first.resources, Some(true));
    assert_eq!(second.resources, Some(true));
    assert_eq!(after_first, before + 1);
    assert_eq!(after_second, after_first);
    let stored = store
        .get(&session.id)
        .await
        .expect("readable")
        .expect("present")
        .session
        .capabilities;
    assert_eq!(stored.resources, Some(true));
    assert_eq!(stored.terminal, old.session.capabilities.terminal);
    assert_eq!(stored.snapshot, old.session.capabilities.snapshot);
}

#[tokio::test]
async fn readoption_starts_a_fresh_resource_watch() {
    let store = Arc::new(FakeStore::default());
    let adapter = capable_adapter();
    let first = service(Arc::clone(&store), Arc::clone(&adapter));
    let alice = user("alice");
    let session = ready(&first, &alice, None).await;
    await_watch_calls(&adapter, 1).await;
    adapter.resource_sink.lock().await.take();
    drop(first);

    let restarted = service(store, Arc::clone(&adapter));
    restarted.reconcile().await;
    await_watch_calls(&adapter, 2).await;
    let sender = resource_sender(&adapter).await;
    sender.send(sample(70)).await.expect("watch remains open");
    let held = await_held(&restarted, &session.id, &reading(70)).await;
    assert_eq!(held.reading, reading(70));
}
