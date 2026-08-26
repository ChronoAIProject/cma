//! Who holds a session's terminal, and how they lose it.
//!
//! Exclusivity is what keeps two clients from writing into one another's
//! escape sequences. Takeover is what keeps that exclusivity from locking a
//! person out of their own session when the client holding it has died in a
//! way nothing has noticed yet.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;

use layer3_adapter::{AdapterError, TermSize, TerminalInput, TerminalOutput};

use crate::attach::AttachmentClaim;
use crate::commit_fault_support::CommitFaultStore;
use crate::conversation::ApprovalDecision;
use crate::coordination::{SessionLease, SessionOperation};
use crate::error::CoreError;
use crate::event::{Event, EventKind};
use crate::store::SessionStore;
use crate::terminal::TerminalGeometry;
use crate::test_support::{
    await_settled, service, service_on_version, service_over, spec, terminal_input_channel, user,
    write_screen, FakeAdapter, FakeStore,
};

struct WakeFlag(AtomicBool);

impl Wake for WakeFlag {
    fn wake(self: Arc<Self>) {
        self.0.store(true, Ordering::SeqCst);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.store(true, Ordering::SeqCst);
    }
}

fn is_terminal_attachment(_record: &crate::store::SessionRecord, events: &[Event]) -> bool {
    events
        .iter()
        .any(|event| matches!(event.kind, EventKind::TerminalAttached))
}

fn is_terminal_attachment_change(_record: &crate::store::SessionRecord, events: &[Event]) -> bool {
    events.iter().any(|event| {
        matches!(
            event.kind,
            EventKind::TerminalAttached | EventKind::TerminalDetached
        )
    })
}

/// A service with one running session, and its owner.
async fn attached_session() -> (
    Arc<crate::service::SessionService>,
    crate::authz::Caller,
    crate::ids::SessionId,
) {
    let svc = service(
        Arc::new(FakeStore::default()),
        Arc::new(FakeAdapter::default()),
    );
    let alice = user("alice");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    (svc, alice, created.id)
}

async fn wait_for_interaction_lease(store: &FakeStore, id: &crate::ids::SessionId) -> SessionLease {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(lease) = store
                .active_session_lease(id)
                .await
                .filter(|lease| lease.operation == SessionOperation::Interact)
            {
                return lease;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the interaction lease is acquired")
}

async fn wait_for_interaction_release(store: &FakeStore, id: &crate::ids::SessionId) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while store.active_session_lease(id).await.is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the interaction lease is released");
}

#[tokio::test]
async fn a_session_without_a_terminal_is_refused_before_readoption_or_logging() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter {
        capabilities: layer3_adapter::KernelCapabilities {
            conversation: true,
            ..FakeAdapter::default().capabilities
        },
        ..FakeAdapter::default()
    });
    let svc = service(Arc::clone(&store), adapter);
    let alice = user("alice");
    let created = svc
        .create_session(
            &alice,
            crate::session::SessionSpec {
                requested_mode: Some(crate::options::SessionMode::Conversation),
                ..spec()
            },
        )
        .await
        .expect("conversation session created");
    let settled = await_settled(&svc, &alice, &created.id).await;
    assert!(!settled.capabilities.terminal);

    // Model a fresh control-plane process: the durable session and its runtime
    // survive, but this process has not adopted an in-memory entry for it.
    svc.entries.write().await.remove(&created.id);
    let events_before = svc
        .events_since(&alice, &created.id, None)
        .await
        .expect("history readable")
        .len();

    assert_eq!(
        svc.attach_terminal(
            &alice,
            &created.id,
            TerminalGeometry::default(),
            AttachmentClaim::Exclusive,
        )
        .await
        .err(),
        Some(CoreError::CapabilityUnsupported("terminal".into()))
    );
    assert!(
        !svc.entries.read().await.contains_key(&created.id),
        "a refused attachment must not readopt a runtime entry"
    );

    let events = svc
        .events_since(&alice, &created.id, None)
        .await
        .expect("history readable");
    assert_eq!(
        events.len(),
        events_before,
        "refusing an attachment must not change the durable log"
    );
    assert!(
        events
            .iter()
            .all(|event| !matches!(event.kind, crate::event::EventKind::TerminalAttached)),
        "a refused attachment must never be recorded as attached"
    );
}

#[tokio::test]
async fn the_live_grant_is_rechecked_before_the_attachment_is_mutated() {
    let (svc, alice, id) = attached_session().await;
    {
        // This is the state attach can meet when it read the durable terminal
        // grant just before a concurrent plane change replaced the live entry.
        let mut entries = svc.entries.write().await;
        let entry = entries.get_mut(&id).expect("session is live");
        entry.session.capabilities.terminal = false;
        entry.session.capabilities.conversation = true;
    }
    let requested = TerminalGeometry::new(101, 37).expect("valid geometry");

    assert_eq!(
        svc.attach_terminal(&alice, &id, requested, AttachmentClaim::Exclusive)
            .await
            .err(),
        Some(CoreError::CapabilityUnsupported("terminal".into()))
    );

    {
        let entries = svc.entries.read().await;
        let entry = entries.get(&id).expect("session remains live");
        assert_eq!(entry.session.geometry, TerminalGeometry::default());
        assert!(entry.attached.is_none());
        assert_eq!(entry.attachments_issued, 0);
    }
    let events = svc
        .events_since(&alice, &id, None)
        .await
        .expect("history readable");
    assert!(
        events
            .iter()
            .all(|event| !matches!(event.kind, crate::event::EventKind::TerminalAttached)),
        "the repeated gate must run before attachment state or events change"
    );
}

#[tokio::test]
async fn a_conflicted_attachment_has_no_live_or_runtime_side_effects() {
    let inner = Arc::new(FakeStore::default());
    let store = Arc::new(CommitFaultStore::new(
        Arc::clone(&inner),
        is_terminal_attachment,
    ));
    let svc = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let alice = user("attachment-conflict-owner");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    let before_screen = svc
        .screen_snapshot(&alice, &created.id)
        .await
        .expect("screen is readable");
    let requested = TerminalGeometry::new(117, 39).expect("valid requested geometry");
    let rival_geometry = TerminalGeometry::new(119, 41).expect("valid rival geometry");

    store.pause_next();
    let attach = tokio::spawn({
        let svc = Arc::clone(&svc);
        let alice = alice.clone();
        let id = created.id.clone();
        async move {
            svc.attach_terminal(&alice, &id, requested, AttachmentClaim::Exclusive)
                .await
        }
    });
    store.wait_until_held().await;
    let mut rival = inner
        .get(&created.id)
        .await
        .unwrap()
        .expect("rival reads the record");
    rival.session.geometry = rival_geometry;
    inner
        .commit_session_idempotent("rival-terminal-attachment", &rival, &[])
        .await
        .expect("the rival geometry commits first");
    store.release();

    let error = attach
        .await
        .expect("attachment task completes")
        .err()
        .expect("the conflicted attachment is refused");
    assert!(matches!(error, CoreError::RuntimeUnavailable(_)));
    {
        let entries = svc.entries.read().await;
        let entry = entries.get(&created.id).expect("session remains live");
        assert_eq!(entry.session.geometry, before_screen.geometry);
        assert!(entry.attached.is_none());
        assert_eq!(entry.attachments_issued, 0);
        assert_eq!(
            entry
                .live
                .as_ref()
                .expect("runtime output is live")
                .receiver_count(),
            0,
            "a refused attachment publishes no output subscriber"
        );
    }
    assert_eq!(
        svc.screen_snapshot(&alice, &created.id)
            .await
            .expect("screen remains readable")
            .geometry,
        before_screen.geometry,
        "the screen is resized only after the attachment is durable"
    );
    assert_eq!(
        inner
            .get(&created.id)
            .await
            .unwrap()
            .expect("record remains present")
            .session
            .geometry,
        rival_geometry
    );
    assert!(
        svc.events_since(&alice, &created.id, None)
            .await
            .expect("history is readable")
            .iter()
            .all(|event| !matches!(event.kind, EventKind::TerminalAttached)),
        "a refused attachment has no durable event"
    );
}

