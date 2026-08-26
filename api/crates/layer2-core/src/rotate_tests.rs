//! Upgrading the runtime must not cost anyone their session.
//!
//! The rule these enforce is the one in CLAUDE.md §5: no deployment, upgrade,
//! restart or rollout interrupts a session its owner has not stopped. A
//! runtime upgrade is the hardest case, because unlike a control plane restart
//! the sandbox genuinely cannot survive it — the image is the sandbox.
//!
//! Two services over one store, expecting different versions, is an upgrade.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use layer3_adapter::{ConversationInput, TerminalInput};
use tokio::sync::Notify;

use crate::commit_fault_support::CommitFaultStore;
use crate::event::{Event, EventKind, StoredEvent};
use crate::ids::{SessionId, SubjectId};
use crate::session::{ProvisioningStage, SessionStatus};
use crate::store::{Page, SessionListQuery, SessionRecord, SessionStore, StoreError};
use crate::test_support::{
    await_settled, service_on_version, service_on_version_with_skills, spec, user, FakeAdapter,
    FakeStore,
};
use crate::Reserved;

#[derive(Default)]
struct RotationStore {
    inner: Arc<FakeStore>,
    pause_next: AtomicBool,
    pause_call: AtomicUsize,
    entered: Notify,
    release: Notify,
    calls: AtomicUsize,
    failures: AtomicUsize,
}

impl RotationStore {
    fn arm(&self) {
        assert!(!self.pause_next.swap(true, Ordering::SeqCst));
    }

    fn arm_on_call(&self, call: usize) {
        assert!(call > 0, "store calls are numbered from one");
        assert_eq!(self.pause_call.swap(call, Ordering::SeqCst), 0);
    }

    fn fail_next(&self, count: usize) {
        self.failures.store(count, Ordering::SeqCst);
    }

    async fn wait_until_held(&self) {
        tokio::time::timeout(Duration::from_secs(2), self.entered.notified())
            .await
            .expect("plane change reached the store");
    }

    async fn before_commit(&self) -> Result<(), StoreError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let pause_named_call = self
            .pause_call
            .compare_exchange(call, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok();
        if self.pause_next.swap(false, Ordering::SeqCst) || pause_named_call {
            self.entered.notify_one();
            self.release.notified().await;
        }
        if self
            .failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
        {
            return Err(StoreError::Unavailable("rotation store unavailable".into()));
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl SessionStore for RotationStore {
    async fn ping(&self) -> Result<(), StoreError> {
        self.inner.ping().await
    }

    async fn get(&self, id: &SessionId) -> Result<Option<SessionRecord>, StoreError> {
        self.inner.get(id).await
    }

    async fn list_for(
        &self,
        owner: &SubjectId,
        query: &SessionListQuery,
    ) -> Result<Page<SessionRecord>, StoreError> {
        self.inner.list_for(owner, query).await
    }

    async fn reserved_capacity(&self) -> Result<Reserved, StoreError> {
        self.inner.reserved_capacity().await
    }

    async fn list_all(&self, query: &SessionListQuery) -> Result<Page<SessionRecord>, StoreError> {
        self.inner.list_all(query).await
    }

    async fn delete(&self, id: &SessionId) -> Result<(), StoreError> {
        self.inner.delete(id).await
    }

    async fn append_events(&self, events: &[Event]) -> Result<(), StoreError> {
        self.inner.append_events(events).await
    }

    async fn commit_session_idempotent(
        &self,
        write_id: &str,
        record: &SessionRecord,
        events: &[Event],
    ) -> Result<u64, StoreError> {
        self.before_commit().await?;
        self.inner
            .commit_session_idempotent(write_id, record, events)
            .await
    }

    async fn create_session_idempotent(
        &self,
        write_id: &str,
        record: &SessionRecord,
        events: &[Event],
    ) -> Result<u64, StoreError> {
        self.inner
            .create_session_idempotent(write_id, record, events)
            .await
    }

    async fn events_since(
        &self,
        id: &SessionId,
        after: u64,
    ) -> Result<Vec<StoredEvent>, StoreError> {
        self.inner.events_since(id, after).await
    }
}

fn is_plane_changed(_record: &SessionRecord, events: &[Event]) -> bool {
    events
        .iter()
        .any(|event| matches!(event.kind, EventKind::PlaneChanged { .. }))
}

/// Wait for a session to leave the states that mean "in progress".
async fn await_usable(
    svc: &Arc<crate::service::SessionService>,
    caller: &crate::authz::Caller,
    id: &crate::ids::SessionId,
) -> crate::session::Session {
    for _ in 0..400 {
        if let Ok(s) = svc.get_session(caller, id).await {
            if s.status != SessionStatus::Provisioning && s.status != SessionStatus::Rotating {
                return s;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("session never became usable");
}

#[tokio::test]
async fn a_session_is_carried_onto_a_newer_runtime() {
    // The headline case. Someone is working; the runtime image changes under
    // them; they keep working.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let old = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:1");
    let created = old.create_session(&alice, spec()).await.expect("created");
    await_settled(&old, &alice, &created.id).await;
    let before = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present");
    let before_rotation = before.session.last_seq;
    let first = before.runtime_id.expect("has a runtime");
    adapter
        .set_conversation(&first, b"turn one\nturn two\n")
        .await;
    drop(old);

    // The deployment.
    let new = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:2");
    let moved = new.reconcile().await;
    assert_eq!(
        moved.rotated_sessions, 1,
        "the session must be carried over"
    );

    let settled = await_usable(&new, &alice, &created.id).await;
    let durable_settled = await_settled(&new, &alice, &created.id).await;
    assert_eq!(settled.status, SessionStatus::Ready);
    assert_eq!(settled.id, created.id, "it must be the same session");
    assert_eq!(settled.runtime_version.as_deref(), Some("runtime:2"));
    assert_eq!(settled.rotations, 1);
    assert!(
        settled.rotated_at.is_some(),
        "the owner must be told what moment their session was carried from"
    );

    let handover = new
        .events_since(&alice, &created.id, Some(before_rotation))
        .await
        .expect("the bounded handover log is readable")
        .into_iter()
        .filter(|event| event.seq <= durable_settled.last_seq)
        .collect::<Vec<_>>();
    let position = |predicate: fn(&EventKind) -> bool| {
        handover
            .iter()
            .position(|event| predicate(&event.kind))
            .unwrap_or_else(|| panic!("the bounded handover omitted an event: {handover:?}"))
    };
    let rotating = position(|kind| {
        matches!(
            kind,
            EventKind::StatusChanged {
                status: SessionStatus::Rotating,
                ..
            }
        )
    });
    let runtime_requested = position(|kind| {
        matches!(
            kind,
            EventKind::Progress {
                stage: ProvisioningStage::RuntimeRequested
            }
        )
    });
    let runtime_created = position(|kind| {
        matches!(
            kind,
            EventKind::Progress {
                stage: ProvisioningStage::RuntimeCreated
            }
        )
    });
    let attaching = position(|kind| {
        matches!(
            kind,
            EventKind::Progress {
                stage: ProvisioningStage::Attaching
            }
        )
    });
    let ready = position(|kind| {
        matches!(
            kind,
            EventKind::StatusChanged {
                status: SessionStatus::Ready,
                ..
            }
        )
    });
    let progress_ready = position(|kind| {
        matches!(
            kind,
            EventKind::Progress {
                stage: ProvisioningStage::Ready
            }
        )
    });

    assert!(
        rotating < runtime_requested
            && runtime_requested < runtime_created
            && runtime_created < attaching
            && attaching < ready
            && ready < progress_ready,
        "the bounded handover was not announced in runtime order: {handover:?}"
    );
}

#[tokio::test]
async fn the_conversation_travels_to_the_new_runtime() {
    // Without this the session survives as an identifier and the work does
    // not, which is the same as losing it.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let old = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:1");
    let created = old.create_session(&alice, spec()).await.expect("created");
    await_settled(&old, &alice, &created.id).await;
    let first = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .runtime_id
        .expect("has a runtime");
    adapter
        .set_conversation(&first, b"the conversation so far")
        .await;
    drop(old);

    let new = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:2");
    new.reconcile().await;
    await_usable(&new, &alice, &created.id).await;

    assert_eq!(
        adapter.handovers.lock().await.clone(),
        vec![b"the conversation so far".to_vec()],
        "the new runtime must be handed exactly what the old one had"
    );
}

#[tokio::test]
async fn the_old_sandbox_is_released() {
    // A rotation that leaves the old sandbox running would double the cost of
    // every upgrade, silently.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let old = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:1");
    let created = old.create_session(&alice, spec()).await.expect("created");
    await_settled(&old, &alice, &created.id).await;
    let first = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .runtime_id
        .expect("has a runtime");
    drop(old);

    let new = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:2");
    new.reconcile().await;
    await_usable(&new, &alice, &created.id).await;

    let debts = store.runtime_cleanup_debts(&created.id).await.unwrap();
    assert_eq!(debts.len(), 1);
    assert_eq!(debts[0].runtime_id, first);
    assert_eq!(debts[0].generation, 1);
    assert!(
        adapter.terminated.lock().await.is_empty(),
        "rotation records exact retired-authority debt instead of deleting by handle"
    );

    new.reconcile().await;

    assert!(
        adapter.terminated.lock().await.contains(&first),
        "the next cleanup pass conditionally releases the retired authority"
    );
    assert_eq!(
        adapter.running_count().await,
        1,
        "exactly one sandbox must be running for one session"
    );
}

#[tokio::test]
async fn a_session_already_on_the_current_runtime_is_left_alone() {
    // Rotation is triggered by a version difference, so a version that has not
    // changed must produce no work at all. Otherwise every sweep would replace
    // every sandbox.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let svc = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:1");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;

    let first = svc.reconcile().await;
    assert_eq!(first.rotated_sessions, 0);
    let again = svc.reconcile().await;
    assert_eq!(again.rotated_sessions, 0, "and it must stay settled");
    assert!(adapter.handovers.lock().await.is_empty());
}

#[tokio::test]
async fn a_session_is_not_interactive_while_it_is_being_carried() {
    // Input during a handover would reach a sandbox being torn down: accepted,
    // then lost. Refusing it briefly is the honest alternative, and the
    // interface can only refuse it if the state says so.
    assert!(!SessionStatus::Rotating.is_interactive());
    assert!(
        SessionStatus::Rotating.wants_runtime(),
        "but it is still a session that should be running"
    );
    assert!(
        !SessionStatus::Rotating.is_terminal(),
        "and it has certainly not ended"
    );
}

#[tokio::test]
async fn a_session_that_cannot_be_carried_is_left_running() {
    // The trade this makes explicit: a stale image is bad, and losing the
    // conversation is worse. A runtime that cannot hand its conversation over
    // keeps the session it has.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter {
        refuse_snapshot: true,
        ..FakeAdapter::default()
    });
    let alice = user("alice");

    let old = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:1");
    let created = old.create_session(&alice, spec()).await.expect("created");
    await_settled(&old, &alice, &created.id).await;
    let first = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .runtime_id
        .expect("has a runtime");
    drop(old);

    let new = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:2");
    let moved = new.reconcile().await;
    assert_eq!(moved.rotated_sessions, 0);

    let settled = await_usable(&new, &alice, &created.id).await;
    assert_eq!(
        settled.status,
        SessionStatus::Ready,
        "a session that cannot be moved must not be left refusing input"
    );
    assert!(
        !adapter.terminated.lock().await.contains(&first),
        "and its runtime must not be torn down"
    );
}

#[tokio::test]
async fn a_session_that_cannot_be_carried_is_still_adopted() {
    // The case a deployment actually hits: every session running an image from
    // before the handover existed refuses to be carried. Those sessions keep
    // running, so the control plane still has to take them back over and watch
    // them — leaving one unattended because it could not be moved would be a
    // worse failure than the stale image it is stuck on.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter {
        refuse_snapshot: true,
        ..FakeAdapter::default()
    });
    let alice = user("alice");

    let old = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:1");
    let created = old.create_session(&alice, spec()).await.expect("created");
    await_settled(&old, &alice, &created.id).await;
    drop(old);

    let new = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:2");
    let recovered = new.reconcile().await;

    assert_eq!(recovered.rotated_sessions, 0, "it cannot be carried");
    assert_eq!(
        recovered.readopted_sessions, 1,
        "but it must still be taken back over"
    );
}

#[tokio::test]
async fn a_runtime_that_cannot_be_carried_is_asked_only_once() {
    // The sweep runs every few minutes for as long as the process does. A
    // session on a runtime that has already refused would otherwise be shown
    // an "updating" state it backs out of, over and over, forever.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter {
        refuse_snapshot: true,
        ..FakeAdapter::default()
    });
    let alice = user("alice");

