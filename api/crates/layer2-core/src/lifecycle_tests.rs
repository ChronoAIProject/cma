//! Tests for ending and removing sessions.
//!
//! Terminating and deleting are different acts on purpose, and the difference
//! is only observable across a restart and against the runtime, so these are
//! written against the durable record rather than against live memory.

use std::sync::Arc;
use std::time::Duration;

use crate::authz::{Authorization, Caller, Capability};
use crate::coordination::{SessionBoundary, SessionLeaseRequest, SessionOperation};
use crate::error::CoreError;
use crate::ids::{SessionId, SubjectId};
use crate::session::SessionStatus;
use crate::store::SessionStore;
use crate::terminal::TerminalGeometry;
use crate::test_support::{await_settled, service, spec, user, FakeAdapter, FakeStore};

/// Somebody who runs the service: they may stop anyone's work and nothing else.
///
/// Built without the everyday grants on purpose, so these tests prove the
/// operator capability is doing the work rather than riding on ownership.
fn operator() -> Caller {
    Caller::new(
        SubjectId("operator".into()),
        Authorization::from_capabilities([
            Capability::ControlAnySession,
            Capability::ReadAllSessions,
        ]),
    )
}

#[tokio::test]
async fn a_terminated_session_remains_listed_until_it_is_deleted() {
    // Terminate and delete are different acts. A finished conversation is
    // still worth reading, so ending one must not remove it.
    let store = Arc::new(FakeStore::default());
    let caller = user("alice");
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    let created = svc.create_session(&caller, spec()).await.expect("created");
    await_settled(&svc, &caller, &created.id).await;

    svc.terminate_session(&caller, &created.id)
        .await
        .expect("terminates");
    assert_eq!(
        svc.list_sessions(&caller, &crate::SessionListQuery::default())
            .await
            .expect("session list is readable")
            .items
            .len(),
        1,
        "terminating must not remove the session"
    );
    svc.delete_session(&caller, &created.id)
        .await
        .expect("deletes");
    assert!(
        svc.list_sessions(&caller, &crate::SessionListQuery::default())
            .await
            .expect("session list is readable")
            .items
            .is_empty(),
        "deleting must remove it"
    );
}

#[tokio::test]
async fn coordinated_termination_retries_exact_debt_before_delete() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let caller = user("alice");
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = svc.create_session(&caller, spec()).await.expect("created");
    await_settled(&svc, &caller, &created.id).await;
    let durable = store.get(&created.id).await.unwrap().unwrap();
    let runtime = durable.runtime_id.unwrap();
    let generation = store
        .runtime_authority(&created.id)
        .await
        .unwrap()
        .unwrap()
        .generation;

    // Make the first conditional cleanup refuse this stale provider identity.
    // Termination must still commit terminal state and retain exact debt.
    adapter.generations.lock().await.insert(
        runtime.clone(),
        std::num::NonZeroU64::new(generation + 1).unwrap(),
    );
    svc.terminate_session(&caller, &created.id)
        .await
        .expect("terminal state commits even when cleanup must be retried");
    assert_eq!(
        store
            .get(&created.id)
            .await
            .unwrap()
            .unwrap()
            .session
            .status,
        SessionStatus::Terminated
    );
    assert_eq!(
        store
            .runtime_cleanup_debts(&created.id)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(adapter.running.lock().await.contains_key(&runtime));

    // Deleting an already-terminal row re-enters coordinated termination, so
    // restoring the exact provider identity lets it settle debt before the row
    // (the only cleanup authority) is removed.
    adapter.generations.lock().await.insert(
        runtime.clone(),
        std::num::NonZeroU64::new(generation).unwrap(),
    );
    svc.delete_session(&caller, &created.id)
        .await
        .expect("delete retries and settles terminal cleanup debt");
    assert!(store.get(&created.id).await.unwrap().is_none());
    assert!(!adapter.running.lock().await.contains_key(&runtime));
}