#[tokio::test]
async fn four_store_outages_leave_an_attachment_entirely_unpublished() {
    let inner = Arc::new(FakeStore::default());
    let store = Arc::new(CommitFaultStore::new(
        Arc::clone(&inner),
        is_terminal_attachment,
    ));
    let svc = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let alice = user("attachment-outage-owner");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    let before = inner
        .get(&created.id)
        .await
        .unwrap()
        .expect("settled record exists");
    let before_screen = svc
        .screen_snapshot(&alice, &created.id)
        .await
        .expect("screen is readable");

    store.fail_next(4);
    let error = svc
        .attach_terminal(
            &alice,
            &created.id,
            TerminalGeometry::new(117, 39).expect("valid requested geometry"),
            AttachmentClaim::Exclusive,
        )
        .await
        .err()
        .expect("the fourth unavailable response is reported");

    assert!(matches!(error, CoreError::RuntimeUnavailable(_)));
    assert_eq!(store.matched_calls(), 4);
    let entries = svc.entries.read().await;
    let entry = entries.get(&created.id).expect("session remains live");
    assert_eq!(entry.session.geometry, before.session.geometry);
    assert!(entry.attached.is_none());
    assert_eq!(entry.attachments_issued, 0);
    drop(entries);
    assert_eq!(
        svc.screen_snapshot(&alice, &created.id)
            .await
            .expect("screen remains readable")
            .geometry,
        before_screen.geometry
    );
    let durable = inner.get(&created.id).await.unwrap().unwrap();
    assert_eq!(durable.session.geometry, before.session.geometry);
    assert_eq!(durable.session.last_seq, before.session.last_seq);
    assert!(svc
        .events_since(&alice, &created.id, None)
        .await
        .expect("history is readable")
        .iter()
        .all(|event| !matches!(event.kind, EventKind::TerminalAttached)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_after_attachment_install_retains_detach_when_the_entry_disappears() {
    let inner = Arc::new(FakeStore::default());
    let store = Arc::new(CommitFaultStore::new(
        Arc::clone(&inner),
        is_terminal_attachment_change,
    ));
    let svc = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let alice = user("cancelled-attachment-owner");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    let before = inner.get(&created.id).await.unwrap().unwrap();
    let lane = svc.event_logs.lane(&created.id);
    let screen = {
        let entries = svc.entries.read().await;
        Arc::clone(&entries.get(&created.id).expect("live entry").screen)
    };
    let screen_guard = screen.lock().await;
    let requested = TerminalGeometry::new(127, 49).expect("valid geometry");

    store.pause_next();
    let attach = tokio::spawn({
        let svc = Arc::clone(&svc);
        let alice = alice.clone();
        let id = created.id.clone();
        async move {
            svc.attach_terminal(&alice, &id, requested, AttachmentClaim::Exclusive)
                .await
        }
    });
    store.wait_until_held().await;
    // The next selected write is the compensating detach. Holding the screen
    // keeps handle construction behind the live installation boundary.
    store.pause_next();
    store.release();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if svc
                .entries
                .read()
                .await
                .get(&created.id)
                .and_then(|entry| entry.attached.as_ref())
                .is_some()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the durable attachment is installed before handle construction");

    let removed = svc
        .entries
        .write()
        .await
        .remove(&created.id)
        .expect("reconciliation removes the original live entry");
    assert_eq!(removed.attached.as_ref().map(|a| a.generation), Some(1));
    drop(removed);
    attach.abort();
    let _ = attach.await;
    drop(screen_guard);

    store.wait_until_held().await;
    let interim = inner
        .events_since(&created.id, before.session.last_seq)
        .await
        .unwrap();
    assert_eq!(interim.len(), 1);
    assert!(matches!(
        interim[0].kind,
        crate::event::StoredEventKind::Known(EventKind::TerminalAttached)
    ));
    store.fail_next(4);
    store.release();

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let events = inner
                .events_since(&created.id, before.session.last_seq)
                .await
                .unwrap();
            if events.len() == 2 && lane.queued() == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the retained detach survives four temporary outages");

    let events = inner
        .events_since(&created.id, before.session.last_seq)
        .await
        .unwrap();
    assert!(matches!(
        events[0].kind,
        crate::event::StoredEventKind::Known(EventKind::TerminalAttached)
    ));
    assert!(matches!(
        events[1].kind,
        crate::event::StoredEventKind::Known(EventKind::TerminalDetached)
    ));
    assert_eq!(store.matched_calls(), 6);
    assert!(!svc.entries.read().await.contains_key(&created.id));

    let later = svc
        .attach_terminal(
            &alice,
            &created.id,
            TerminalGeometry::default(),
            AttachmentClaim::Exclusive,
        )
        .await
        .expect("a later attachment can readopt and claim the terminal");
    drop(later);
}

#[tokio::test]
async fn a_terminal_attachment_carries_its_repaint_and_requested_geometry() {
    let store = Arc::new(FakeStore::default());
    let svc = service_on_version(store, Arc::new(FakeAdapter::default()), "runtime:test");
    let alice = user("alice");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    write_screen(&svc, &created.id, b"work already on screen\r\n").await;
    let geometry = TerminalGeometry::new(132, 43).expect("valid geometry");

    let attachment = svc
        .attach_terminal(&alice, &created.id, geometry, AttachmentClaim::Exclusive)
        .await
        .expect("terminal session attaches");

    assert_eq!(attachment.geometry, geometry);
    assert_eq!(attachment.repaint, b"work already on screen\r\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_crossing_attach_is_in_the_repaint_or_live_stream_exactly_once() {
    let adapter = Arc::new(FakeAdapter::default());
    let svc = service_on_version(
        Arc::new(FakeStore::default()),
        Arc::clone(&adapter),
        "runtime:test",
    );
    let alice = user("alice");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;

    let (screen, live) = {
        let entries = svc.entries.read().await;
        let entry = entries.get(&created.id).expect("session is live");
        (
            Arc::clone(&entry.screen),
            entry.live.clone().expect("runtime output is live"),
        )
    };
    assert_eq!(live.receiver_count(), 0);

    // Hold the screen, then put the relay first in the mutex's FIFO. The
    // attachment arrives second, which is the exact interleaving that used to
    // subscribe before the relay both painted and broadcast the same bytes.
    let screen_guard = screen.lock().await;
    let runtime = adapter
        .senders
        .lock()
        .await
        .last()
        .cloned()
        .expect("runtime output sender");
    runtime
        .send(TerminalOutput::Bytes(b"one crossing chunk".to_vec()))
        .await
        .expect("runtime sends output");
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while runtime.capacity() != runtime.max_capacity() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("relay consumes the chunk and waits for the screen");

    let attach = tokio::spawn({
        let svc = Arc::clone(&svc);
        let alice = alice.clone();
        let id = created.id.clone();
        async move {
            svc.attach_terminal(
                &alice,
                &id,
                TerminalGeometry::default(),
                AttachmentClaim::Exclusive,
            )
            .await
        }
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if svc
                .entries
                .read()
                .await
                .get(&created.id)
                .is_some_and(|entry| entry.attached.is_some())
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("attachment reaches the snapshot boundary");

    // A subscriber appearing while the screen is locked is the old, broken
    // ordering. Give the spawned attach ample time to expose that ordering.
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(250), async {
            while live.receiver_count() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_err(),
        "the attachment must take the screen boundary before subscribing"
    );

    drop(screen_guard);
    let mut attachment = tokio::time::timeout(std::time::Duration::from_secs(2), attach)
        .await
        .expect("attachment completes")
        .expect("attachment task stays alive")
        .expect("terminal attaches");
    assert_eq!(attachment.repaint, b"one crossing chunk");
    assert_eq!(
        attachment.output.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty),
        "bytes already present in the repaint must not be queued again"
    );
}

#[tokio::test]
async fn a_second_client_is_refused_while_one_holds_the_terminal() {
    // Two clients writing into one screen corrupt each other's view in a way
    // neither can see, which is why the attachment is exclusive at all.
    let (svc, alice, id) = attached_session().await;
    let _held = svc
        .attach_terminal(
            &alice,
            &id,
            TerminalGeometry::default(),
            AttachmentClaim::Exclusive,
        )
        .await
        .expect("the first client attaches");

    assert_eq!(
        svc.attach_terminal(
            &alice,
            &id,
            TerminalGeometry::default(),
            AttachmentClaim::Exclusive
        )
        .await
        .err(),
        Some(CoreError::AlreadyAttached)
    );
}

#[tokio::test]
async fn an_observed_attachment_releases_interaction_authority_before_handle_drop() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));
    let alice = user("observed-attachment-authority-owner");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    adapter.terminal_inputs.lock().await.clear();
    adapter.pause_next_terminal_input();

    let attaching = tokio::spawn({
        let svc = Arc::clone(&svc);
        let alice = alice.clone();
        let id = created.id.clone();
        async move {
            svc.attach_terminal(
                &alice,
                &id,
                TerminalGeometry::new(101, 37).expect("valid geometry"),
                AttachmentClaim::Exclusive,
            )
            .await
        }
    });
    adapter.wait_until_terminal_input_paused().await;
    let held = wait_for_interaction_lease(&store, &created.id).await;
    assert_eq!(store.active_session_lease(&created.id).await, Some(held));

    adapter.resume_terminal_input();
    let attachment = attaching
        .await
        .expect("attachment task remains live")
        .expect("attachment succeeds");
    wait_for_interaction_release(&store, &created.id).await;
    assert!(
        svc.entries
            .read()
            .await
            .get(&created.id)
            .is_some_and(|entry| entry.attached.is_some()),
        "the handle keeps the local attachment, not interaction authority"
    );
    assert_eq!(
        svc.attach_terminal(
            &alice,
            &created.id,
            TerminalGeometry::default(),
            AttachmentClaim::Exclusive,
        )
        .await
        .err(),
        Some(CoreError::AlreadyAttached)
    );
    wait_for_interaction_release(&store, &created.id).await;
    drop(attachment);
}

#[tokio::test]
async fn cancelling_an_admitted_attach_finishes_receipt_then_detaches_exactly_once() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));
    let alice = user("cancelled-admitted-attachment-owner");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    let before = store
        .get(&created.id)
        .await
        .unwrap()
        .expect("settled record exists");
    adapter.terminal_inputs.lock().await.clear();
    adapter.pause_next_terminal_input();
    let geometry = TerminalGeometry::new(103, 39).expect("valid geometry");

    let attaching = tokio::spawn({
        let svc = Arc::clone(&svc);
        let alice = alice.clone();
        let id = created.id.clone();
        async move {
            svc.attach_terminal(&alice, &id, geometry, AttachmentClaim::Exclusive)
                .await
        }
    });
    adapter.wait_until_terminal_input_paused().await;
    let held = wait_for_interaction_lease(&store, &created.id).await;
    attaching.abort();
    assert!(attaching.await.unwrap_err().is_cancelled());
    assert_eq!(
        store.active_session_lease(&created.id).await,
        Some(held),
        "an admitted attachment resize retains exact authority through receipt and cleanup"
    );

    adapter.resume_terminal_input();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let events = store
                .events_since(&created.id, before.session.last_seq)
                .await
                .expect("attachment events are readable");
            let detached = events.iter().filter(|event| {
                matches!(
                    event.kind,
                    crate::event::StoredEventKind::Known(EventKind::TerminalDetached)
                )
            });
            if events.len() == 2
                && detached.count() == 1
                && svc
                    .entries
                    .read()
                    .await
                    .get(&created.id)
                    .is_some_and(|entry| entry.attached.is_none())
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the unread admitted attachment is detached once");
    wait_for_interaction_release(&store, &created.id).await;

    let delivered = adapter.terminal_inputs.lock().await;
    assert!(matches!(
        delivered.as_slice(),
        [(_, TerminalInput::Resize(size))]
            if size.cols == geometry.columns() && size.rows == geometry.rows()
    ));
    drop(delivered);
    let events = store
        .events_since(&created.id, before.session.last_seq)
        .await
        .expect("attachment events are readable");
    assert!(matches!(
        events[0].kind,
        crate::event::StoredEventKind::Known(EventKind::TerminalAttached)
    ));
    assert!(matches!(
        events[1].kind,
        crate::event::StoredEventKind::Known(EventKind::TerminalDetached)
    ));
}