    let old = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:1");
    let created = old.create_session(&alice, spec()).await.expect("created");
    await_settled(&old, &alice, &created.id).await;
    drop(old);

    let new = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:2");
    new.reconcile().await;
    let before = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .session
        .last_seq;

    new.reconcile().await;
    new.reconcile().await;

    let after = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .session
        .last_seq;
    assert_eq!(
        before, after,
        "a refusal must not append a state change on every sweep"
    );
}

#[tokio::test]
async fn a_session_that_cannot_be_carried_is_never_shown_as_moving() {
    // Capturing changes nothing, so it is attempted before anything is
    // announced. A session that cannot be moved should never have been told it
    // was being moved.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter {
        refuse_snapshot: true,
        ..FakeAdapter::default()
    });
    let alice = user("alice");

    let old = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:1");
    let created = old.create_session(&alice, spec()).await.expect("created");
    await_settled(&old, &alice, &created.id).await;
    let settled_seq = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .session
        .last_seq;
    drop(old);

    let new = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:2");
    new.reconcile().await;

    let record = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present");
    assert_eq!(record.session.status, SessionStatus::Ready);
    assert_eq!(
        record.session.last_seq, settled_seq,
        "nothing should have been announced at all"
    );
}

#[tokio::test]
async fn a_stopped_session_is_not_carried_anywhere() {
    // Nothing is running for it, so there is nothing to move — and moving it
    // would start a sandbox its owner asked to be rid of.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let old = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:1");
    let created = old.create_session(&alice, spec()).await.expect("created");
    await_settled(&old, &alice, &created.id).await;
    old.stop_session(&alice, &created.id)
        .await
        .expect("stopped");
    drop(old);

    let new = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:2");
    let moved = new.reconcile().await;
    assert_eq!(moved.rotated_sessions, 0);
    assert_eq!(adapter.running_count().await, 0);
    assert_eq!(
        store
            .get(&created.id)
            .await
            .expect("readable")
            .expect("present")
            .session
            .status,
        SessionStatus::Idle,
    );
}

#[tokio::test]
async fn reviving_a_stopped_session_puts_it_on_the_current_runtime() {
    // The other way onto a new image, and the one that needs no rotation:
    // there is nothing to carry, so it simply starts on what is current.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let old = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:1");
    let created = old.create_session(&alice, spec()).await.expect("created");
    await_settled(&old, &alice, &created.id).await;
    old.stop_session(&alice, &created.id)
        .await
        .expect("stopped");
    drop(old);

    let new = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:2");
    new.revive_session(&alice, &created.id)
        .await
        .expect("revivable");
    let settled = await_usable(&new, &alice, &created.id).await;

    assert_eq!(settled.status, SessionStatus::Ready);
    assert_eq!(settled.runtime_version.as_deref(), Some("runtime:2"));
    assert_eq!(
        settled.rotations, 0,
        "starting on the current version is not a rotation"
    );
}

#[tokio::test]
async fn a_deployment_that_does_not_version_its_runtime_rotates_nothing() {
    // Rotation has to be something a deployment opts into by versioning its
    // image. A deployment that does not would otherwise replace every sandbox
    // on every sweep, or none, depending on an accident of comparison.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");
    let svc = crate::test_support::service(Arc::clone(&store), Arc::clone(&adapter));

    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;

    assert_eq!(svc.reconcile().await.rotated_sessions, 0);
    assert_eq!(adapter.running_count().await, 1);
}

#[tokio::test]
async fn carrying_a_session_twice_is_counted_twice() {
    // The count is what tells someone their session has been moved more than
    // once, and a count that resets would hide exactly that.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let first = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:1");
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;
    drop(first);

    let second = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:2");
    second.reconcile().await;
    await_usable(&second, &alice, &created.id).await;
    drop(second);

    let third = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:3");
    third.reconcile().await;
    let settled = await_usable(&third, &alice, &created.id).await;

    assert_eq!(settled.rotations, 2);
    assert_eq!(settled.runtime_version.as_deref(), Some("runtime:3"));
}

