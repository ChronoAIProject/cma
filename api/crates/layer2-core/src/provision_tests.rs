//! Tests for bringing a runtime up behind the request that asked for it.
//!
//! The point of provisioning asynchronously is that no connection has to be
//! held open for it. The risk it introduces is that nobody is listening when it
//! goes wrong, so most of what is asserted here is that failures end up
//! recorded rather than merely logged.

use std::sync::Arc;

use crate::session::{SessionStatus, StopReason};
use crate::store::SessionStore;
use crate::test_support::{await_settled, service, spec, user, FakeAdapter, FakeStore};

#[tokio::test]
async fn creating_a_session_returns_before_a_runtime_exists() {
    // The whole reason for the change: the caller is answered immediately, so
    // no timeout anywhere is load-bearing.
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let created = service(Arc::clone(&store), Arc::new(FakeAdapter::default()))
        .create_session(&caller, spec())
        .await
        .expect("created");

    assert_eq!(created.status, SessionStatus::Provisioning);
}

#[tokio::test]
async fn the_session_is_durable_before_any_runtime_is_asked_for() {
    // Record-first is what makes a leak impossible: a runtime cannot come to
    // exist before something references it.
    let store = Arc::new(FakeStore::default());
    let created = service(Arc::clone(&store), Arc::new(FakeAdapter::default()))
        .create_session(&user("alice"), spec())
        .await
        .expect("created");

    let record = store
        .get(&created.id)
        .await
        .expect("store readable")
        .expect("the record exists the moment creation returns");
    assert_eq!(record.session.status, SessionStatus::Provisioning);
}

#[tokio::test]
async fn a_session_becomes_ready_once_its_runtime_is_up() {
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    let created = svc.create_session(&caller, spec()).await.expect("created");

    let settled = await_settled(&svc, &caller, &created.id).await;
    assert_eq!(settled.status, SessionStatus::Ready);
}

#[tokio::test]
async fn a_provider_that_cannot_provision_fails_the_session_rather_than_leaving_it_pending() {
    // Nobody is waiting to be told. A session left in provisioning forever is
    // worse than the synchronous version, where the caller at least saw the
    // error, so the failure has to be written down.
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let adapter = Arc::new(FakeAdapter {
        refuse_provision: true,
        ..FakeAdapter::default()
    });
    let svc = service(Arc::clone(&store), adapter);
    let created = svc.create_session(&caller, spec()).await.expect("created");

    let settled = await_settled(&svc, &caller, &created.id).await;
    assert_eq!(settled.status, SessionStatus::Terminated);
    assert_eq!(settled.stop_reason, Some(StopReason::Error));
}

#[tokio::test]
async fn a_runtime_that_cannot_be_attached_to_is_torn_down() {
    // The leak this design exists to prevent: provisioning succeeded, so a
    // runtime is running, and the session it belongs to has failed.
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let adapter = Arc::new(FakeAdapter {
        refuse_attach: true,
        ..FakeAdapter::default()
    });
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = svc.create_session(&caller, spec()).await.expect("created");

    let settled = await_settled(&svc, &caller, &created.id).await;
    assert_eq!(settled.status, SessionStatus::Terminated);
    let torn_down = adapter.terminated.lock().await.clone();
    assert!(
        !torn_down.is_empty() && torn_down.iter().all(|r| r.contains(&created.id.0)),
        "every runtime the attempts created must be torn down, got {torn_down:?}"
    );
}

#[tokio::test]
async fn a_failed_session_stops_referencing_a_runtime_that_is_gone() {
    // Otherwise reconciliation would keep trying to reclaim something already
    // torn down.
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let svc = service(
        Arc::clone(&store),
        Arc::new(FakeAdapter {
            refuse_attach: true,
            ..FakeAdapter::default()
        }),
    );
    let created = svc.create_session(&caller, spec()).await.expect("created");
    await_settled(&svc, &caller, &created.id).await;

    let record = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present");
    assert!(record.runtime_id.is_none());
}

#[tokio::test]
async fn the_failure_is_appended_to_the_durable_log() {
    // A client following the event stream has to learn that it failed; polling
    // the record is not the only way this is observed.
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let svc = service(
        Arc::clone(&store),
        Arc::new(FakeAdapter {
            refuse_provision: true,
            ..FakeAdapter::default()
        }),
    );
    let created = svc.create_session(&caller, spec()).await.expect("created");
    await_settled(&svc, &caller, &created.id).await;

    let events = svc
        .events_since(&caller, &created.id, None)
        .await
        .expect("history readable");
    let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
    let mut sorted = seqs.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(seqs, sorted, "sequences must stay monotonic and unique");
    assert!(
        events.len() >= 3,
        "creation, provisioning and the failure must all be recorded, got {}",
        events.len()
    );
}