#[tokio::test]
async fn cancelling_before_attach_admission_sends_nothing_and_compensates_durably() {
    let inner = Arc::new(FakeStore::default());
    let store = Arc::new(CommitFaultStore::new(
        Arc::clone(&inner),
        is_terminal_attachment,
    ));
    let adapter = Arc::new(FakeAdapter::default());
    let svc = service_over(store.clone(), Arc::clone(&adapter));
    let alice = user("cancelled-pre-admission-attachment-owner");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    let before = inner
        .get(&created.id)
        .await
        .unwrap()
        .expect("settled record exists");
    let before_screen = svc
        .screen_snapshot(&alice, &created.id)
        .await
        .expect("screen is readable");
    adapter.terminal_inputs.lock().await.clear();
    let geometry = TerminalGeometry::new(105, 41).expect("valid geometry");

    store.pause_next();
    let attaching = tokio::spawn({
        let svc = Arc::clone(&svc);
        let alice = alice.clone();
        let id = created.id.clone();
        async move {
            svc.attach_terminal(&alice, &id, geometry, AttachmentClaim::Exclusive)
                .await
        }
    });
    store.wait_until_held().await;
    let held = wait_for_interaction_lease(&inner, &created.id).await;
    attaching.abort();
    assert!(attaching.await.unwrap_err().is_cancelled());
    assert_eq!(
        inner.active_session_lease(&created.id).await,
        Some(held),
        "the retained append owns authority until its compensation settles"
    );
    store.release();

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let events = inner
                .events_since(&created.id, before.session.last_seq)
                .await
                .expect("attachment events are readable");
            if events.len() == 2
                && events.iter().any(|event| {
                    matches!(
                        event.kind,
                        crate::event::StoredEventKind::Known(EventKind::TerminalDetached)
                    )
                })
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the committed proposal receives a compensating detach");
    wait_for_interaction_release(&inner, &created.id).await;

    assert!(
        adapter.terminal_inputs.lock().await.is_empty(),
        "cancellation before final admission reaches no runtime sender"
    );
    let entries = svc.entries.read().await;
    let entry = entries.get(&created.id).expect("session remains live");
    assert!(entry.attached.is_none());
    assert_eq!(entry.attachments_issued, 0);
    drop(entries);
    assert_eq!(
        svc.screen_snapshot(&alice, &created.id)
            .await
            .expect("screen remains readable")
            .geometry,
        before_screen.geometry,
        "a refused attachment does not resize the local screen"
    );
    let events = inner
        .events_since(&created.id, before.session.last_seq)
        .await
        .expect("attachment events are readable");
    assert!(matches!(
        events[0].kind,
        crate::event::StoredEventKind::Known(EventKind::TerminalAttached)
    ));
    assert!(matches!(
        events[1].kind,
        crate::event::StoredEventKind::Known(EventKind::TerminalDetached)
    ));
}

