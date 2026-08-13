//! Tests for what survives the process that created it.
//!
//! Persistence exists so a session outlives the control plane that started it.
//! These tests express that as the thing it actually is: two service instances
//! sharing one store, where the second never saw the first's memory.

use std::sync::Arc;

use crate::authz::{Authorization, Caller};
use crate::error::CoreError;
use crate::ids::SubjectId;
use crate::session::SessionStatus;
use crate::store::SessionStore;
use crate::terminal::TerminalGeometry;
use crate::test_support::{admin, service, spec, user, FakeAdapter, FakeStore};

#[tokio::test]
async fn a_session_is_readable_after_the_process_that_created_it_is_gone() {
    // The reason persistence exists. The second service never saw the first's
    // memory, so anything it can read came out of the store.
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");

    let created = service(Arc::clone(&store), Arc::new(FakeAdapter::default()))
        .create_session(&caller, spec())
        .await
        .expect("session is created");

    let restarted = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    let found = restarted
        .get_session(&caller, &created.id)
        .await
        .expect("the session survives the restart");

    assert_eq!(found.id, created.id);
    assert_eq!(found.owner, created.owner);
    assert_eq!(found.status, SessionStatus::Ready);
}

#[tokio::test]
async fn a_restarted_service_still_lists_the_session() {
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    service(Arc::clone(&store), Arc::new(FakeAdapter::default()))
        .create_session(&caller, spec())
        .await
        .expect("session is created");

    let listed = service(Arc::clone(&store), Arc::new(FakeAdapter::default()))
        .list_sessions(&caller)
        .await;
    assert_eq!(listed.len(), 1, "the list must come from the store");
}

#[tokio::test]
async fn history_written_before_a_restart_is_readable_after_it() {
    // Events are the session's transcript of record. If they lived only in
    // memory, a restart would silently truncate a client's cursor.
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let created = service(Arc::clone(&store), Arc::new(FakeAdapter::default()))
        .create_session(&caller, spec())
        .await
        .expect("session is created");

    let events = service(Arc::clone(&store), Arc::new(FakeAdapter::default()))
        .events_since(&caller, &created.id, None)
        .await
        .expect("history is readable");

    assert!(!events.is_empty(), "creation must be recorded durably");
    let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
    let mut sorted = seqs.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(seqs, sorted, "sequences must be monotonic and unique");
}

#[tokio::test]
async fn one_subject_cannot_read_another_subjects_persisted_session() {
    // Ownership is enforced against the stored record, so it holds after a
    // restart just as it did in memory.
    let store = Arc::new(FakeStore::default());
    let created = service(Arc::clone(&store), Arc::new(FakeAdapter::default()))
        .create_session(&user("alice"), spec())
        .await
        .expect("created");

    let restarted = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    assert!(matches!(
        restarted.get_session(&user("mallory"), &created.id).await,
        Err(CoreError::Forbidden)
    ));
    assert!(
        restarted.list_sessions(&user("mallory")).await.is_empty(),
        "another subject's sessions must not be listed"
    );
}

#[tokio::test]
async fn a_privileged_reader_sees_persisted_sessions_it_does_not_own() {
    let store = Arc::new(FakeStore::default());
    service(Arc::clone(&store), Arc::new(FakeAdapter::default()))
        .create_session(&user("alice"), spec())
        .await
        .expect("created");
    service(Arc::clone(&store), Arc::new(FakeAdapter::default()))
        .create_session(&user("bob"), spec())
        .await
        .expect("created");

    let listed = service(Arc::clone(&store), Arc::new(FakeAdapter::default()))
        .list_sessions(&admin())
        .await;
    assert_eq!(listed.len(), 2, "the dashboard must see every session");
}

#[tokio::test]
async fn a_caller_without_capabilities_cannot_create_a_session() {
    let store = Arc::new(FakeStore::default());
    let nobody = Caller::new(SubjectId("nobody".into()), Authorization::none());
    assert!(matches!(
        service(Arc::clone(&store), Arc::new(FakeAdapter::default()))
            .create_session(&nobody, spec())
            .await,
        Err(CoreError::Forbidden)
    ));
    assert!(
        store.list_all().await.expect("readable").is_empty(),
        "a refused request must write nothing"
    );
}

#[tokio::test]
async fn an_unreachable_store_is_reported_rather_than_read_as_absent() {
    // A store that is down must not look like a session that never existed:
    // one is a transient fault, the other tells the user their work is gone.
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let created = service(Arc::clone(&store), Arc::new(FakeAdapter::default()))
        .create_session(&caller, spec())
        .await
        .expect("created");

    store.fail_everything().await;
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    assert!(
        matches!(
            svc.get_session(&caller, &created.id).await,
            Err(CoreError::RuntimeUnavailable(_))
        ),
        "an unreachable store must not report the session as missing"
    );
}

#[tokio::test]
async fn a_session_cannot_be_created_if_it_cannot_be_persisted() {
    // Returning a session id that outlives nothing would hand the caller a
    // handle to work no restart can recover.
    let store = Arc::new(FakeStore::default());
    store.fail_everything().await;
    let result = service(Arc::clone(&store), Arc::new(FakeAdapter::default()))
        .create_session(&user("alice"), spec())
        .await;
    assert!(
        matches!(result, Err(CoreError::RuntimeUnavailable(_))),
        "creation must fail when the record cannot be written, got {result:?}"
    );
}

#[tokio::test]
async fn a_persisted_session_reports_no_runtime_rather_than_attaching_blindly() {
    // After a restart the record exists but the relay does not. Attaching must
    // say so plainly, because reviving is the caller's next step.
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let created = service(Arc::clone(&store), Arc::new(FakeAdapter::default()))
        .create_session(&caller, spec())
        .await
        .expect("created");

    let restarted = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    let result = restarted
        .attach_terminal(&caller, &created.id, TerminalGeometry::default())
        .await;
    assert!(
        matches!(result, Err(CoreError::RuntimeUnavailable(_))),
        "expected a clear 'no runtime' answer, got {:?}",
        result.map(|_| "an attachment")
    );
}

#[tokio::test]
async fn the_stored_runtime_reference_is_what_a_later_process_reclaims() {
    // The record is the only route from a restarted process back to a running
    // sandbox, so it must carry the handle from the moment the session exists.
    let store = Arc::new(FakeStore::default());
    let created = service(Arc::clone(&store), Arc::new(FakeAdapter::default()))
        .create_session(&user("alice"), spec())
        .await
        .expect("created");

    let record = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present");
    assert_eq!(
        record.runtime_id.as_deref(),
        Some(format!("runtime-for-{}", created.id.0).as_str())
    );
}

/// Terminal input is not routed by the persisted record, only by a live entry.
#[tokio::test]
async fn driving_a_terminal_requires_a_live_runtime_not_merely_a_record() {
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let created = service(Arc::clone(&store), Arc::new(FakeAdapter::default()))
        .create_session(&caller, spec())
        .await
        .expect("created");

    let restarted = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    assert!(matches!(
        restarted
            .send_input(&caller, &created.id, b"ls\n".to_vec())
            .await,
        Err(CoreError::SessionNotFound)
    ));
}
