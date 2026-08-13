//! Tests for ending and removing sessions.
//!
//! Terminating and deleting are different acts on purpose, and the difference
//! is only observable across a restart and against the runtime, so these are
//! written against the durable record rather than against live memory.

use std::sync::Arc;

use crate::error::CoreError;
use crate::ids::SessionId;
use crate::session::SessionStatus;
use crate::store::SessionStore;
use crate::terminal::TerminalGeometry;
use crate::test_support::{service, spec, user, FakeAdapter, FakeStore};

#[tokio::test]
async fn a_terminated_session_remains_listed_until_it_is_deleted() {
    // Terminate and delete are different acts. A finished conversation is
    // still worth reading, so ending one must not remove it.
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    let created = svc.create_session(&caller, spec()).await.expect("created");

    svc.terminate_session(&caller, &created.id)
        .await
        .expect("terminates");
    assert_eq!(
        svc.list_sessions(&caller).await.len(),
        1,
        "terminating must not remove the session"
    );

    svc.delete_session(&caller, &created.id)
        .await
        .expect("deletes");
    assert!(
        svc.list_sessions(&caller).await.is_empty(),
        "deleting must remove it"
    );
}
#[tokio::test]
async fn deleting_a_running_session_tears_its_runtime_down_first() {
    // Otherwise the record vanishes while the sandbox keeps running, and the
    // only handle to it is gone.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let caller = user("alice");
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = svc.create_session(&caller, spec()).await.expect("created");

    svc.delete_session(&caller, &created.id)
        .await
        .expect("deletes");

    assert_eq!(
        adapter.terminated.lock().await.len(),
        1,
        "a running session must be stopped before its record is removed"
    );
    assert!(matches!(
        svc.get_session(&caller, &created.id).await,
        Err(CoreError::SessionNotFound)
    ));
}
#[tokio::test]
async fn deleting_removes_the_durable_history_too() {
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    let created = svc.create_session(&caller, spec()).await.expect("created");
    svc.delete_session(&caller, &created.id)
        .await
        .expect("deletes");

    let leftover = store
        .events_since(&created.id, 0)
        .await
        .expect("store readable");
    assert!(
        leftover.is_empty(),
        "history must not outlive the session its owner removed"
    );
}
#[tokio::test]
async fn a_terminated_session_cannot_be_attached_to() {
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    let created = svc.create_session(&caller, spec()).await.expect("created");
    svc.terminate_session(&caller, &created.id)
        .await
        .expect("terminates");

    assert!(matches!(
        svc.attach_terminal(&caller, &created.id, TerminalGeometry::default())
            .await,
        Err(CoreError::SessionTerminated)
    ));
}
#[tokio::test]
async fn terminating_twice_is_not_an_error() {
    // The button is in a browser; a double click must not surface a failure.
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    let created = svc.create_session(&caller, spec()).await.expect("created");

    svc.terminate_session(&caller, &created.id)
        .await
        .expect("first terminate succeeds");
    svc.terminate_session(&caller, &created.id)
        .await
        .expect("terminating an already-terminated session is idempotent");
}
#[tokio::test]
async fn deleting_a_session_that_is_already_gone_reports_not_found() {
    let store = Arc::new(FakeStore::default());
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    assert!(matches!(
        svc.delete_session(&user("alice"), &SessionId("sesn_missing".into()))
            .await,
        Err(CoreError::SessionNotFound)
    ));
}
#[tokio::test]
async fn one_subject_cannot_terminate_another_subjects_persisted_session() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let created = service(Arc::clone(&store), Arc::clone(&adapter))
        .create_session(&user("alice"), spec())
        .await
        .expect("created");

    let restarted = service(Arc::clone(&store), Arc::clone(&adapter));
    assert!(matches!(
        restarted
            .terminate_session(&user("mallory"), &created.id)
            .await,
        Err(CoreError::Forbidden)
    ));
    assert!(
        adapter.terminated.lock().await.is_empty(),
        "a forbidden request must not reach the runtime"
    );
}
#[tokio::test]
async fn a_session_can_be_terminated_after_a_restart() {
    // The sandbox outlives the control plane. If terminate only worked against
    // live memory, a restart would strand a running runtime with no way to
    // reclaim it — and the user's terminate button is the only reaper there is.
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let created = service(Arc::clone(&store), Arc::new(FakeAdapter::default()))
        .create_session(&caller, spec())
        .await
        .expect("session is created");

    let adapter = Arc::new(FakeAdapter::default());
    let restarted = service(Arc::clone(&store), Arc::clone(&adapter));
    restarted
        .terminate_session(&caller, &created.id)
        .await
        .expect("a persisted session can be terminated");

    assert_eq!(
        adapter.terminated.lock().await.clone(),
        vec![format!("runtime-for-{}", created.id.0)],
        "the runtime recorded in the store must actually be torn down"
    );

    let after = restarted
        .get_session(&caller, &created.id)
        .await
        .expect("a terminated session is still readable");
    assert_eq!(after.status, SessionStatus::Terminated);
    let record = store
        .get(&created.id)
        .await
        .expect("store readable")
        .expect("record present");
    assert!(
        record.runtime_id.is_none(),
        "a torn-down runtime must not still be referenced"
    );
}
#[tokio::test]
async fn terminating_records_the_status_change_durably() {
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    let created = svc
        .create_session(&caller, spec())
        .await
        .expect("session is created");
    let before = svc
        .events_since(&caller, &created.id, None)
        .await
        .expect("history readable")
        .len();

    service(Arc::clone(&store), Arc::new(FakeAdapter::default()))
        .terminate_session(&caller, &created.id)
        .await
        .expect("terminates");

    let after = svc
        .events_since(&caller, &created.id, None)
        .await
        .expect("history readable");
    assert!(
        after.len() > before,
        "terminating must append to the durable log"
    );
}