#[tokio::test]
async fn a_committed_attach_never_reaches_a_replaced_generation_or_sender() {
    let inner = Arc::new(FakeStore::default());
    let store = Arc::new(CommitFaultStore::new(
        Arc::clone(&inner),
        is_terminal_attachment,
    ));
    let adapter = Arc::new(FakeAdapter::default());
    let svc = service_over(store.clone(), Arc::clone(&adapter));
    let alice = user("stale-attachment-generation-owner");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    let before = inner
        .get(&created.id)
        .await
        .unwrap()
        .expect("settled record exists");
    let before_screen = svc
        .screen_snapshot(&alice, &created.id)
        .await
        .expect("screen is readable");
    adapter.terminal_inputs.lock().await.clear();
    let geometry = TerminalGeometry::new(107, 43).expect("valid geometry");

    store.pause_next();
    let attaching = tokio::spawn({
        let svc = Arc::clone(&svc);
        let alice = alice.clone();
        let id = created.id.clone();
        async move {
            svc.attach_terminal(&alice, &id, geometry, AttachmentClaim::Exclusive)
                .await
        }
    });
    store.wait_until_held().await;
    let held = wait_for_interaction_lease(&inner, &created.id).await;
    let (successor_input, mut successor_received) = terminal_input_channel(1);
    {
        let mut entries = svc.entries.write().await;
        let entry = entries.get_mut(&created.id).expect("session is live");
        entry.runtime_generation = entry
            .runtime_generation
            .checked_add(1)
            .expect("test generations remain bounded");
        entry.input = Some(successor_input);
    }
    store.release();

    let error = attaching
        .await
        .expect("attachment task remains live")
        .err()
        .expect("the stale attachment is refused");
    assert!(matches!(error, CoreError::RuntimeUnavailable(_)));
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let events = inner
                .events_since(&created.id, before.session.last_seq)
                .await
                .expect("attachment events are readable");
            if events.len() == 2
                && events.iter().any(|event| {
                    matches!(
                        event.kind,
                        crate::event::StoredEventKind::Known(EventKind::TerminalDetached)
                    )
                })
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the stale durable attach receives a compensating detach");
    wait_for_interaction_release(&inner, &created.id).await;
    assert_ne!(
        inner.active_session_lease(&created.id).await,
        Some(held),
        "the exact stale interaction lease is retired after compensation"
    );

    assert!(
        adapter.terminal_inputs.lock().await.is_empty(),
        "the outgoing generation receives no stale resize"
    );
    assert!(matches!(
        successor_received.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    let entries = svc.entries.read().await;
    let entry = entries
        .get(&created.id)
        .expect("successor entry remains live");
    assert_eq!(entry.session.geometry, before.session.geometry);
    assert!(entry.attached.is_none());
    assert_eq!(entry.attachments_issued, 0);
    drop(entries);
    assert_eq!(
        svc.screen_snapshot(&alice, &created.id)
            .await
            .expect("successor screen remains readable")
            .geometry,
        before_screen.geometry
    );
}

#[test]
fn dropping_an_unpolled_attach_after_its_runtime_stops_is_recovered_by_the_next_runtime() {
    let origin = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("origin runtime builds");
    let (inner, svc, alice, created, before, attach) = origin.block_on(async {
        let inner = Arc::new(FakeStore::default());
        let svc = service(Arc::clone(&inner), Arc::new(FakeAdapter::default()));
        let alice = user("outside-runtime-attachment-owner");
        let created = svc.create_session(&alice, spec()).await.expect("created");
        await_settled(&svc, &alice, &created.id).await;
        let before = inner
            .get(&created.id)
            .await
            .unwrap()
            .expect("settled record exists");
        let mut attach = Box::pin({
            let svc = Arc::clone(&svc);
            let alice = alice.clone();
            let id = created.id.clone();
            async move {
                svc.attach_terminal(
                    &alice,
                    &id,
                    TerminalGeometry::default(),
                    AttachmentClaim::Exclusive,
                )
                .await
            }
        });
        let woken = Arc::new(WakeFlag(AtomicBool::new(false)));
        let waker = Waker::from(Arc::clone(&woken));
        let mut context = Context::from_waker(&waker);
        assert!(
            matches!(attach.as_mut().poll(&mut context), Poll::Pending),
            "the attachment waits for its durable commit"
        );
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let installed = svc
                    .entries
                    .read()
                    .await
                    .get(&created.id)
                    .is_some_and(|entry| entry.attached.is_some());
                if installed && woken.0.load(Ordering::SeqCst) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the successful attachment is buffered without being polled");
        (inner, svc, alice, created, before, attach)
    });

    drop(origin);
    assert!(
        tokio::runtime::Handle::try_current().is_err(),
        "the origin runtime is gone before the unread future is dropped"
    );
    drop(attach);

    let replacement = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("replacement runtime builds");
    replacement.block_on(async {
        let later = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            svc.attach_terminal(
                &alice,
                &created.id,
                TerminalGeometry::default(),
                AttachmentClaim::Exclusive,
            ),
        )
        .await
        .expect("the replacement runtime drains the abandoned attachment")
        .expect("the next exclusive attachment is not permanently blocked");

        let events = inner
            .events_since(&created.id, before.session.last_seq)
            .await
            .expect("attachment events are readable");
        assert_eq!(events.len(), 3);
        assert!(matches!(
            events[0].kind,
            crate::event::StoredEventKind::Known(EventKind::TerminalAttached)
        ));
        assert!(matches!(
            events[1].kind,
            crate::event::StoredEventKind::Known(EventKind::TerminalDetached)
        ));
        assert!(matches!(
            events[2].kind,
            crate::event::StoredEventKind::Known(EventKind::TerminalAttached)
        ));
        drop(later);
    });
}

#[tokio::test]
async fn a_waiting_attach_takes_over_when_the_active_cleanup_drain_is_cancelled() {
    let inner = Arc::new(FakeStore::default());
    let store = Arc::new(CommitFaultStore::new(
        Arc::clone(&inner),
        is_terminal_attachment_change,
    ));
    let svc = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let alice = user("cleanup-drain-takeover-owner");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    let before = inner
        .get(&created.id)
        .await
        .unwrap()
        .expect("settled record exists");
    let mut attach = Box::pin({
        let svc = Arc::clone(&svc);
        let alice = alice.clone();
        let id = created.id.clone();
        async move {
            svc.attach_terminal(
                &alice,
                &id,
                TerminalGeometry::default(),
                AttachmentClaim::Exclusive,
            )
            .await
        }
    });
    let woken = Arc::new(WakeFlag(AtomicBool::new(false)));
    let waker = Waker::from(Arc::clone(&woken));
    let mut context = Context::from_waker(&waker);
    assert!(matches!(attach.as_mut().poll(&mut context), Poll::Pending));
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let installed = svc
                .entries
                .read()
                .await
                .get(&created.id)
                .is_some_and(|entry| entry.attached.is_some());
            if installed && woken.0.load(Ordering::SeqCst) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the successful attachment is buffered without being polled");

    store.pause_next();
    let first_service = Arc::clone(&svc);
    let first_owner = alice.clone();
    let first_id = created.id.clone();
    let first = tokio::spawn(async move {
        first_service
            .attach_terminal(
                &first_owner,
                &first_id,
                TerminalGeometry::default(),
                AttachmentClaim::Exclusive,
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while svc.event_logs.cleanup_drain_users(&created.id) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the first attach waits after observing an empty cleanup queue");
    assert!(!first.is_finished());

    std::thread::spawn(move || {
        assert!(tokio::runtime::Handle::try_current().is_err());
        drop(attach);
    })
    .join()
    .expect("the unread attachment is handed to deferred cleanup");
    store.wait_until_held().await;

    let waiting_service = Arc::clone(&svc);
    let waiting_owner = alice.clone();
    let waiting_id = created.id.clone();
    let waiting = tokio::spawn(async move {
        waiting_service
            .attach_terminal(
                &waiting_owner,
                &waiting_id,
                TerminalGeometry::default(),
                AttachmentClaim::Exclusive,
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while svc.event_logs.cleanup_drain_users(&created.id) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the second attach waits on the active cleanup drain");
    assert!(!waiting.is_finished());

    first.abort();
    assert!(matches!(first.await, Err(error) if error.is_cancelled()));
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while svc.event_logs.cleanup_drain_users(&created.id) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the waiting attach takes ownership of the cleanup drain");
    store.release();

    let later = tokio::time::timeout(std::time::Duration::from_secs(2), waiting)
        .await
        .expect("the replacement drain completes")
        .expect("the waiting attach task remains live")
        .expect("the terminal becomes exclusively attachable");
    let events = inner
        .events_since(&created.id, before.session.last_seq)
        .await
        .expect("attachment events are readable");
    assert_eq!(events.len(), 3);
    assert_eq!(
        events
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    crate::event::StoredEventKind::Known(EventKind::TerminalDetached)
                )
            })
            .count(),
        1,
        "cleanup takeover must not duplicate the durable detach"
    );
    assert!(matches!(
        events[2].kind,
        crate::event::StoredEventKind::Known(EventKind::TerminalAttached)
    ));
    assert_eq!(store.matched_calls(), 3);
    drop(later);
}

#[test]
fn a_detach_interrupted_after_store_commit_restarts_without_a_duplicate() {
    let origin = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("origin runtime builds");
    let (inner, store, svc, alice, created, before, attach) = origin.block_on(async {
        let inner = Arc::new(FakeStore::default());
        let store = Arc::new(CommitFaultStore::new(
            Arc::clone(&inner),
            is_terminal_attachment_change,
        ));
        let svc = service_over(store.clone(), Arc::new(FakeAdapter::default()));
        let alice = user("interrupted-cleanup-owner");
        let created = svc.create_session(&alice, spec()).await.expect("created");
        await_settled(&svc, &alice, &created.id).await;
        let before = inner
            .get(&created.id)
            .await
            .unwrap()
            .expect("settled record exists");
        let mut attach = Box::pin({
            let svc = Arc::clone(&svc);
            let alice = alice.clone();
            let id = created.id.clone();
            async move {
                svc.attach_terminal(
                    &alice,
                    &id,
                    TerminalGeometry::default(),
                    AttachmentClaim::Exclusive,
                )
                .await
            }
        });
        let woken = Arc::new(WakeFlag(AtomicBool::new(false)));
        let waker = Waker::from(Arc::clone(&woken));
        let mut context = Context::from_waker(&waker);
        assert!(matches!(attach.as_mut().poll(&mut context), Poll::Pending));
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let installed = svc
                    .entries
                    .read()
                    .await
                    .get(&created.id)
                    .is_some_and(|entry| entry.attached.is_some());
                if installed && woken.0.load(Ordering::SeqCst) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the first attachment success is buffered");
        store.pause_after_next();
        (inner, store, svc, alice, created, before, attach)
    });

    drop(origin);
    assert!(tokio::runtime::Handle::try_current().is_err());
    drop(attach);

    let interrupted = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("interrupted cleanup runtime builds");
    interrupted.block_on(async {
        let task_service = Arc::clone(&svc);
        let task_owner = alice.clone();
        let task_id = created.id.clone();
        tokio::spawn(async move {
            task_service
                .attach_terminal(
                    &task_owner,
                    &task_id,
                    TerminalGeometry::default(),
                    AttachmentClaim::Exclusive,
                )
                .await
        });
        store.wait_until_held().await;
        let events = inner
            .events_since(&created.id, before.session.last_seq)
            .await
            .expect("the committed detach is visible before its response");
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[1].kind,
            crate::event::StoredEventKind::Known(EventKind::TerminalDetached)
        ));
    });
    drop(interrupted);

    let replacement = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("replacement runtime builds");
    replacement.block_on(async {
        let later = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            svc.attach_terminal(
                &alice,
                &created.id,
                TerminalGeometry::default(),
                AttachmentClaim::Exclusive,
            ),
        )
        .await
        .expect("the replacement runtime restarts the retained detach")
        .expect("the terminal is attachable after cleanup replay");
        let events = inner
            .events_since(&created.id, before.session.last_seq)
            .await
            .expect("attachment events are readable");
        assert_eq!(events.len(), 3, "the detach is idempotent across replay");
        assert_eq!(
            events
                .iter()
                .filter(|event| {
                    matches!(
                        event.kind,
                        crate::event::StoredEventKind::Known(EventKind::TerminalDetached)
                    )
                })
                .count(),
            1
        );
        assert!(matches!(
            events[2].kind,
            crate::event::StoredEventKind::Known(EventKind::TerminalAttached)
        ));
        assert_eq!(store.matched_calls(), 4);
        drop(later);
    });
}

#[tokio::test]
async fn a_takeover_supersedes_the_holder_and_tells_them() {
    // The lock-out this exists to prevent: a client whose connection died in a
    // way the server has not noticed still holds the attachment, and its owner
    // cannot get back into their own session.
    let (svc, alice, id) = attached_session().await;
    let mut first = svc
        .attach_terminal(
            &alice,
            &id,
            TerminalGeometry::default(),
            AttachmentClaim::Exclusive,
        )
        .await
        .expect("the first client attaches");

    let _second = svc
        .attach_terminal(
            &alice,
            &id,
            TerminalGeometry::default(),
            AttachmentClaim::Takeover,
        )
        .await
        .expect("the owner may take their own terminal back");

    assert!(
        first.superseded.try_recv().is_ok(),
        "a superseded client must be told, not merely starved of output"
    );
}

#[tokio::test]
async fn the_superseded_client_leaving_does_not_detach_its_replacement() {
    // The race this is about: the old client drops its handle after the new
    // one is already live. An unscoped release would detach the person who
    // just took their session back, and they would watch it go dead.
    let (svc, alice, id) = attached_session().await;
    let first = svc
        .attach_terminal(
            &alice,
            &id,
            TerminalGeometry::default(),
            AttachmentClaim::Exclusive,
        )
        .await
        .expect("attaches");
    let _second = svc
        .attach_terminal(
            &alice,
            &id,
            TerminalGeometry::default(),
            AttachmentClaim::Takeover,
        )
        .await
        .expect("takes over");

    drop(first);
    // The release is spawned, so give it a moment to land.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert_eq!(
        svc.attach_terminal(
            &alice,
            &id,
            TerminalGeometry::default(),
            AttachmentClaim::Exclusive
        )
        .await
        .err(),
        Some(CoreError::AlreadyAttached),
        "the replacement must still hold the attachment"
    );
}