#[tokio::test]
async fn coordinated_termination_preserves_each_failed_identity_among_multiple_debts() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let caller = user("alice");
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = svc.create_session(&caller, spec()).await.expect("created");
    await_settled(&svc, &caller, &created.id).await;

    let original = store.get(&created.id).await.unwrap().unwrap();
    let original_runtime = original.runtime_id.clone().unwrap();
    let authority = store.runtime_authority(&created.id).await.unwrap().unwrap();
    let request = SessionLeaseRequest::new(
        created.id.clone(),
        "terminal-multiple-debt-test",
        SessionBoundary {
            status: original.session.status,
            authority,
        },
        SessionOperation::Handover,
        Duration::from_secs(30),
    );
    let mut lease = store.acquire_session_lease(&request).await.unwrap();
    let reservation = store
        .reserve_runtime_generation(&lease, "terminal-current-candidate")
        .await
        .unwrap();
    let current_runtime = "runtime-terminal-current".to_string();
    adapter
        .running
        .lock()
        .await
        .insert(current_runtime.clone(), Some(created.id.0.clone()));
    adapter
        .generations
        .lock()
        .await
        .insert(current_runtime.clone(), reservation.generation);
    let candidate = store
        .register_runtime_candidate(&lease, &reservation, &current_runtime)
        .await
        .unwrap();
    let original_cleanup = store
        .switch_runtime_authority(&mut lease, &candidate)
        .await
        .unwrap()
        .cleanup
        .expect("switching retains the former authority as exact debt");
    store.release_session_lease(&lease).await.unwrap();

    adapter.generations.lock().await.insert(
        original_runtime.clone(),
        std::num::NonZeroU64::new(original_cleanup.generation + 100).unwrap(),
    );
    svc.terminate_session(&caller, &created.id)
        .await
        .expect("one failed cleanup does not block the other debt");

    let remaining = store.runtime_cleanup_debts(&created.id).await.unwrap();
    assert_eq!(remaining, vec![original_cleanup.clone()]);
    assert!(adapter.running.lock().await.contains_key(&original_runtime));
    assert!(!adapter.running.lock().await.contains_key(&current_runtime));

    adapter.generations.lock().await.insert(
        original_runtime.clone(),
        std::num::NonZeroU64::new(original_cleanup.generation).unwrap(),
    );
    svc.delete_session(&caller, &created.id)
        .await
        .expect("delete retries the one remaining exact identity");
    assert!(store.get(&created.id).await.unwrap().is_none());
    assert!(!adapter.running.lock().await.contains_key(&original_runtime));
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
    // Deleting must stop a *running* runtime, so wait until there is one.
    await_settled(&svc, &caller, &created.id).await;

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
    await_settled(&svc, &caller, &created.id).await;
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
    await_settled(&svc, &caller, &created.id).await;
    svc.terminate_session(&caller, &created.id)
        .await
        .expect("terminates");

    assert!(matches!(
        svc.attach_terminal(
            &caller,
            &created.id,
            TerminalGeometry::default(),
            Default::default()
        )
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
    await_settled(&svc, &caller, &created.id).await;

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
    let adapter = Arc::new(FakeAdapter::default());
    let caller = user("alice");
    let first = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = first
        .create_session(&caller, spec())
        .await
        .expect("session is created");
    await_settled(&first, &caller, &created.id).await;

    let recorded = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .runtime_id
        .expect("the session has a runtime");

    let restarted = service(Arc::clone(&store), Arc::clone(&adapter));
    restarted
        .terminate_session(&caller, &created.id)
        .await
        .expect("a persisted session can be terminated");

    assert_eq!(
        adapter.terminated.lock().await.clone(),
        vec![recorded],
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
    let adapter = Arc::new(FakeAdapter::default());
    let caller = user("alice");
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = svc
        .create_session(&caller, spec())
        .await
        .expect("session is created");
    await_settled(&svc, &caller, &created.id).await;
    let before = svc
        .events_since(&caller, &created.id, None)
        .await
        .expect("history readable")
        .len();

    service(Arc::clone(&store), Arc::clone(&adapter))
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
#[tokio::test]
async fn an_operator_can_terminate_a_session_another_subject_owns() {
    // The reason the capability exists: work that is costing money or running
    // away has to be stoppable by whoever runs the service, and waiting for
    // its owner to come back is not an answer.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let owner = user("alice");
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = svc.create_session(&owner, spec()).await.expect("created");
    await_settled(&svc, &owner, &created.id).await;

    svc.terminate_session(&operator(), &created.id)
        .await
        .expect("an operator may end a session it does not own");

    let after = svc
        .get_session(&owner, &created.id)
        .await
        .expect("still readable by its owner");
    assert_eq!(after.status, SessionStatus::Terminated);
    assert_eq!(
        adapter.terminated.lock().await.len(),
        1,
        "the runtime must actually be torn down"
    );
}
#[tokio::test]
async fn an_operator_can_stop_a_session_another_subject_owns() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let owner = user("alice");
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = svc.create_session(&owner, spec()).await.expect("created");
    await_settled(&svc, &owner, &created.id).await;

    svc.stop_session(&operator(), &created.id)
        .await
        .expect("an operator may pause a session it does not own");

    let after = svc
        .get_session(&owner, &created.id)
        .await
        .expect("still readable by its owner");
    assert_eq!(
        after.status,
        SessionStatus::Idle,
        "stopping stays reversible whoever asked for it"
    );
}
#[tokio::test]
async fn an_operator_may_not_delete_another_subjects_session() {
    // Stopping is reversible and removing is not. Running the service needs
    // the first and never the second.
    let store = Arc::new(FakeStore::default());
    let owner = user("alice");
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    let created = svc.create_session(&owner, spec()).await.expect("created");

    assert!(matches!(
        svc.delete_session(&operator(), &created.id).await,
        Err(CoreError::Forbidden)
    ));
    assert!(
        svc.get_session(&owner, &created.id).await.is_ok(),
        "the session must survive a refused deletion"
    );
}
#[tokio::test]
async fn one_subject_still_cannot_stop_another_subjects_session() {
    // The operator capability must not have widened what an ordinary caller
    // can reach. This is the check that would silently disappear if the two
    // grants were ever collapsed into one.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = svc
        .create_session(&user("alice"), spec())
        .await
        .expect("created");
    await_settled(&svc, &user("alice"), &created.id).await;

    assert!(matches!(
        svc.stop_session(&user("mallory"), &created.id).await,
        Err(CoreError::Forbidden)
    ));
    assert!(
        adapter.terminated.lock().await.is_empty(),
        "a forbidden request must not reach the runtime"
    );
}
#[tokio::test]
async fn a_caller_entitled_to_nothing_is_refused_without_the_session_being_read() {
    // A caller holding neither control grant is turned away before the record
    // is touched, so a real identifier and an invented one produce the same
    // refusal and neither confirms the other.
    let store = Arc::new(FakeStore::default());
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    let created = svc
        .create_session(&user("alice"), spec())
        .await
        .expect("created");

    let onlooker = Caller::new(
        SubjectId("onlooker".into()),
        Authorization::from_capabilities([Capability::UseOwnSessions]),
    );
    assert_eq!(
        svc.terminate_session(&onlooker, &created.id).await,
        Err(CoreError::Forbidden)
    );
    assert_eq!(
        svc.terminate_session(&onlooker, &SessionId("sesn_invented".into()))
            .await,
        Err(CoreError::Forbidden),
        "an identifier that does not exist must be indistinguishable from one that does"
    );
}
#[tokio::test]
async fn the_operator_entry_point_refuses_a_caller_acting_on_their_own_session() {
    // Its own gate rather than the everyday one. The surface projecting this
    // is restricted as an operator surface in front of the service as well as
    // inside it, and a caller who succeeds here while that restriction would
    // have refused them means the two disagree about who may call it. Ending
    // their own session is still available to them at their own address.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;

    assert_eq!(
        svc.terminate_any_session(&alice, &created.id).await,
        Err(CoreError::Forbidden)
    );
    assert_eq!(
        svc.stop_any_session(&alice, &created.id).await,
        Err(CoreError::Forbidden)
    );
    assert!(
        adapter.terminated.lock().await.is_empty(),
        "a refused request must not reach the runtime"
    );

    svc.terminate_session(&alice, &created.id)
        .await
        .expect("their own session is still theirs to end");
}
#[tokio::test]
async fn the_operator_entry_point_reports_whose_work_it_ended() {
    // Without this the record cannot tell an operator ending somebody else's
    // session from that person ending their own: same action, same target,
    // and the owner is knowable only here, before the session is gone.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));

    let stopped = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &stopped.id).await;
    assert_eq!(
        svc.stop_any_session(&operator(), &stopped.id)
            .await
            .expect("an operator may pause work it does not own"),
        SubjectId("alice".into())
    );

    let ended = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &ended.id).await;
    assert_eq!(
        svc.terminate_any_session(&operator(), &ended.id)
            .await
            .expect("an operator may end work it does not own"),
        SubjectId("alice".into())
    );
}