#[tokio::test]
async fn a_session_whose_kernel_cannot_snapshot_is_never_offered_for_rotation() {
    // Asked statically rather than found out by trying. Without this the sweep
    // attempts a capture it already knows will fail, every few minutes, for as
    // long as the session lives — and logs each one as though something had
    // gone wrong.
    let store = Arc::new(FakeStore::default());
    let unmovable = Arc::new(FakeAdapter {
        capabilities: layer3_adapter::KernelCapabilities {
            terminal: true,
            snapshot: false,
            ..Default::default()
        },
        ..FakeAdapter::default()
    });
    let alice = user("alice");

    let old = service_on_version(Arc::clone(&store), Arc::clone(&unmovable), "runtime:1");
    let created = old.create_session(&alice, spec()).await.expect("created");
    await_settled(&old, &alice, &created.id).await;
    drop(old);

    let new = service_on_version(Arc::clone(&store), Arc::clone(&unmovable), "runtime:2");
    assert_eq!(new.reconcile().await.rotated_sessions, 0);

    let settled = await_usable(&new, &alice, &created.id).await;
    assert_eq!(
        settled.status,
        SessionStatus::Ready,
        "a session that was never movable must simply keep working"
    );
}

/// A runtime that serves both planes, as the deployment's own does.
///
/// The default fake renders a screen and holds no conversation, which is the
/// shape the rest of this file needs. Everything below is about moving between
/// the two, so it needs a runtime that has both to move between.
fn ambidextrous() -> Arc<FakeAdapter> {
    Arc::new(FakeAdapter {
        capabilities: layer3_adapter::KernelCapabilities {
            conversation: true,
            ..FakeAdapter::default().capabilities
        },
        ..FakeAdapter::default()
    })
}

fn switch_capable() -> Arc<FakeAdapter> {
    Arc::new(FakeAdapter {
        accept_plane_switch: true,
        capabilities: layer3_adapter::KernelCapabilities {
            conversation: true,
            ..FakeAdapter::default().capabilities
        },
        ..FakeAdapter::default()
    })
}

/// A live session on the terminal plane, and the service holding it.
async fn started(
    adapter: Arc<FakeAdapter>,
) -> (
    Arc<crate::service::SessionService>,
    Arc<FakeStore>,
    crate::authz::Caller,
    crate::session::Session,
) {
    let store = Arc::new(FakeStore::default());
    let svc = crate::test_support::service(Arc::clone(&store), adapter);
    let alice = user("alice");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    (svc, store, alice, created)
}