#[tokio::test]
async fn releasing_the_only_attachment_frees_the_terminal() {
    // The ordinary case: a client goes away and the next one can attach.
    let (svc, alice, id) = attached_session().await;
    let held = svc
        .attach_terminal(
            &alice,
            &id,
            TerminalGeometry::default(),
            AttachmentClaim::Exclusive,
        )
        .await
        .expect("attaches");
    drop(held);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert!(
        svc.attach_terminal(
            &alice,
            &id,
            TerminalGeometry::default(),
            AttachmentClaim::Exclusive
        )
        .await
        .is_ok(),
        "a released terminal must be attachable again"
    );
}

#[tokio::test]
async fn a_takeover_on_an_unheld_terminal_is_an_ordinary_attach() {
    // Nothing to supersede is not a failure. It is the common case when a
    // client reconnects after its previous connection was already reaped.
    let (svc, alice, id) = attached_session().await;
    assert!(svc
        .attach_terminal(
            &alice,
            &id,
            TerminalGeometry::default(),
            AttachmentClaim::Takeover
        )
        .await
        .is_ok());
}

#[tokio::test]
async fn somebody_elses_session_cannot_be_taken_over() {
    // Takeover is a stronger claim than attach, so it must not be a way around
    // the ownership check that guards the weaker one.
    let (svc, _alice, id) = attached_session().await;
    assert_eq!(
        svc.attach_terminal(
            &user("mallory"),
            &id,
            TerminalGeometry::default(),
            AttachmentClaim::Takeover
        )
        .await
        .err(),
        Some(CoreError::Forbidden),
        "a session somebody does not own must not be reachable by taking it over"
    );
}