#[tokio::test]
async fn an_unknown_kernel_is_refused_immediately_rather_than_asynchronously() {
    // The caller's own mistake, and they are still on the line to be told.
    // Accepting it would mean writing a record for a session that can never
    // start.
    let store = Arc::new(FakeStore::default());
    let mut bad = spec();
    bad.kernel = crate::ids::KernelId::new("no-such-kernel");
    let result = service(Arc::clone(&store), Arc::new(FakeAdapter::default()))
        .create_session(&user("alice"), bad)
        .await;

    assert!(result.is_err());
    assert!(
        store.list_all().await.expect("readable").is_empty(),
        "a refused request must leave no record behind"
    );
}

#[tokio::test]
async fn a_transient_provider_failure_is_retried_rather_than_reported() {
    // Someone asking for a session wants a session. A provider under load is
    // nobody's decision and usually passes, so it must not be the answer they
    // get — this is the failure that ended sessions with no explanation.
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let adapter = Arc::new(FakeAdapter {
        fail_first: std::sync::atomic::AtomicUsize::new(2),
        ..FakeAdapter::default()
    });
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = svc.create_session(&caller, spec()).await.expect("created");

    let settled = await_settled(&svc, &caller, &created.id).await;
    assert_eq!(
        settled.status,
        SessionStatus::Ready,
        "a session must survive a provider that failed twice"
    );
}

#[tokio::test]
async fn a_failed_attempt_does_not_leave_a_runtime_behind() {
    // Each retry starts clean. Without discarding what the previous attempt
    // created, a session that eventually succeeds costs several sandboxes.
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let adapter = Arc::new(FakeAdapter {
        // Provisioning succeeds and attaching fails, so a runtime exists each
        // time the attempt is abandoned.
        refuse_attach: true,
        ..FakeAdapter::default()
    });
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = svc.create_session(&caller, spec()).await.expect("created");
    await_settled(&svc, &caller, &created.id).await;

    let torn_down = adapter.terminated.lock().await.len();
    assert!(
        torn_down >= 2,
        "every runtime a failed attempt created must be torn down, got {torn_down}"
    );
}

#[tokio::test]
async fn a_session_that_cannot_start_says_why() {
    // The reason travels with the event, so the person watching learns what
    // the log already knew instead of seeing a session end for no stated
    // reason.
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let svc = service(
        Arc::clone(&store),
        Arc::new(FakeAdapter {
            refuse_provision: true,
            ..FakeAdapter::default()
        }),
    );
    let created = svc.create_session(&caller, spec()).await.expect("created");
    await_settled(&svc, &caller, &created.id).await;

    let events = svc
        .events_since(&caller, &created.id, None)
        .await
        .expect("history readable");
    let explained = events.iter().any(|e| {
        matches!(&e.kind, crate::event::EventKind::StatusChanged { detail: Some(d), .. }
            if d.contains("no capacity") || d.contains("attempts"))
    });
    assert!(explained, "the failure carried no explanation: {events:?}");
}

#[tokio::test]
async fn every_event_of_a_successful_start_is_recorded() {
    // Progress and status were allocated by two counters from the same
    // starting point, so they claimed the same sequence and the second insert
    // was rejected. The session came up and nothing recorded that it had,
    // leaving a live session that looked stuck forever.
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    let created = svc.create_session(&caller, spec()).await.expect("created");
    await_settled(&svc, &caller, &created.id).await;

    let events = svc
        .events_since(&caller, &created.id, None)
        .await
        .expect("history readable");

    let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
    let mut unique = seqs.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        seqs.len(),
        unique.len(),
        "two events claimed the same sequence: {seqs:?}"
    );

    // The one that matters: a client following the stream has to be told the
    // session is usable.
    let announced_ready = events.iter().any(|e| {
        matches!(
            &e.kind,
            crate::event::EventKind::StatusChanged {
                status: SessionStatus::Ready,
                ..
            }
        )
    });
    assert!(announced_ready, "readiness was never recorded: {events:?}");
}