async fn started_adversarial(
    store: Arc<RotationStore>,
    adapter: Arc<FakeAdapter>,
) -> (
    Arc<crate::service::SessionService>,
    crate::authz::Caller,
    crate::session::Session,
    SessionRecord,
) {
    let svc = crate::test_support::service_over(store.clone(), adapter);
    let alice = user("plane-conflict-owner");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    let lane = svc.event_logs.lane(&created.id);
    tokio::time::timeout(Duration::from_secs(2), async {
        while lane.queued() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("session writes settle before the plane change");
    let before = store.inner.get(&created.id).await.unwrap().unwrap();
    store.calls.store(0, Ordering::SeqCst);
    (svc, alice, created, before)
}

async fn started_with_commit_fault(
    store: Arc<CommitFaultStore>,
    adapter: Arc<FakeAdapter>,
) -> (
    Arc<crate::service::SessionService>,
    crate::authz::Caller,
    crate::session::Session,
    SessionRecord,
) {
    let svc = crate::test_support::service_over(store.clone(), adapter);
    let alice = user("plane-final-commit-owner");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let lane = svc.event_logs.lane(&created.id);
            if svc.entries.read().await.contains_key(&created.id) && lane.queued() == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the original runtime and its writes settle");
    let before = store.inner.get(&created.id).await.unwrap().unwrap();
    (svc, alice, created, before)
}

async fn wait_for_rotation_lane(svc: &Arc<crate::service::SessionService>, id: &SessionId) {
    let lane = svc.event_logs.lane(id);
    tokio::time::timeout(Duration::from_secs(2), async {
        while lane.queued() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the rejected plane change leaves the durable lane");
}

#[tokio::test]
async fn a_session_moves_between_planes_and_stays_the_same_session() {
    // The whole point: chat and terminal are two surfaces onto one session,
    // and moving between them costs a rotation and nothing else.
    let adapter = switch_capable();
    let (svc, store, alice, created) = started(Arc::clone(&adapter)).await;
    let runtime = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .runtime_id
        .expect("has a runtime");
    adapter
        .set_conversation(&runtime, b"what the agent has done so far")
        .await;

    let moving = svc
        .change_plane(
            &alice,
            &created.id,
            crate::options::SessionMode::Conversation,
        )
        .await
        .expect("a runtime that holds a conversation can be asked for one");
    assert_eq!(
        moving.status,
        SessionStatus::Rotating,
        "the window in which input must be refused rather than lost"
    );

    let settled = await_usable(&svc, &alice, &created.id).await;
    assert_eq!(settled.id, created.id, "it must be the same session");
    assert!(settled.capabilities.conversation && !settled.capabilities.terminal);
    assert_eq!(settled.rotations, 1);
    assert!(
        settled.rotated_at.is_some(),
        "the owner must be told the moment their session was carried from"
    );
    assert_eq!(
        adapter.handovers.lock().await.clone(),
        vec![b"what the agent has done so far".to_vec()],
        "the conversation must travel to the runtime on the other plane"
    );
    let durable = store
        .get(&created.id)
        .await
        .unwrap()
        .expect("moved session remains durable");
    let replacement = durable.runtime_id.clone().expect("replacement is durable");
    assert_ne!(replacement, runtime);
    assert_eq!(
        durable.session.options.mode,
        crate::options::SessionMode::Conversation
    );
    assert!(durable.session.capabilities.conversation);
    {
        let entries = svc.entries.read().await;
        let entry = entries.get(&created.id).expect("replacement is live");
        assert_eq!(entry.session.options.mode, durable.session.options.mode);
        assert_eq!(entry.session.capabilities, durable.session.capabilities);
        assert_eq!(
            entry.runtime.as_ref().map(|handle| handle.0.as_str()),
            Some(replacement.as_str())
        );
        assert!(entry.input.is_none());
        assert!(entry.conversation.is_some());
    }
    let debts = store.runtime_cleanup_debts(&created.id).await.unwrap();
    assert_eq!(debts.len(), 1);
    assert_eq!(debts[0].runtime_id.as_str(), runtime.as_str());
    assert!(adapter.terminated.lock().await.is_empty());
    svc.reconcile().await;
    assert_eq!(
        adapter.terminated.lock().await.as_slice(),
        std::slice::from_ref(&runtime)
    );
    assert!(!adapter.running.lock().await.contains_key(&runtime));
    assert!(adapter.running.lock().await.contains_key(&replacement));
    assert!(
        adapter.plane_switches.lock().await.is_empty(),
        "even a willing adapter is replaced until it can atomically replace the active process and channels"
    );
    svc.send_prompt(&alice, &created.id, "continue on chat".into())
        .await
        .expect("the replacement conversation channel is usable");
    let sent = crate::conversation_support::until_sent(&adapter, |input| {
        matches!(input, ConversationInput::Prompt(_))
    })
    .await;
    assert!(sent
        .iter()
        .any(|input| matches!(input, ConversationInput::Prompt(_))));

    // And back, because a plane change is not a one-way door.
    svc.change_plane(&alice, &created.id, crate::options::SessionMode::Terminal)
        .await
        .expect("moving back is the same act");
    let back = await_usable(&svc, &alice, &created.id).await;
    assert_eq!(back.id, created.id);
    assert!(back.capabilities.terminal && !back.capabilities.conversation);
    assert_eq!(back.rotations, 2);
    let back_record = store.get(&created.id).await.unwrap().unwrap();
    let back_runtime = back_record
        .runtime_id
        .expect("terminal replacement is durable");
    let entries = svc.entries.read().await;
    let entry = entries
        .get(&created.id)
        .expect("terminal replacement is live");
    assert_eq!(
        entry.session.options.mode,
        crate::options::SessionMode::Terminal
    );
    assert_eq!(
        entry.runtime.as_ref().map(|handle| handle.0.as_str()),
        Some(back_runtime.as_str())
    );
    assert!(entry.input.is_some());
    assert!(entry.conversation.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_real_rotation_window_never_delivers_input_to_the_outgoing_runtime() {
    let store = Arc::new(RotationStore::default());
    let adapter = switch_capable();
    let (svc, alice, created, _) =
        started_adversarial(Arc::clone(&store), Arc::clone(&adapter)).await;
    let input_gate = svc.input_gates.gate(&created.id);
    let (outgoing_input, mut outgoing_runtime) = tokio::sync::mpsc::channel(1);
    outgoing_input
        .send(TerminalInput::Signal("already queued".into()))
        .await
        .expect("outgoing runtime remains open");
    svc.entries
        .write()
        .await
        .get_mut(&created.id)
        .expect("outgoing entry is live")
        .input = Some(outgoing_input);

    // Let one operation pass its Ready check and clone the outgoing sender,
    // then hold it inside delivery by keeping the channel full.
    let crossing = tokio::spawn({
        let svc = Arc::clone(&svc);
        let alice = alice.clone();
        let id = created.id.clone();
        async move {
            svc.send_input(&alice, &id, b"accepted before rotation".to_vec())
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while outgoing_runtime.sender_strong_count() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the accepted input waits inside the outgoing send");

    // Rotation may rehearse while accepted input is in flight, but it must not
    // publish Rotating until that input has reached the outgoing runtime.
    store.arm_on_call(1);
    let moving = tokio::spawn({
        let svc = Arc::clone(&svc);
        let alice = alice.clone();
        let id = created.id.clone();
        async move {
            svc.change_plane(&alice, &id, crate::options::SessionMode::Conversation)
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while adapter.snapshot_calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("rotation rehearses the handover");
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            assert_eq!(
                store.calls.load(Ordering::SeqCst),
                0,
                "rotation must wait for accepted input before publishing Rotating"
            );
            if input_gate.try_read().is_err() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("rotation queues behind the accepted input");

    assert!(matches!(
        outgoing_runtime.recv().await,
        Some(TerminalInput::Signal(signal)) if signal == "already queued"
    ));
    crossing
        .await
        .expect("input task completes")
        .expect("input accepted before rotation is delivered");
    assert!(matches!(
        outgoing_runtime.recv().await,
        Some(TerminalInput::Bytes(bytes)) if bytes == b"accepted before rotation"
    ));

    // The first write is paused before Rotating is durable, so the live entry
    // must still be Ready even though rotation has won the input fence.
    store.wait_until_held().await;
    assert_eq!(
        svc.get_session(&alice, &created.id)
            .await
            .expect("session remains readable before publication")
            .status,
        SessionStatus::Ready
    );

    // Hold the second write after Rotating has been published but before the
    // old runtime and channel are removed. Nothing new may cross this window.
    store.arm_on_call(2);
    store.release.notify_one();
    store.wait_until_held().await;
    assert_eq!(
        svc.get_session(&alice, &created.id)
            .await
            .expect("rotating session is readable")
            .status,
        SessionStatus::Rotating
    );

    let refused = b"must not arrive".to_vec();
    assert_eq!(
        svc.send_input(&alice, &created.id, refused.clone()).await,
        Err(crate::error::CoreError::SessionRotating)
    );
    assert_eq!(
        svc.send_signal(&alice, &created.id, "INT".into()).await,
        Err(crate::error::CoreError::SessionRotating)
    );
    assert!(matches!(
        outgoing_runtime.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));

    store.release.notify_one();
    moving
        .await
        .expect("plane-change task completes")
        .expect("handover completes");
    await_usable(&svc, &alice, &created.id).await;

    // Return to the terminal plane and retry the identical operation against a
    // replacement runtime. Rotation is a transient refusal, not a terminal one.
    svc.change_plane(&alice, &created.id, crate::options::SessionMode::Terminal)
        .await
        .expect("session can return to its terminal plane");
    await_usable(&svc, &alice, &created.id).await;

    let (replacement_input, mut replacement_runtime) = tokio::sync::mpsc::channel(1);
    svc.entries
        .write()
        .await
        .get_mut(&created.id)
        .expect("replacement entry is live")
        .input = Some(replacement_input);
    svc.send_input(&alice, &created.id, refused.clone())
        .await
        .expect("the refused input succeeds after rotation");
    assert!(matches!(
        replacement_runtime.recv().await,
        Some(TerminalInput::Bytes(bytes)) if bytes == refused
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_the_plane_change_caller_does_not_abandon_its_handover() {
    let store = Arc::new(RotationStore::default());
    let adapter = switch_capable();
    let (svc, alice, created, before) =
        started_adversarial(Arc::clone(&store), Arc::clone(&adapter)).await;
    let old_runtime = before.runtime_id.expect("old runtime is durable");

    // The second write completes the handover. Hold it after Rotating is
    // durable, then disconnect the request that initiated the move.
    store.arm_on_call(2);
    let caller = tokio::spawn({
        let svc = Arc::clone(&svc);
        let alice = alice.clone();
        let id = created.id.clone();
        async move {
            svc.change_plane(&alice, &id, crate::options::SessionMode::Conversation)
                .await
        }
    });
    store.wait_until_held().await;
    assert_eq!(
        store
            .inner
            .get(&created.id)
            .await
            .unwrap()
            .unwrap()
            .session
            .status,
        SessionStatus::Rotating
    );
    assert!(svc.starting.read().await.contains(&created.id));

    caller.abort();
    assert!(caller
        .await
        .expect_err("the request task was cancelled")
        .is_cancelled());
    store.release.notify_one();

    let settled = await_usable(&svc, &alice, &created.id).await;
    assert_eq!(settled.status, SessionStatus::Ready);
    assert_eq!(
        settled.options.mode,
        crate::options::SessionMode::Conversation
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        while svc.starting.read().await.contains(&created.id) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the service-owned handover releases its local claim");

    let durable = store.inner.get(&created.id).await.unwrap().unwrap();
    let replacement = durable
        .runtime_id
        .as_deref()
        .expect("the replacement runtime is durable");
    assert_ne!(replacement, old_runtime);
    assert_eq!(durable.rotation_claim, None);
    assert!(adapter.terminated.lock().await.contains(&old_runtime));
    assert!(adapter.running.lock().await.contains_key(replacement));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn outgoing_conversation_updates_rebase_across_the_final_handover_commit() {
    let adapter = switch_capable();
    let (svc, store, alice, created) = started(Arc::clone(&adapter)).await;

    svc.change_plane(
        &alice,
        &created.id,
        crate::options::SessionMode::Conversation,
    )
    .await
    .expect("the session moves onto its conversation plane");
    await_usable(&svc, &alice, &created.id).await;
    let outgoing_runtime = store
        .get(&created.id)
        .await
        .unwrap()
        .unwrap()
        .runtime_id
        .expect("the conversation runtime is durable");
    adapter
        .set_conversation(&outgoing_runtime, b"conversation before the move")
        .await;
    let updates = adapter
        .conversation
        .lock()
        .await
        .as_ref()
        .expect("the outgoing conversation is open")
        .updates
        .clone();
    let snapshot_call = adapter.snapshot_calls.load(Ordering::SeqCst) + 2;
    adapter.pause_snapshot_on_call(snapshot_call);

    let moving = tokio::spawn({
        let svc = Arc::clone(&svc);
        let alice = alice.clone();
        let id = created.id.clone();
        async move {
            svc.change_plane(&alice, &id, crate::options::SessionMode::Terminal)
                .await
        }
    });
    adapter.wait_until_snapshot_paused().await;

    updates
        .send(layer3_adapter::ConversationUpdate::Asked(
            crate::conversation_support::asking("req-during-rotation", "preserve this request"),
        ))
        .await
        .expect("the outgoing conversation still reports updates");
    updates
        .send(layer3_adapter::ConversationUpdate::TurnEnded(
            layer3_adapter::PromptResponse::new(layer3_adapter::TurnStopReason::EndTurn),
        ))
        .await
        .expect("the outgoing conversation still reports completion");
    updates
        .send(layer3_adapter::ConversationUpdate::Failed {
            message: "late failure from the outgoing runtime".into(),
            capability: None,
        })
        .await
        .expect("the outgoing conversation still reports failure");

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let events = svc
                .events_since(&alice, &created.id, None)
                .await
                .expect("history remains readable");
            let request = events.iter().any(|event| {
                matches!(
                    &event.kind,
                    EventKind::ApprovalRequested { request_id, .. }
                        if request_id == "req-during-rotation"
                )
            });
            let completions = events
                .iter()
                .filter(|event| matches!(event.kind, EventKind::TurnCompleted))
                .count();
            if request && completions >= 2 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("all outgoing updates become durable before the move completes");

    let while_moving = store.get(&created.id).await.unwrap().unwrap();
    assert_eq!(while_moving.session.status, SessionStatus::Rotating);
    assert!(while_moving.rotation_claim.is_some());
    assert_eq!(
        svc.get_session(&alice, &created.id)
            .await
            .expect("the live session remains readable")
            .status,
        SessionStatus::Rotating,
        "late completion and failure reports must not reopen the outgoing runtime"
    );

    adapter.resume_snapshot();
    moving
        .await
        .expect("plane-change task completes")
        .expect("the final commit rebases over outgoing history");
    let settled = await_usable(&svc, &alice, &created.id).await;
    assert_eq!(settled.options.mode, crate::options::SessionMode::Terminal);

    let durable = store.get(&created.id).await.unwrap().unwrap();
    assert_eq!(durable.rotation_claim, None);
    assert_ne!(
        durable.runtime_id.as_deref(),
        Some(outgoing_runtime.as_str())
    );
    let events = svc
        .events_since(&alice, &created.id, None)
        .await
        .expect("rebased history remains readable");
    assert!(events.iter().any(|event| {
        matches!(
            &event.kind,
            EventKind::ApprovalRequested { request_id, .. }
                if request_id == "req-during-rotation"
        )
    }));
    assert!(
        events
            .iter()
            .filter(|event| matches!(event.kind, EventKind::TurnCompleted))
            .count()
            >= 2
    );
    assert!(events.iter().any(|event| matches!(
        event.kind,
        EventKind::PlaneChanged {
            to: crate::options::SessionMode::Terminal,
            ..
        }
    )));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plane_change_revision_collision_never_switches_or_tears_down_the_runtime() {
    let store = Arc::new(RotationStore::default());
    let adapter = switch_capable();
    let (svc, alice, created, before) = started_adversarial(store.clone(), adapter.clone()).await;
    let old_runtime = before.runtime_id.clone().expect("old runtime is durable");
    store.arm();

    let moving_service = Arc::clone(&svc);
    let moving_owner = alice.clone();
    let moving_id = created.id.clone();
    let moving = tokio::spawn(async move {
        moving_service
            .change_plane(
                &moving_owner,
                &moving_id,
                crate::options::SessionMode::Conversation,
            )
            .await
    });
    store.wait_until_held().await;

    let mut rival = store.inner.get(&created.id).await.unwrap().unwrap();
    rival.session.status = SessionStatus::Running;
    let rival_seq = rival.session.last_seq + 1;
    rival.session.last_seq = rival_seq;
    store
        .inner
        .commit_session_idempotent(
            "rival-before-plane-change",
            &rival,
            &[Event {
                session_id: created.id.clone(),
                seq: rival_seq,
                kind: EventKind::StatusChanged {
                    status: SessionStatus::Running,
                    stop_reason: None,
                    detail: None,
                    capability: None,
                },
                appended_at: String::new(),
            }],
        )
        .await
        .expect("rival lifecycle decision commits first");
    store.release.notify_one();

    assert!(matches!(
        moving.await.expect("plane change task completes"),
        Err(crate::error::CoreError::RuntimeUnavailable(_))
    ));
    wait_for_rotation_lane(&svc, &created.id).await;
    let durable = store.inner.get(&created.id).await.unwrap().unwrap();
    assert_eq!(durable.session.status, SessionStatus::Running);
    assert_eq!(
        durable.session.options.mode,
        crate::options::SessionMode::Terminal
    );
    assert_eq!(durable.runtime_id.as_deref(), Some(old_runtime.as_str()));
    let replay = store
        .inner
        .events_since(&created.id, before.session.last_seq)
        .await
        .unwrap();
    assert_eq!(replay.len(), 1);
    assert!(replay.iter().all(|event| !matches!(
        event.kind,
        crate::event::StoredEventKind::Known(EventKind::PlaneChanged { .. })
    )));
    {
        let entries = svc.entries.read().await;
        let entry = entries.get(&created.id).expect("old runtime stays live");
        assert_eq!(entry.session.status, SessionStatus::Ready);
        assert_eq!(
            entry.session.options.mode,
            crate::options::SessionMode::Terminal
        );
        assert_eq!(
            entry.runtime.as_ref().map(|handle| handle.0.as_str()),
            Some(old_runtime.as_str())
        );
        assert!(entry.input.is_some());
        assert!(entry.conversation.is_none());
    }
    assert!(adapter.plane_switches.lock().await.is_empty());
    assert!(adapter.terminated.lock().await.is_empty());
    assert!(adapter.running.lock().await.contains_key(&old_runtime));
}

#[tokio::test]
async fn a_plane_change_ends_after_four_store_failures_without_runtime_side_effects() {
    let store = Arc::new(RotationStore::default());
    let adapter = switch_capable();
    let (svc, alice, created, before) = started_adversarial(store.clone(), adapter.clone()).await;
    let old_runtime = before.runtime_id.clone().expect("old runtime is durable");
    store.fail_next(4);

    assert!(matches!(
        svc.change_plane(
            &alice,
            &created.id,
            crate::options::SessionMode::Conversation,
        )
        .await,
        Err(crate::error::CoreError::RuntimeUnavailable(_))
    ));
    wait_for_rotation_lane(&svc, &created.id).await;
    assert_eq!(store.calls.load(Ordering::SeqCst), 4);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(store.calls.load(Ordering::SeqCst), 4);

    let durable = store.inner.get(&created.id).await.unwrap().unwrap();
    assert_eq!(durable.session.status, before.session.status);
    assert_eq!(durable.session.options, before.session.options);
    assert_eq!(durable.session.capabilities, before.session.capabilities);
    assert_eq!(durable.session.last_seq, before.session.last_seq);
    assert_eq!(durable.runtime_id.as_deref(), Some(old_runtime.as_str()));
    assert!(store
        .inner
        .events_since(&created.id, before.session.last_seq)
        .await
        .unwrap()
        .is_empty());
    {
        let entries = svc.entries.read().await;
        let entry = entries.get(&created.id).expect("old runtime stays live");
        assert_eq!(entry.session.status, SessionStatus::Ready);
        assert_eq!(entry.session.options, before.session.options);
        assert_eq!(entry.session.capabilities, before.session.capabilities);
        assert_eq!(
            entry.runtime.as_ref().map(|handle| handle.0.as_str()),
            Some(old_runtime.as_str())
        );
        assert!(entry.input.is_some());
        assert!(entry.conversation.is_none());
    }
    assert!(adapter.plane_switches.lock().await.is_empty());
    assert!(adapter.terminated.lock().await.is_empty());
    assert!(adapter.running.lock().await.contains_key(&old_runtime));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_final_plane_change_conflict_preserves_the_newer_record_and_old_runtime() {
    let inner = Arc::new(FakeStore::default());
    let store = Arc::new(CommitFaultStore::new(Arc::clone(&inner), is_plane_changed));
    let adapter = switch_capable();
    let (svc, alice, created, before) =
        started_with_commit_fault(store.clone(), adapter.clone()).await;
    let old_runtime = before.runtime_id.clone().expect("old runtime is durable");

    store.pause_next();
    let moving = tokio::spawn({
        let svc = Arc::clone(&svc);
        let alice = alice.clone();
        let id = created.id.clone();
        async move {
            svc.change_plane(&alice, &id, crate::options::SessionMode::Conversation)
                .await
        }
    });
    store.wait_until_held().await;
    assert_eq!(
        inner
            .get(&created.id)
            .await
            .unwrap()
            .unwrap()
            .session
            .status,
        SessionStatus::Rotating,
        "the selected fault is the second durable stage"
    );
    assert_eq!(
        svc.entries
            .read()
            .await
            .get(&created.id)
            .expect("old entry remains")
            .session
            .status,
        SessionStatus::Rotating
    );
    assert!(adapter.running.lock().await.contains_key(&old_runtime));
    assert!(adapter.terminated.lock().await.is_empty());

    let mut rival = inner.get(&created.id).await.unwrap().unwrap();
    let rival_seq = rival.session.last_seq + 1;
    rival.session.status = SessionStatus::Ready;
    rival.session.last_seq = rival_seq;
    rival.context_ref = Some("rival-between-rotation-stages".into());
    inner
        .commit_session_idempotent(
            "rival-between-rotation-stages",
            &rival,
            &[Event {
                session_id: created.id.clone(),
                seq: rival_seq,
                kind: EventKind::StatusChanged {
                    status: SessionStatus::Ready,
                    stop_reason: None,
                    detail: None,
                    capability: None,
                },
                appended_at: String::new(),
            }],
        )
        .await
        .expect("the outgoing runtime settles before PlaneChanged");
    let (newer_input, mut outgoing_runtime) = tokio::sync::mpsc::channel(1);
    {
        let mut entries = svc.entries.write().await;
        let entry = entries
            .get_mut(&created.id)
            .expect("old entry remains live");
        entry.session.status = SessionStatus::Ready;
        entry.session.last_seq = rival_seq;
        entry.input = Some(newer_input);
    }
    svc.send_input(&alice, &created.id, b"accepted by newer state".to_vec())
        .await
        .expect("the newer Ready state accepts input while the stale final CAS waits");
    assert!(matches!(
        outgoing_runtime.recv().await,
        Some(TerminalInput::Bytes(bytes)) if bytes == b"accepted by newer state"
    ));
    store.release();

    assert!(matches!(
        moving.await.expect("plane change task completes"),
        Err(crate::error::CoreError::RuntimeUnavailable(_))
    ));
    wait_for_rotation_lane(&svc, &created.id).await;
    assert_eq!(store.matched_calls(), 1);
    let durable = inner.get(&created.id).await.unwrap().unwrap();
    assert_eq!(durable.session.status, SessionStatus::Ready);
    assert_eq!(durable.session.options, before.session.options);
    assert_eq!(durable.session.capabilities, before.session.capabilities);
    assert_eq!(durable.session.rotations, before.session.rotations);
    assert_eq!(durable.runtime_id.as_deref(), Some(old_runtime.as_str()));
    assert_eq!(
        durable.context_ref.as_deref(),
        Some("rival-between-rotation-stages")
    );
    assert!(inner
        .events_since(&created.id, before.session.last_seq)
        .await
        .unwrap()
        .iter()
        .all(|event| !matches!(
            event.kind,
            crate::event::StoredEventKind::Known(EventKind::PlaneChanged { .. })
        )));
    {
        let entries = svc.entries.read().await;
        let entry = entries
            .get(&created.id)
            .expect("old entry remains installed");
        assert_eq!(entry.session.status, SessionStatus::Ready);
        assert_eq!(entry.session.options, before.session.options);
        assert_eq!(
            entry.runtime.as_ref().map(|handle| handle.0.as_str()),
            Some(old_runtime.as_str())
        );
        assert!(entry.input.is_some());
        assert!(entry.conversation.is_none());
    }
    assert!(adapter.running.lock().await.contains_key(&old_runtime));
    assert!(adapter.terminated.lock().await.is_empty());
    assert!(
        adapter
            .senders
            .lock()
            .await
            .last()
            .is_some_and(|sender| !sender.is_closed()),
        "the old output relay remains connected"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_older_final_cas_cannot_steal_a_newer_rotation_of_the_same_runtime() {
    let inner = Arc::new(FakeStore::default());
    let store = Arc::new(CommitFaultStore::new(Arc::clone(&inner), is_plane_changed));
    let adapter = switch_capable();
    let (svc, alice, created, before) =
        started_with_commit_fault(store.clone(), adapter.clone()).await;
    let old_runtime = before.runtime_id.clone().expect("old runtime is durable");

    store.pause_next();
    let moving = tokio::spawn({
        let svc = Arc::clone(&svc);
        let alice = alice.clone();
        let id = created.id.clone();
        async move {
            svc.change_plane(&alice, &id, crate::options::SessionMode::Conversation)
                .await
        }
    });
    store.wait_until_held().await;

    let mut readopted = inner.get(&created.id).await.unwrap().unwrap();
    assert_eq!(readopted.session.status, SessionStatus::Rotating);
    readopted.session.status = SessionStatus::Ready;
    readopted.rotation_claim = None;
    readopted.context_ref = Some("readopted-between-rotation-claims".into());
    inner
        .commit_session_idempotent("readopted-between-rotation-claims", &readopted, &[])
        .await
        .expect("a replacement process settles the older claim");

    let mut newer_rotation = inner.get(&created.id).await.unwrap().unwrap();
    newer_rotation.session.status = SessionStatus::Rotating;
    newer_rotation.rotation_claim = Some("rotn_newer_rotation_claim".into());
    newer_rotation.context_ref = Some("newer-rotation-claim".into());
    let newer_revision = inner
        .commit_session_idempotent("newer-rotation-claim", &newer_rotation, &[])
        .await
        .expect("a newer rotation claims the same still-running runtime");

    store.release();
    assert!(matches!(
        moving.await.expect("older plane change task completes"),
        Err(crate::error::CoreError::RuntimeUnavailable(_))
    ));
    wait_for_rotation_lane(&svc, &created.id).await;

    let durable = inner.get(&created.id).await.unwrap().unwrap();
    assert_eq!(durable.revision, newer_revision);
    assert_eq!(durable.session.status, SessionStatus::Rotating);
    assert_eq!(
        durable.rotation_claim.as_deref(),
        Some("rotn_newer_rotation_claim")
    );
    assert_eq!(durable.context_ref.as_deref(), Some("newer-rotation-claim"));
    assert_eq!(durable.runtime_id.as_deref(), Some(old_runtime.as_str()));
    assert_eq!(durable.session.options, before.session.options);
    assert_eq!(durable.session.rotations, before.session.rotations);
    assert!(adapter.terminated.lock().await.is_empty());
    assert!(adapter.running.lock().await.contains_key(&old_runtime));
}

#[tokio::test]
async fn four_final_plane_change_outages_restore_without_a_false_change() {
    let inner = Arc::new(FakeStore::default());
    let store = Arc::new(CommitFaultStore::new(Arc::clone(&inner), is_plane_changed));
    let adapter = switch_capable();
    let (svc, alice, created, before) =
        started_with_commit_fault(store.clone(), adapter.clone()).await;
    let old_runtime = before.runtime_id.clone().expect("old runtime is durable");

    store.fail_next(4);
    assert!(matches!(
        svc.change_plane(
            &alice,
            &created.id,
            crate::options::SessionMode::Conversation,
        )
        .await,
        Err(crate::error::CoreError::RuntimeUnavailable(_))
    ));
    wait_for_rotation_lane(&svc, &created.id).await;
    assert_eq!(store.matched_calls(), 4);

    let durable = inner.get(&created.id).await.unwrap().unwrap();
    assert_eq!(durable.session.status, before.session.status);
    assert_eq!(durable.session.options, before.session.options);
    assert_eq!(durable.session.capabilities, before.session.capabilities);
    assert_eq!(durable.session.rotations, before.session.rotations);
    assert_eq!(durable.runtime_id.as_deref(), Some(old_runtime.as_str()));
    assert!(inner
        .events_since(&created.id, before.session.last_seq)
        .await
        .unwrap()
        .iter()
        .all(|event| !matches!(
            event.kind,
            crate::event::StoredEventKind::Known(EventKind::PlaneChanged { .. })
        )));
    let entries = svc.entries.read().await;
    let entry = entries.get(&created.id).expect("old entry remains usable");
    assert_eq!(entry.session.status, before.session.status);
    assert_eq!(entry.session.options, before.session.options);
    assert_eq!(
        entry.runtime.as_ref().map(|handle| handle.0.as_str()),
        Some(old_runtime.as_str())
    );
    assert!(entry.input.is_some());
    assert!(entry.conversation.is_none());
    drop(entries);
    assert!(adapter.running.lock().await.contains_key(&old_runtime));
    assert!(adapter.terminated.lock().await.is_empty());
}

#[tokio::test]
async fn the_change_is_written_where_its_owner_reads_their_history() {
    // A surface that changed under somebody, with nothing in the log saying
    // so, is a session that appears to have rewritten itself.
    let (svc, _store, alice, created) = started(ambidextrous()).await;

    svc.change_plane(
        &alice,
        &created.id,
        crate::options::SessionMode::Conversation,
    )
    .await
    .expect("moved");
    await_usable(&svc, &alice, &created.id).await;

    let events = svc
        .events_since(&alice, &created.id, None)
        .await
        .expect("readable");
    let change = events
        .iter()
        .find_map(|e| match &e.kind {
            crate::event::EventKind::PlaneChanged {
                from,
                to,
                snapshot_at,
            } => Some((*from, *to, snapshot_at.clone())),
            _ => None,
        })
        .expect("the change must be a durable event");
    assert_eq!(change.0, crate::options::SessionMode::Terminal);
    assert_eq!(change.1, crate::options::SessionMode::Conversation);
    assert!(
        !change.2.is_empty(),
        "the moment the conversation was taken is what says where history stops being continuous"
    );
}

#[tokio::test]
async fn a_runtime_that_does_not_serve_the_plane_refuses_by_name() {
    // The typed refusal, before anything is torn down. The default fake
    // renders a screen and holds no conversation, which is exactly the kernel
    // this must refuse for.
    let adapter = Arc::new(FakeAdapter::default());
    let (svc, _store, alice, created) = started(Arc::clone(&adapter)).await;

    let refusal = svc
        .change_plane(
            &alice,
            &created.id,
            crate::options::SessionMode::Conversation,
        )
        .await
        .expect_err("a screen-only runtime cannot hold a conversation");

    assert!(
        matches!(&refusal, crate::error::CoreError::CapabilityUnsupported(name) if name == "conversation"),
        "got {refusal:?}"
    );
    let after = svc
        .get_session(&alice, &created.id)
        .await
        .expect("readable");
    assert_eq!(
        after.status,
        SessionStatus::Ready,
        "a refused session keeps running exactly as it was"
    );
    assert_eq!(after.rotations, 0);
    assert!(adapter.terminated.lock().await.is_empty());
}

#[tokio::test]
async fn a_runtime_that_cannot_hand_its_conversation_over_refuses_rather_than_tries() {
    // The fail-safe. Moving planes means capturing and resuming, so a kernel
    // that cannot be captured would be trading the conversation for the
    // surface — the §5 defect, done deliberately.
    let adapter = Arc::new(FakeAdapter {
        capabilities: layer3_adapter::KernelCapabilities {
            conversation: true,
            snapshot: false,
            ..FakeAdapter::default().capabilities
        },
        ..FakeAdapter::default()
    });
    let (svc, _store, alice, created) = started(Arc::clone(&adapter)).await;

    let refusal = svc
        .change_plane(
            &alice,
            &created.id,
            crate::options::SessionMode::Conversation,
        )
        .await
        .expect_err("a session that cannot be carried must not be moved");

    assert!(
        matches!(&refusal, crate::error::CoreError::CapabilityUnsupported(name) if name == "snapshot"),
        "got {refusal:?}"
    );
    let after = svc
        .get_session(&alice, &created.id)
        .await
        .expect("readable");
    assert_eq!(after.status, SessionStatus::Ready);
    assert!(adapter.terminated.lock().await.is_empty());
}

#[tokio::test]
async fn a_session_already_being_moved_refuses_a_second_move() {
    // Two handovers at once would capture a sandbox being torn down and resume
    // from whichever answered last.
    let (svc, store, alice, created) = started(ambidextrous()).await;
    let mut record = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present");
    record.session.status = SessionStatus::Rotating;
    store.seed(&record).await.expect("recorded");

    let refusal = svc
        .change_plane(
            &alice,
            &created.id,
            crate::options::SessionMode::Conversation,
        )
        .await
        .expect_err("a session mid-handover cannot be handed over again");

    assert!(
        matches!(refusal, crate::error::CoreError::InvalidState),
        "got {refusal:?}"
    );
}

#[tokio::test]
async fn a_session_with_no_runtime_refuses() {
    // A stopped session is started on whichever plane its profile names when
    // it is revived. Rotating one would mean carrying a sandbox that is gone.
    let (svc, _store, alice, created) = started(ambidextrous()).await;
    svc.stop_session(&alice, &created.id)
        .await
        .expect("stopped");

    let refusal = svc
        .change_plane(
            &alice,
            &created.id,
            crate::options::SessionMode::Conversation,
        )
        .await
        .expect_err("a stopped session has nothing to move");

    assert!(
        matches!(refusal, crate::error::CoreError::InvalidState),
        "got {refusal:?}"
    );
}

#[tokio::test]
async fn only_the_owner_may_change_a_sessions_plane() {
    // Stopping somebody else's session is an operator's job; deciding what
    // they are looking at is not.
    let (svc, _store, _alice, created) = started(ambidextrous()).await;

    let refusal = svc
        .change_plane(
            &crate::test_support::admin(),
            &created.id,
            crate::options::SessionMode::Conversation,
        )
        .await
        .expect_err("an operator grant does not stand in for the owner here");

    assert!(
        matches!(refusal, crate::error::CoreError::Forbidden),
        "got {refusal:?}"
    );
}

#[tokio::test]
async fn asking_for_the_plane_a_session_is_already_on_costs_nothing() {
    // Not an error and not a rotation: a handover to arrive where it already
    // is would be a cost with nothing on the other side of it.
    let adapter = ambidextrous();
    let (svc, _store, alice, created) = started(Arc::clone(&adapter)).await;

    let unchanged = svc
        .change_plane(&alice, &created.id, crate::options::SessionMode::Terminal)
        .await
        .expect("already there");

    assert_eq!(unchanged.status, SessionStatus::Ready);
    assert_eq!(unchanged.rotations, 0);
    assert!(adapter.terminated.lock().await.is_empty());
}

#[tokio::test]
async fn a_terminal_session_nobody_moves_is_untouched() {
    // The regression this whole issue must not cause: adding the operation
    // must not change what a session does when nobody calls it.
    let adapter = ambidextrous();
    let (svc, _store, alice, created) = started(Arc::clone(&adapter)).await;

    let settled = svc
        .get_session(&alice, &created.id)
        .await
        .expect("readable");

    assert_eq!(settled.status, SessionStatus::Ready);
    assert_eq!(settled.rotations, 0);
    assert!(settled.rotated_at.is_none());
    assert!(settled.capabilities.terminal && !settled.capabilities.conversation);
}

#[tokio::test]
async fn a_rotation_whose_replacement_cannot_start_does_not_end_the_session() {
    // CLAUDE.md §5: no deployment may interrupt a session its owner has not
    // stopped, and losing one is as severe as losing the data. A rotation is
    // caused by a deployment — moving the runtime image makes every live
    // session due for one — so its replacement failing is a fact about a
    // provider at a moment, not about the session.
    //
    // Four sessions were lost exactly this way during the v0.3.0 deploy. Their
    // logs read "moving this session to an updated runtime; it will come back
    // where it left off", then "the runtime did not become usable within
    // 600s", then nothing: terminated and retired. Each promised in its own
    // record that it would come back, and did not come back at all.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let old = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:1");
    let created = old.create_session(&alice, spec()).await.expect("created");
    await_settled(&old, &alice, &created.id).await;
    drop(old);

    // The provider stops giving out runtimes, which is what a burst of
    // rotations does to one, and the deployment happens anyway.
    let refusing = Arc::new(FakeAdapter {
        refuse_provision: true,
        ..FakeAdapter::default()
    });
    let new = service_on_version(Arc::clone(&store), Arc::clone(&refusing), "runtime:2");
    new.reconcile().await;
    let settled = await_settled(&new, &alice, &created.id).await;

    assert_ne!(
        settled.status,
        SessionStatus::Terminated,
        "a rotation its owner did not ask for must not end their session"
    );
    assert_ne!(
        settled.recovery_state,
        crate::session::RecoveryState::Retired,
        "and it must stay recoverable, or nothing will ever bring it back"
    );
    assert!(
        new.list_sessions(&alice, &crate::SessionListQuery::default())
            .await
            .expect("session list is readable")
            .items
            .iter()
            .any(|s| s.id == created.id),
        "it keeps its place in its owner's list"
    );
}

#[tokio::test]
async fn a_permanently_refused_replacement_preserves_the_old_authority() {
    // A permanent refusal says this replacement can never serve the session;
    // it does not authorize destroying the old runtime that still can. Until
    // an operator chooses otherwise, the exact pre-switch rollback keeps the
    // existing session usable on its older runtime.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let old = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:1");
    let created = old.create_session(&alice, spec()).await.expect("created");
    let ready = await_settled(&old, &alice, &created.id).await;
    let before_attempt = ready.last_seq;
    let old_runtime = store
        .get(&created.id)
        .await
        .expect("the old session is readable")
        .expect("the old session is present")
        .runtime_id
        .expect("the old session still names its runtime");
    drop(old);

    // The same kernel, rebuilt without the ability this session was granted.
    let lesser = Arc::new(FakeAdapter {
        capabilities: layer3_adapter::KernelCapabilities::default(),
        ..FakeAdapter::default()
    });
    lesser
        .running
        .lock()
        .await
        .insert(old_runtime.clone(), Some(created.id.0.clone()));
    let new = service_on_version(Arc::clone(&store), Arc::clone(&lesser), "runtime:2");
    new.reconcile().await;
    let settled = await_settled(&new, &alice, &created.id).await;

    assert_eq!(settled.status, SessionStatus::Ready);
    assert_eq!(
        store
            .get(&created.id)
            .await
            .unwrap()
            .unwrap()
            .runtime_id
            .as_deref(),
        Some(old_runtime.as_str())
    );
    assert_eq!(lesser.provision_calls.load(Ordering::SeqCst), 0);

    let failed_attempt = new
        .events_since(&alice, &created.id, Some(before_attempt))
        .await
        .expect("the bounded failed handover is readable")
        .into_iter()
        .filter(|event| event.seq <= settled.last_seq)
        .collect::<Vec<_>>();
    let rotating = failed_attempt.iter().position(|event| {
        matches!(
            &event.kind,
            EventKind::StatusChanged {
                status: SessionStatus::Rotating,
                ..
            }
        )
    });
    let terminated = failed_attempt.iter().position(|event| {
        matches!(
            &event.kind,
            EventKind::StatusChanged {
                status: SessionStatus::Terminated,
                ..
            }
        )
    });
    assert!(
        rotating
            .zip(terminated)
            .is_some_and(|(start, end)| start < end),
        "the bounded failed attempt did not end after it began: {failed_attempt:?}"
    );
    assert!(
        failed_attempt.iter().all(|event| !matches!(
            &event.kind,
            EventKind::StatusChanged {
                status: SessionStatus::Ready,
                ..
            }
        )),
        "a permanently failed attempt fabricated readiness inside its bounded cursor window: \
         {failed_attempt:?}"
    );
}

/// A source that refuses, the way the registry refuses this service's own
/// identity asking for somebody's private skill.
struct RefusingSkills;

#[async_trait::async_trait]
impl crate::store::SkillSource for RefusingSkills {
    async fn packs_for(
        &self,
        _caller_credential: Option<&str>,
        _skills: &[crate::environment::SkillId],
    ) -> Result<Vec<crate::store::SkillPack>, String> {
        Err("the registry answered 403 Forbidden".to_string())
    }
}

#[tokio::test]
async fn a_session_whose_skills_cannot_be_refetched_is_still_carried() {
    /*
     * This ended three live sessions the first time skill installation
     * shipped.
     *
     * A rotation has no caller and no credential of theirs to fetch with, so
     * the registry is asked as this service, which sees only public skills and
     * answers 403. Treating that the way a creation treats it — refuse, do not
     * retry — terminates a session somebody is working in, to avoid it missing
     * a file. Losing the work is far worse than losing the skill.
     */
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let old = service_on_version_with_skills(
        Arc::clone(&store),
        Arc::clone(&adapter),
        "runtime:1",
        Arc::new(RefusingSkills),
    );
    let created = old.create_session(&alice, spec()).await;
    // At creation the caller is on the line, so the refusal is theirs to hear.
    assert!(
        created.is_err() || created.is_ok(),
        "creation may go either way here; what matters is the rotation below"
    );

    // A session that exists, on an older runtime, with no caller present.
    let store2 = Arc::new(FakeStore::default());
    let adapter2 = Arc::new(FakeAdapter::default());
    let first = service_on_version(Arc::clone(&store2), Arc::clone(&adapter2), "runtime:1");
    let session = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &session.id).await;

    let newer = service_on_version_with_skills(
        Arc::clone(&store2),
        Arc::clone(&adapter2),
        "runtime:2",
        Arc::new(RefusingSkills),
    );
    newer.reconcile().await;

    let after = await_usable(&newer, &alice, &session.id).await;
    assert_ne!(
        after.status,
        SessionStatus::Terminated,
        "a skill that could not be refetched must not end the session"
    );
}

/// A source that hands back whatever was asked for, the way the registry
/// answers a caller whose credential can actually see the skill.
struct WorkingSkills;

#[async_trait::async_trait]
impl crate::store::SkillSource for WorkingSkills {
    async fn packs_for(
        &self,
        _caller_credential: Option<&str>,
        skills: &[crate::environment::SkillId],
    ) -> Result<Vec<crate::store::SkillPack>, String> {
        Ok(skills
            .iter()
            .map(|id| crate::store::SkillPack {
                name: id.0.clone(),
                version: Some("1.0".to_string()),
                files: vec![("SKILL.md".to_string(), b"# a skill".to_vec())],
            })
            .collect())
    }
}

#[tokio::test]
async fn the_record_says_what_a_session_has_and_not_what_it_asked_for() {
    /*
     * The other half of the test above, and the reason it is not enough that
     * the session survives.
     *
     * A rotation that cannot refetch carries the session forward with no
     * skills. If the record still lists the ones it was created with, every
     * interface goes on reporting them — which is worse than reporting none,
     * because somebody is then told their agent can do something it cannot,
     * and only finds out when it fails.
     *
     * #442 is about not losing them in the first place. Until that lands the
     * record must at least agree with the disk.
     */
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let first = service_on_version_with_skills(
        Arc::clone(&store),
        Arc::clone(&adapter),
        "runtime:1",
        Arc::new(WorkingSkills),
    );
    let session = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &session.id).await;

    let created = store
        .get(&session.id)
        .await
        .expect("readable")
        .expect("recorded");
    assert_eq!(
        created.session.granted_skills.as_deref(),
        Some(
            &[crate::environment::GrantedSkill {
                id: crate::environment::SkillId::new("a-mandated-skill"),
                version: Some("1.0".to_string()),
            }][..]
        ),
        "a session that got its skills should say so"
    );

    let newer = service_on_version_with_skills(
        Arc::clone(&store),
        Arc::clone(&adapter),
        "runtime:2",
        Arc::new(RefusingSkills),
    );
    newer.reconcile().await;
    await_usable(&newer, &alice, &session.id).await;

    let rotated = store
        .get(&session.id)
        .await
        .expect("readable")
        .expect("recorded");
    assert_eq!(
        rotated.session.granted_skills.as_deref(),
        Some(&[][..]),
        "a rotation that lost the skills must not leave the record claiming them"
    );
}

#[tokio::test]
async fn a_rotation_that_keeps_its_skills_keeps_them_in_the_record() {
    /*
     * The mutation the test above would not catch: clearing the record on
     * every rotation. Then it agrees with the disk by always saying "nothing",
     * which is a different lie.
     */
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let first = service_on_version_with_skills(
        Arc::clone(&store),
        Arc::clone(&adapter),
        "runtime:1",
        Arc::new(WorkingSkills),
    );
    let session = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &session.id).await;

    let newer = service_on_version_with_skills(
        Arc::clone(&store),
        Arc::clone(&adapter),
        "runtime:2",
        Arc::new(WorkingSkills),
    );
    newer.reconcile().await;
    await_usable(&newer, &alice, &session.id).await;

    let rotated = store
        .get(&session.id)
        .await
        .expect("readable")
        .expect("recorded");
    assert_eq!(
        rotated.session.granted_skills.as_deref(),
        Some(
            &[crate::environment::GrantedSkill {
                id: crate::environment::SkillId::new("a-mandated-skill"),
                version: Some("1.0".to_string()),
            }][..]
        ),
        "a rotation that refetched its skills still has them"
    );
}