#[tokio::test]
async fn the_client_holding_the_terminal_can_still_type_into_it() {
    // The defect this guards, and it is one this file's own subject invites:
    // a guard against writing while somebody is attached, put in the operation
    // itself, refuses the attachment its own keystrokes. Every character a PTY
    // client sends goes through `send_input` while it holds the terminal.
    let (svc, alice, id) = attached_session().await;
    let _held = svc
        .attach_terminal(
            &alice,
            &id,
            TerminalGeometry::default(),
            AttachmentClaim::Exclusive,
        )
        .await
        .expect("the client attaches");

    svc.send_input(&alice, &id, b"ls\n".to_vec())
        .await
        .expect("an attached client types into its own terminal");
    svc.resize_terminal(&alice, &id, TerminalGeometry::new(100, 30).expect("valid"))
        .await
        .expect("and declares the geometry it is drawing for");
    svc.send_signal(&alice, &id, "INT".to_string())
        .await
        .expect("and can stop what it started");
}

#[tokio::test]
async fn a_second_service_readopts_before_fenced_terminal_delivery() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let first = service(Arc::clone(&store), Arc::clone(&adapter));
    let alice = user("terminal-readoption-owner");
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;

    let second = service(Arc::clone(&store), Arc::clone(&adapter));
    assert!(!second.entries.read().await.contains_key(&created.id));
    adapter.terminal_inputs.lock().await.clear();

    second
        .send_input(&alice, &created.id, b"from replica two".to_vec())
        .await
        .expect("the second service readopts and delivers bytes");
    wait_for_interaction_release(&store, &created.id).await;
    second
        .send_signal(&alice, &created.id, "INT".into())
        .await
        .expect("the readopted service delivers a signal");

    let delivered = adapter.terminal_inputs.lock().await;
    assert_eq!(delivered.len(), 2);
    assert!(matches!(
        &delivered[0].1,
        TerminalInput::Bytes(bytes) if bytes == b"from replica two"
    ));
    assert!(matches!(
        &delivered[1].1,
        TerminalInput::Signal(signal) if signal == "INT"
    ));
    assert!(second.entries.read().await.contains_key(&created.id));
}

#[tokio::test]
async fn a_stale_terminal_generation_reaches_no_sender() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));
    let alice = user("stale-terminal-generation-owner");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    adapter.terminal_inputs.lock().await.clear();
    let mut entries = svc.entries.write().await;
    let entry = entries.get_mut(&created.id).expect("the session is live");
    entry.runtime_generation = entry
        .runtime_generation
        .checked_add(1)
        .expect("test generations remain bounded");
    drop(entries);

    assert_eq!(
        svc.send_input(&alice, &created.id, b"stale".to_vec()).await,
        Err(CoreError::RuntimeUnavailable(
            "the live terminal belongs to another runtime generation".into()
        ))
    );
    assert!(adapter.terminal_inputs.lock().await.is_empty());
}

#[tokio::test]
async fn terminal_delivery_reports_missing_and_rejected_senders() {
    let store = Arc::new(FakeStore::default());
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    let alice = user("terminal-sender-errors-owner");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    svc.entries
        .write()
        .await
        .get_mut(&created.id)
        .expect("the session is live")
        .input = None;

    assert_eq!(
        svc.send_input(&alice, &created.id, b"missing".to_vec())
            .await,
        Err(CoreError::RuntimeUnavailable(
            "this session holds no terminal input sender".into()
        ))
    );
    wait_for_interaction_release(&store, &created.id).await;

    let rejected = layer3_adapter::TerminalInputSender::spawn(1, |_input| async {
        Err(AdapterError::Rejected(
            "the runtime refused terminal input".into(),
        ))
    });
    svc.entries
        .write()
        .await
        .get_mut(&created.id)
        .expect("the session is live")
        .input = Some(rejected);
    assert_eq!(
        svc.send_signal(&alice, &created.id, "INT".into()).await,
        Err(CoreError::RuntimeRefused(
            "the runtime refused terminal input".into()
        ))
    );
}

#[tokio::test]
async fn caller_cancellation_before_terminal_delivery_retires_the_owned_lease() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));
    let alice = user("cancelled-terminal-delivery-owner");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    adapter.terminal_inputs.lock().await.clear();

    let input_fence = svc.input_gates.gate(&created.id).write_owned().await;
    let caller = tokio::spawn({
        let svc = Arc::clone(&svc);
        let alice = alice.clone();
        let id = created.id.clone();
        async move { svc.send_input(&alice, &id, b"cancelled".to_vec()).await }
    });
    let held = wait_for_interaction_lease(&store, &created.id).await;
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert_eq!(
        store.active_session_lease(&created.id).await,
        Some(held),
        "the owned action retains its lease while its local gate is blocked"
    );

    drop(input_fence);
    wait_for_interaction_release(&store, &created.id).await;
    assert!(
        adapter.terminal_inputs.lock().await.is_empty(),
        "cancellation before the final authority check reaches no sender"
    );

    svc.send_signal(&alice, &created.id, "INT".into())
        .await
        .expect("a successor proceeds after owned cleanup");
    assert!(matches!(
        adapter.terminal_inputs.lock().await.as_slice(),
        [(_, TerminalInput::Signal(signal))] if signal == "INT"
    ));
}

#[tokio::test]
async fn caller_cancellation_after_terminal_admission_finishes_its_receipt() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));
    let alice = user("cancelled-admitted-terminal-owner");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    adapter.terminal_inputs.lock().await.clear();
    adapter.pause_next_terminal_input();

    let caller = tokio::spawn({
        let svc = Arc::clone(&svc);
        let alice = alice.clone();
        let id = created.id.clone();
        async move {
            svc.send_input(&alice, &id, b"already admitted".to_vec())
                .await
        }
    });
    adapter.wait_until_terminal_input_paused().await;
    let held = wait_for_interaction_lease(&store, &created.id).await;
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert_eq!(
        store.active_session_lease(&created.id).await,
        Some(held),
        "an admitted sender retains the lease through its receipt"
    );

    adapter.resume_terminal_input();
    wait_for_interaction_release(&store, &created.id).await;
    assert!(matches!(
        adapter.terminal_inputs.lock().await.as_slice(),
        [(_, TerminalInput::Bytes(bytes))] if bytes == b"already admitted"
    ));
}

const ROTATION_BYTES: &[u8] = b"retry after rotation";
const ROTATION_SIGNAL: &str = "INT";
const ROTATION_PROMPT: &str = "wait for the replacement";
const ROTATION_APPROVAL: &str = "approval-during-rotation";

fn rotation_resize_geometry() -> TerminalGeometry {
    TerminalGeometry::new(100, 30).expect("valid resize geometry")
}

fn rotation_attach_geometry() -> TerminalGeometry {
    TerminalGeometry::new(120, 40).expect("valid attachment geometry")
}

async fn assert_every_runtime_operation_fails_with(
    svc: &Arc<crate::service::SessionService>,
    caller: &crate::authz::Caller,
    id: &crate::ids::SessionId,
    expected: CoreError,
) {
    assert_eq!(
        svc.send_input(caller, id, ROTATION_BYTES.to_vec()).await,
        Err(expected.clone())
    );
    assert_eq!(
        svc.send_signal(caller, id, ROTATION_SIGNAL.into()).await,
        Err(expected.clone())
    );
    assert_eq!(
        svc.resize_terminal(caller, id, rotation_resize_geometry())
            .await,
        Err(expected.clone())
    );
    assert_eq!(
        svc.terminal_is_held(caller, id).await,
        Err(expected.clone())
    );
    assert_eq!(
        svc.attach_terminal(
            caller,
            id,
            rotation_attach_geometry(),
            AttachmentClaim::Exclusive,
        )
        .await
        .err(),
        Some(expected.clone())
    );
    assert_eq!(
        svc.send_prompt(caller, id, ROTATION_PROMPT.into()).await,
        Err(expected.clone())
    );
    assert_eq!(svc.interrupt_turn(caller, id).await, Err(expected.clone()));
    assert_eq!(
        svc.resolve_approval(
            caller,
            id,
            ROTATION_APPROVAL.into(),
            ApprovalDecision::Allow,
        )
        .await,
        Err(expected)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_operation_refuses_the_retained_outgoing_runtime_and_retries_on_the_replacement() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let svc = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:2");
    let alice = user("rotating-terminal-owner");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;

    let (outgoing_input, mut outgoing_runtime) = terminal_input_channel(8);
    let mut record = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present");
    record.session.runtime_version = Some("runtime:1".into());
    store.seed(&record).await.expect("old version is durable");
    {
        let mut entries = svc.entries.write().await;
        let outgoing = entries.get_mut(&created.id).expect("outgoing entry");
        outgoing.session.runtime_version = Some("runtime:1".into());
        outgoing.input = Some(outgoing_input.clone());
    }

    adapter.pause_next_provision();
    svc.pause_next_live_install();
    let reconciling = tokio::spawn({
        let svc = Arc::clone(&svc);
        async move { svc.reconcile().await }
    });
    adapter.wait_until_provision_paused().await;

    assert_eq!(
        svc.entries
            .read()
            .await
            .get(&created.id)
            .expect("the authoritative outgoing entry remains installed")
            .session
            .status,
        crate::session::SessionStatus::Rotating,
        "the outgoing entry remains present but cannot accept input"
    );
    assert_eq!(
        store
            .get(&created.id)
            .await
            .expect("rotation is readable")
            .expect("session remains durable")
            .session
            .status,
        crate::session::SessionStatus::Rotating
    );
    assert_every_runtime_operation_fails_with(
        &svc,
        &alice,
        &created.id,
        CoreError::SessionRotating,
    )
    .await;
    assert_every_runtime_operation_fails_with(
        &svc,
        &user("foreign-rotation-observer"),
        &created.id,
        CoreError::Forbidden,
    )
    .await;
    assert!(matches!(
        outgoing_runtime.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));

    adapter.resume_provision();
    svc.wait_until_live_install_paused().await;
    assert_eq!(
        svc.entries
            .read()
            .await
            .get(&created.id)
            .expect("the outgoing entry remains until replacement installation")
            .session
            .status,
        crate::session::SessionStatus::Rotating,
        "durable readiness alone must not make the outgoing entry interactive"
    );
    assert_eq!(
        store
            .get(&created.id)
            .await
            .expect("readiness is readable")
            .expect("session remains durable")
            .session
            .status,
        crate::session::SessionStatus::Ready,
        "the in-progress rotation marker spans the final durable/live disagreement"
    );
    assert_every_runtime_operation_fails_with(
        &svc,
        &alice,
        &created.id,
        CoreError::SessionRotating,
    )
    .await;
    assert_every_runtime_operation_fails_with(
        &svc,
        &user("foreign-ready-gap-observer"),
        &created.id,
        CoreError::Forbidden,
    )
    .await;
    assert!(matches!(
        outgoing_runtime.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    svc.resume_live_install();
    assert_eq!(
        reconciling
            .await
            .expect("reconciliation task completes")
            .rotated_sessions,
        1
    );
    let settled = await_settled(&svc, &alice, &created.id).await;
    assert_eq!(settled.runtime_version.as_deref(), Some("runtime:2"));

    let (replacement_input, mut replacement_runtime) = terminal_input_channel(8);
    svc.entries
        .write()
        .await
        .get_mut(&created.id)
        .expect("replacement entry")
        .input = Some(replacement_input);
    assert!(!svc
        .terminal_is_held(&alice, &created.id)
        .await
        .expect("the replacement starts detached"));

    svc.send_input(&alice, &created.id, ROTATION_BYTES.to_vec())
        .await
        .expect("the same terminal input succeeds after rotation");
    svc.send_signal(&alice, &created.id, ROTATION_SIGNAL.into())
        .await
        .expect("the same signal succeeds after rotation");
    svc.resize_terminal(&alice, &created.id, rotation_resize_geometry())
        .await
        .expect("the same resize succeeds after rotation");
    let attachment = svc
        .attach_terminal(
            &alice,
            &created.id,
            rotation_attach_geometry(),
            AttachmentClaim::Exclusive,
        )
        .await
        .expect("the same attach succeeds after rotation");
    assert!(svc
        .terminal_is_held(&alice, &created.id)
        .await
        .expect("the replacement attachment is visible"));
    assert_eq!(attachment.geometry, rotation_attach_geometry());

    assert!(matches!(
        replacement_runtime.recv().await,
        Some(TerminalInput::Bytes(bytes)) if bytes == ROTATION_BYTES
    ));
    assert!(matches!(
        replacement_runtime.recv().await,
        Some(TerminalInput::Signal(signal)) if signal == ROTATION_SIGNAL
    ));
    assert!(matches!(
        replacement_runtime.recv().await,
        Some(TerminalInput::Resize(size))
            if size == TermSize {
                cols: rotation_resize_geometry().columns(),
                rows: rotation_resize_geometry().rows(),
            }
    ));
    assert!(matches!(
        replacement_runtime.recv().await,
        Some(TerminalInput::Resize(size))
            if size == TermSize {
                cols: rotation_attach_geometry().columns(),
                rows: rotation_attach_geometry().rows(),
            }
    ));
    assert!(matches!(
        outgoing_runtime.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restarted_rotation_stays_rotating_until_the_replacement_is_live() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("restarted-rotation-owner");
    let first = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;

    let mut record = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present");
    let outgoing = record
        .runtime_id
        .take()
        .expect("the outgoing runtime was recorded");
    record.session.status = crate::session::SessionStatus::Rotating;
    store
        .seed(&record)
        .await
        .expect("the post-removal rotation state is durable");
    adapter.lose(&outgoing).await;
    drop(first);

    let restarted = service(Arc::clone(&store), Arc::clone(&adapter));
    adapter.pause_next_provision();
    restarted.pause_next_live_install();
    let recovered = restarted.reconcile().await;
    assert_eq!(recovered.reprovisioned_sessions, 1);
    adapter.wait_until_provision_paused().await;

    assert!(
        !restarted.entries.read().await.contains_key(&created.id),
        "the recovered replacement has not been provisioned yet"
    );
    assert_eq!(
        store
            .get(&created.id)
            .await
            .expect("recovery is readable")
            .expect("session remains durable")
            .session
            .status,
        crate::session::SessionStatus::Provisioning,
        "reconciliation rewrites the durable rotation state while recovery starts"
    );
    assert_every_runtime_operation_fails_with(
        &restarted,
        &alice,
        &created.id,
        CoreError::SessionRotating,
    )
    .await;
    assert_every_runtime_operation_fails_with(
        &restarted,
        &user("foreign-restarted-provision-observer"),
        &created.id,
        CoreError::Forbidden,
    )
    .await;

    adapter.resume_provision();
    restarted.wait_until_live_install_paused().await;

    assert!(
        !restarted.entries.read().await.contains_key(&created.id),
        "the recovered replacement is not live before installation"
    );
    assert_eq!(
        store
            .get(&created.id)
            .await
            .expect("readiness is readable")
            .expect("session remains durable")
            .session
            .status,
        crate::session::SessionStatus::Ready,
        "recovery rewrites the durable rotation state before live installation"
    );
    assert_every_runtime_operation_fails_with(
        &restarted,
        &alice,
        &created.id,
        CoreError::SessionRotating,
    )
    .await;
    assert_every_runtime_operation_fails_with(
        &restarted,
        &user("foreign-restarted-rotation-observer"),
        &created.id,
        CoreError::Forbidden,
    )
    .await;

    restarted.resume_live_install();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if restarted.entries.read().await.contains_key(&created.id)
                && !restarted.rotation_in_progress(&created.id).await
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the recovered replacement becomes live and clears the marker");
    restarted
        .send_input(&alice, &created.id, ROTATION_BYTES.to_vec())
        .await
        .expect("input succeeds once the recovered replacement is live");
}

#[tokio::test]
async fn a_missing_live_entry_preserves_durable_termination_and_owner_precedence() {
    let store = Arc::new(FakeStore::default());
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    let alice = user("terminated-missing-live-owner");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;

    let mut record = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present");
    record.session.status = crate::session::SessionStatus::Terminated;
    store.seed(&record).await.expect("termination is durable");
    svc.entries.write().await.remove(&created.id);
    svc.rotations_in_progress
        .write()
        .await
        .insert(created.id.clone());

    assert_every_runtime_operation_fails_with(
        &svc,
        &alice,
        &created.id,
        CoreError::SessionTerminated,
    )
    .await;
    assert_every_runtime_operation_fails_with(
        &svc,
        &user("foreign-terminated-observer"),
        &created.id,
        CoreError::Forbidden,
    )
    .await;
}

#[tokio::test]
async fn whether_the_terminal_is_held_is_a_question_anyone_may_ask_of_their_own() {
    // Asked by an interface about to write on behalf of somebody who is not the
    // holder. A question rather than a refusal, because the answer differs by
    // caller and the operation cannot tell them apart.
    let (svc, alice, id) = attached_session().await;
    assert!(
        !svc.terminal_is_held(&alice, &id)
            .await
            .expect("readable by its owner"),
        "nobody is attached yet"
    );

    let _held = svc
        .attach_terminal(
            &alice,
            &id,
            TerminalGeometry::default(),
            AttachmentClaim::Exclusive,
        )
        .await
        .expect("the client attaches");

    assert!(svc.terminal_is_held(&alice, &id).await.expect("readable"));
}

#[tokio::test]
async fn a_stranger_may_not_ask_whether_somebody_elses_terminal_is_held() {
    // It is a fact about someone else's session, and answering reveals that
    // the session exists.
    let (svc, _alice, id) = attached_session().await;
    let mallory = user("mallory");
    assert!(svc.terminal_is_held(&mallory, &id).await.is_err());
}

#[tokio::test]
async fn process_exit_is_recorded_after_final_output_without_changing_lifecycle() {
    let adapter = Arc::new(FakeAdapter::default());
    let svc = service(Arc::new(FakeStore::default()), Arc::clone(&adapter));
    let alice = user("alice");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    let mut held = svc
        .attach_terminal(
            &alice,
            &created.id,
            TerminalGeometry::default(),
            AttachmentClaim::Exclusive,
        )
        .await
        .expect("attaches");

    let runtime = adapter.senders.lock().await[0].clone();
    runtime
        .send(layer3_adapter::TerminalOutput::Bytes(
            b"last paint".to_vec(),
        ))
        .await
        .expect("output reaches the core");
    runtime
        .send(layer3_adapter::TerminalOutput::Exited { code: Some(23) })
        .await
        .expect("the ending reaches the core");

    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(2), held.output.recv())
            .await
            .expect("output arrives in time")
            .expect("the final output is readable"),
        b"last paint"
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), held.output.recv())
            .await
            .expect("the live lane closes in time")
            .is_err(),
        "the live lane closes after its final byte"
    );
    assert_eq!(
        *held.ending.borrow(),
        Some(crate::lease::Detached::ProcessExited { code: Some(23) })
    );
    assert_eq!(
        svc.get_session(&alice, &created.id)
            .await
            .expect("the session remains readable")
            .status,
        crate::session::SessionStatus::Ready,
        "reporting a process ending must not decide session lifecycle"
    );
}

#[tokio::test]
async fn a_terminal_channel_lost_without_a_report_closes_without_fabricating_exit() {
    let adapter = Arc::new(FakeAdapter::default());
    let svc = service(Arc::new(FakeStore::default()), Arc::clone(&adapter));
    let alice = user("alice");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    let mut held = svc
        .attach_terminal(
            &alice,
            &created.id,
            TerminalGeometry::default(),
            AttachmentClaim::Exclusive,
        )
        .await
        .expect("attaches");

    adapter.senders.lock().await.clear();

    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), held.output.recv())
            .await
            .expect("the lost lane is noticed")
            .is_err()
    );
    assert_eq!(
        *held.ending.borrow(),
        None,
        "a closed transport is not evidence that the process exited"
    );
}
