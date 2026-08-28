//! Tests for ending and removing sessions.
//!
//! Terminating and deleting are different acts on purpose, and the difference
//! is only observable across a restart and against the runtime, so these are
//! written against the durable record rather than against live memory.

use std::sync::Arc;
use std::time::Duration;

use layer3_adapter::{KernelAdapter, RuntimeHandle};

use crate::authz::{Authorization, Caller, Capability};
use crate::commit_fault_support::CommitFaultStore;
use crate::coordination::{SessionBoundary, SessionLeaseRequest, SessionOperation};
use crate::error::CoreError;
use crate::event::Event;
use crate::ids::{SessionId, SubjectId};
use crate::session::{RecoveryState, SessionStatus, StopReason};
use crate::store::{CredentialSource, SessionRecord, SessionStore, StoreError};
use crate::terminal::TerminalGeometry;
use crate::test_support::{
    await_settled, service, service_over, service_with_credential_source, spec, user, FakeAdapter,
    FakeStore,
};

#[derive(Default)]
struct RecordingCredentialSource {
    releases: std::sync::atomic::AtomicUsize,
}

impl RecordingCredentialSource {
    fn release_calls(&self) -> usize {
        self.releases.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl CredentialSource for RecordingCredentialSource {
    async fn credential_for(
        &self,
        _session: &SessionId,
        _subject: &SubjectId,
        _services: &[crate::environment::ServiceId],
        _caller_credential: Option<&crate::CallerCredential>,
    ) -> Result<Option<String>, StoreError> {
        Ok(Some("recorded-lifecycle-credential".into()))
    }

    async fn release(&self, _session: &SessionId) -> Result<(), StoreError> {
        self.releases
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

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

fn never_matches(_record: &SessionRecord, _events: &[Event]) -> bool {
    false
}

fn is_compatibility_stop(record: &SessionRecord, _events: &[Event]) -> bool {
    record.session.status == SessionStatus::Idle
        && record.session.stop_reason == Some(StopReason::UserTerminated)
        && record.session.recovery_state == RecoveryState::Idle
        && record.rotation_claim.is_none()
        && record.runtime_id.is_some()
}

fn is_compatibility_termination(record: &SessionRecord, _events: &[Event]) -> bool {
    record.session.status == SessionStatus::Terminated
        && record.session.stop_reason == Some(StopReason::UserTerminated)
        && record.session.recovery_state == RecoveryState::Retired
        && record.rotation_claim.is_none()
        && record.runtime_id.is_some()
}

fn is_compatibility_termination_clear(record: &SessionRecord, events: &[Event]) -> bool {
    record.session.status == SessionStatus::Terminated
        && record.session.stop_reason == Some(StopReason::UserTerminated)
        && record.session.recovery_state == RecoveryState::Retired
        && record.rotation_claim.is_none()
        && record.runtime_id.is_none()
        && events.is_empty()
}

async fn publication_finalization_waits_for_compatibility_lifecycle(terminate: bool) {
    let inner = Arc::new(FakeStore::default());
    let selected: fn(&SessionRecord, &[Event]) -> bool = if terminate {
        is_compatibility_termination
    } else {
        is_compatibility_stop
    };
    let store = Arc::new(CommitFaultStore::new(Arc::clone(&inner), selected));
    let adapter = Arc::new(FakeAdapter::default());
    let caller = user(if terminate {
        "publication-termination-owner"
    } else {
        "publication-stop-owner"
    });
    let service = service_over(store.clone(), Arc::clone(&adapter));
    let created = service
        .create_session(&caller, spec())
        .await
        .expect("created");
    await_settled(&service, &caller, &created.id).await;
    service.flush(&created.id).await;

    let claim = if terminate {
        "rotn_compatibility_termination"
    } else {
        "rotn_compatibility_stop"
    };
    let mut claimed = inner.get(&created.id).await.unwrap().unwrap();
    claimed.rotation_claim = Some(claim.into());
    claimed.revision = inner
        .commit_session_idempotent("test-publication-lifecycle-claim", &claimed, &[])
        .await
        .expect("the publication claim is durable");

    store.pause_after_next();
    let lifecycle = tokio::spawn({
        let service = Arc::clone(&service);
        let caller = caller.clone();
        let id = created.id.clone();
        async move {
            if terminate {
                service.terminate_session(&caller, &id).await
            } else {
                service.stop_session(&caller, &id).await
            }
        }
    });
    store.wait_until_held().await;
    assert!(service.entries.read().await.contains_key(&created.id));
    assert_eq!(
        inner
            .get(&created.id)
            .await
            .unwrap()
            .unwrap()
            .session
            .status,
        if terminate {
            SessionStatus::Terminated
        } else {
            SessionStatus::Idle
        },
        "the lifecycle write commits before its retained continuation runs"
    );
    assert!(
        inner
            .get(&created.id)
            .await
            .unwrap()
            .unwrap()
            .runtime_id
            .is_some(),
        "phase one retains the exact runtime until provider teardown"
    );

    let mut finalization = tokio::spawn({
        let service = Arc::clone(&service);
        let id = created.id.clone();
        async move { service.finalize_rotation_publication(&id, claim).await }
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut finalization)
            .await
            .is_err(),
        "publication finalization waits for the lifecycle continuation"
    );

    store.release();
    tokio::time::timeout(Duration::from_secs(2), lifecycle)
        .await
        .expect("the lifecycle operation resumes")
        .expect("the lifecycle task completes")
        .expect("the durable lifecycle success remains successful");
    assert!(
        !tokio::time::timeout(Duration::from_secs(2), finalization)
            .await
            .expect("publication finalization resumes")
            .expect("the finalization task completes")
            .expect("the durable store remains available"),
        "the completed lifecycle operation already removed the publication"
    );
    let entries = service.entries.read().await;
    if terminate {
        let entry = entries
            .get(&created.id)
            .expect("termination keeps the readable local session entry");
        assert_eq!(entry.session.status, SessionStatus::Terminated);
        assert!(entry.runtime.is_none());
        assert!(entry.input.is_none());
        assert!(entry.conversation.is_none());
    } else {
        assert!(!entries.contains_key(&created.id));
    }
}

#[tokio::test]
async fn publication_finalization_cannot_turn_a_compatibility_stop_into_failure() {
    publication_finalization_waits_for_compatibility_lifecycle(false).await;
}

#[tokio::test]
async fn publication_finalization_cannot_turn_a_compatibility_termination_into_failure() {
    publication_finalization_waits_for_compatibility_lifecycle(true).await;
}

#[tokio::test]
async fn compatibility_stop_tears_down_stale_live_and_exact_durable_rotation_runtimes() {
    let inner = Arc::new(FakeStore::default());
    let store = Arc::new(CommitFaultStore::new(Arc::clone(&inner), never_matches));
    let adapter = Arc::new(FakeAdapter::default());
    let caller = user("rotation-stop-owner");
    let svc = service_over(store, Arc::clone(&adapter));

    let stopped = svc.create_session(&caller, spec()).await.expect("created");
    await_settled(&svc, &caller, &stopped.id).await;
    svc.flush(&stopped.id).await;
    let local_runtime = svc
        .entries
        .read()
        .await
        .get(&stopped.id)
        .and_then(|entry| entry.runtime.as_ref())
        .map(|runtime| runtime.0.clone())
        .expect("H1 is installed locally");
    let durable_runtime = format!("{local_runtime}-replacement");
    adapter
        .running
        .lock()
        .await
        .insert(durable_runtime.clone(), Some(stopped.id.0.clone()));
    let mut stopping = inner.get(&stopped.id).await.unwrap().unwrap();
    assert_eq!(stopping.runtime_id.as_deref(), Some(local_runtime.as_str()));
    stopping.runtime_id = Some(durable_runtime.clone());
    stopping.rotation_claim = Some("rotn_stop_exact_claim".into());
    stopping.revision = inner
        .commit_session_idempotent("test-stop-rotation-claim", &stopping, &[])
        .await
        .expect("stop claim is durable");

    adapter.fail_terminations(1);
    assert!(matches!(
        svc.stop_session(&caller, &stopped.id)
            .await
            .expect_err("the failed stale H1 teardown is propagated"),
        CoreError::RuntimeUnavailable(_)
    ));
    let failed = inner.get(&stopped.id).await.unwrap().unwrap();
    assert_eq!(failed.session.status, SessionStatus::Idle);
    assert_eq!(failed.rotation_claim, None);
    assert_eq!(failed.runtime_id.as_deref(), Some(durable_runtime.as_str()));
    {
        let entries = svc.entries.read().await;
        let entry = entries
            .get(&stopped.id)
            .expect("the failed stop retains H1 locally for retry");
        assert_eq!(
            entry.runtime.as_ref().map(|runtime| runtime.0.as_str()),
            Some(local_runtime.as_str())
        );
        assert!(entry.input.is_none());
        assert!(entry.conversation.is_none());
    }
    {
        let running = adapter.running.lock().await;
        assert!(running.contains_key(&local_runtime));
        assert!(running.contains_key(&durable_runtime));
    }

    svc.stop_session(&caller, &stopped.id)
        .await
        .expect("a retry tears down both retained identities");
    let stopped = inner.get(&stopped.id).await.unwrap().unwrap();
    assert_eq!(stopped.session.status, SessionStatus::Idle);
    assert_eq!(stopped.rotation_claim, None);
    assert_eq!(stopped.runtime_id, None);
    assert_eq!(
        adapter.terminated.lock().await.as_slice(),
        &[local_runtime.clone(), durable_runtime.clone()],
        "the stale local H1 and exact durable H2 must both be torn down"
    );
    let running = adapter.running.lock().await;
    assert!(!running.contains_key(&local_runtime));
    assert!(!running.contains_key(&durable_runtime));
}

async fn compatibility_teardown_failure_remains_retryable(terminate: bool) {
    let inner = Arc::new(FakeStore::default());
    let store = Arc::new(CommitFaultStore::new(Arc::clone(&inner), never_matches));
    let adapter = Arc::new(FakeAdapter::default());
    let caller = user(if terminate {
        "retryable-termination-owner"
    } else {
        "retryable-stop-owner"
    });
    let svc = service_over(store, Arc::clone(&adapter));
    let created = svc.create_session(&caller, spec()).await.expect("created");
    await_settled(&svc, &caller, &created.id).await;
    svc.flush(&created.id).await;
    let runtime = inner
        .get(&created.id)
        .await
        .unwrap()
        .unwrap()
        .runtime_id
        .expect("the session has a compatibility runtime");

    adapter.fail_terminations(1);
    let failure = if terminate {
        svc.terminate_session(&caller, &created.id).await
    } else {
        svc.stop_session(&caller, &created.id).await
    }
    .expect_err("the provider teardown failure is returned to the caller");
    assert!(matches!(failure, CoreError::RuntimeUnavailable(_)));

    let retained = inner.get(&created.id).await.unwrap().unwrap();
    assert_eq!(
        retained.session.status,
        if terminate {
            SessionStatus::Terminated
        } else {
            SessionStatus::Idle
        }
    );
    assert_eq!(
        retained.session.recovery_state,
        if terminate {
            RecoveryState::Retired
        } else {
            RecoveryState::Idle
        }
    );
    assert_eq!(
        retained.session.stop_reason,
        Some(StopReason::UserTerminated)
    );
    assert_eq!(retained.rotation_claim, None);
    assert_eq!(retained.runtime_id.as_deref(), Some(runtime.as_str()));
    assert!(adapter.running.lock().await.contains_key(&runtime));
    {
        let entries = svc.entries.read().await;
        let entry = entries
            .get(&created.id)
            .expect("a failed teardown retains its noninteractive local identity");
        assert_eq!(entry.session.status, retained.session.status);
        assert_eq!(
            entry.runtime.as_ref().map(|handle| handle.0.as_str()),
            Some(runtime.as_str())
        );
        assert!(entry.input.is_none());
        assert!(entry.conversation.is_none());
    }
    let events_after_failure = inner
        .events
        .lock()
        .await
        .iter()
        .filter(|event| event.session_id == created.id)
        .count();

    if terminate {
        svc.terminate_session(&caller, &created.id)
            .await
            .expect("termination retries its retained runtime");
    } else {
        svc.stop_session(&caller, &created.id)
            .await
            .expect("stop retries its retained runtime");
    }

    let cleared = inner.get(&created.id).await.unwrap().unwrap();
    assert_eq!(cleared.runtime_id, None);
    assert!(!adapter.running.lock().await.contains_key(&runtime));
    assert_eq!(
        inner
            .events
            .lock()
            .await
            .iter()
            .filter(|event| event.session_id == created.id)
            .count(),
        events_after_failure,
        "an exact phase-one retry must not append another lifecycle event"
    );
    let entries = svc.entries.read().await;
    if terminate {
        let entry = entries
            .get(&created.id)
            .expect("termination keeps a readable local entry");
        assert!(entry.runtime.is_none());
        assert!(entry.input.is_none());
        assert!(entry.conversation.is_none());
    } else {
        assert!(!entries.contains_key(&created.id));
    }
}

#[tokio::test]
async fn compatibility_stop_propagates_teardown_failure_and_retries_exactly() {
    compatibility_teardown_failure_remains_retryable(false).await;
}

#[tokio::test]
async fn compatibility_termination_propagates_teardown_failure_and_retries_exactly() {
    compatibility_teardown_failure_remains_retryable(true).await;
}

#[tokio::test]
async fn compatibility_delete_keeps_its_terminal_row_when_teardown_fails() {
    let inner = Arc::new(FakeStore::default());
    let store = Arc::new(CommitFaultStore::new(Arc::clone(&inner), never_matches));
    let adapter = Arc::new(FakeAdapter::default());
    let caller = user("retryable-delete-owner");
    let svc = service_over(store, Arc::clone(&adapter));
    let created = svc.create_session(&caller, spec()).await.expect("created");
    await_settled(&svc, &caller, &created.id).await;
    svc.flush(&created.id).await;
    let runtime = inner
        .get(&created.id)
        .await
        .unwrap()
        .unwrap()
        .runtime_id
        .expect("the session has a compatibility runtime");

    adapter.fail_terminations(1);
    assert!(matches!(
        svc.delete_session(&caller, &created.id)
            .await
            .expect_err("delete must propagate termination cleanup failure"),
        CoreError::RuntimeUnavailable(_)
    ));
    let retained = inner
        .get(&created.id)
        .await
        .unwrap()
        .expect("delete keeps the only durable cleanup authority");
    assert_eq!(retained.session.status, SessionStatus::Terminated);
    assert_eq!(retained.session.recovery_state, RecoveryState::Retired);
    assert_eq!(retained.runtime_id.as_deref(), Some(runtime.as_str()));
    assert!(adapter.running.lock().await.contains_key(&runtime));

    svc.delete_session(&caller, &created.id)
        .await
        .expect("a later delete retries cleanup before removing the row");
    assert!(inner.get(&created.id).await.unwrap().is_none());
    assert!(!adapter.running.lock().await.contains_key(&runtime));
}

#[tokio::test]
async fn compatibility_termination_retries_when_the_final_runtime_clear_fails() {
    let inner = Arc::new(FakeStore::default());
    let store = Arc::new(CommitFaultStore::new(
        Arc::clone(&inner),
        is_compatibility_termination_clear,
    ));
    let adapter = Arc::new(FakeAdapter::default());
    let caller = user("retryable-final-clear-owner");
    let svc = service_over(store.clone(), Arc::clone(&adapter));
    let created = svc.create_session(&caller, spec()).await.expect("created");
    await_settled(&svc, &caller, &created.id).await;
    svc.flush(&created.id).await;
    let runtime = inner
        .get(&created.id)
        .await
        .unwrap()
        .unwrap()
        .runtime_id
        .expect("the session has a compatibility runtime");

    store.fail_next(4);
    assert!(svc.terminate_session(&caller, &created.id).await.is_err());
    let retained = inner.get(&created.id).await.unwrap().unwrap();
    assert_eq!(retained.session.status, SessionStatus::Terminated);
    assert_eq!(retained.session.recovery_state, RecoveryState::Retired);
    assert_eq!(retained.runtime_id.as_deref(), Some(runtime.as_str()));
    assert!(
        !adapter.running.lock().await.contains_key(&runtime),
        "provider success precedes the failed conditional clear"
    );
    {
        let entries = svc.entries.read().await;
        let entry = entries
            .get(&created.id)
            .expect("the failed final clear retains local retry identity");
        assert_eq!(
            entry.runtime.as_ref().map(|handle| handle.0.as_str()),
            Some(runtime.as_str())
        );
        assert!(entry.input.is_none());
        assert!(entry.conversation.is_none());
    }
    let events_after_failure = inner
        .events
        .lock()
        .await
        .iter()
        .filter(|event| event.session_id == created.id)
        .count();

    svc.terminate_session(&caller, &created.id)
        .await
        .expect("idempotent provider teardown and final clear succeed on retry");
    assert_eq!(
        inner.get(&created.id).await.unwrap().unwrap().runtime_id,
        None
    );
    assert_eq!(
        adapter.terminated.lock().await.as_slice(),
        &[runtime.clone(), runtime.clone()],
        "a failed final clear repeats idempotent teardown from durable identity"
    );
    assert_eq!(
        inner
            .events
            .lock()
            .await
            .iter()
            .filter(|event| event.session_id == created.id)
            .count(),
        events_after_failure,
        "the retry resumes after phase one without another status event"
    );
}

#[tokio::test]
async fn compatibility_termination_cleanup_survives_caller_cancellation() {
    let inner = Arc::new(FakeStore::default());
    let store = Arc::new(CommitFaultStore::new(
        Arc::clone(&inner),
        is_compatibility_termination,
    ));
    let adapter = Arc::new(FakeAdapter::default());
    let caller = user("cancelled-termination-owner");
    let svc = service_over(store.clone(), Arc::clone(&adapter));
    let created = svc.create_session(&caller, spec()).await.expect("created");
    await_settled(&svc, &caller, &created.id).await;
    svc.flush(&created.id).await;
    let runtime = inner
        .get(&created.id)
        .await
        .unwrap()
        .unwrap()
        .runtime_id
        .expect("the session has a compatibility runtime");

    store.pause_after_next();
    let lifecycle = tokio::spawn({
        let svc = Arc::clone(&svc);
        let caller = caller.clone();
        let id = created.id.clone();
        async move { svc.terminate_session(&caller, &id).await }
    });
    store.wait_until_held().await;
    let phase_one = inner.get(&created.id).await.unwrap().unwrap();
    assert_eq!(phase_one.session.status, SessionStatus::Terminated);
    assert_eq!(phase_one.runtime_id.as_deref(), Some(runtime.as_str()));
    lifecycle.abort();
    assert!(lifecycle.await.unwrap_err().is_cancelled());
    store.release();

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let durable_cleared = inner
                .get(&created.id)
                .await
                .unwrap()
                .is_some_and(|record| record.runtime_id.is_none());
            let local_cleared = svc
                .entries
                .read()
                .await
                .get(&created.id)
                .is_some_and(|entry| entry.runtime.is_none());
            if durable_cleared && local_cleared {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the retained continuation tears down and clears after cancellation");
    assert!(!adapter.running.lock().await.contains_key(&runtime));
    let entries = svc.entries.read().await;
    let entry = entries
        .get(&created.id)
        .expect("termination retains the readable entry after cancellation");
    assert_eq!(entry.session.status, SessionStatus::Terminated);
    assert!(entry.runtime.is_none());
    assert!(entry.input.is_none());
    assert!(entry.conversation.is_none());
}

#[tokio::test]
async fn compatibility_termination_tears_down_stale_live_and_exact_durable_rotation_runtimes() {
    let inner = Arc::new(FakeStore::default());
    let store = Arc::new(CommitFaultStore::new(Arc::clone(&inner), never_matches));
    let adapter = Arc::new(FakeAdapter::default());
    let caller = user("rotation-termination-owner");
    let svc = service_over(store, Arc::clone(&adapter));

    let terminated = svc.create_session(&caller, spec()).await.expect("created");
    await_settled(&svc, &caller, &terminated.id).await;
    svc.flush(&terminated.id).await;
    let local_runtime = svc
        .entries
        .read()
        .await
        .get(&terminated.id)
        .and_then(|entry| entry.runtime.as_ref())
        .map(|runtime| runtime.0.clone())
        .expect("H1 is installed locally");
    let durable_runtime = format!("{local_runtime}-replacement");
    adapter
        .running
        .lock()
        .await
        .insert(durable_runtime.clone(), Some(terminated.id.0.clone()));
    let mut terminating = inner.get(&terminated.id).await.unwrap().unwrap();
    assert_eq!(
        terminating.runtime_id.as_deref(),
        Some(local_runtime.as_str())
    );
    terminating.runtime_id = Some(durable_runtime.clone());
    terminating.rotation_claim = Some("rotn_terminate_exact_claim".into());
    terminating.revision = inner
        .commit_session_idempotent("test-terminate-rotation-claim", &terminating, &[])
        .await
        .expect("termination claim is durable");

    svc.terminate_session(&caller, &terminated.id)
        .await
        .expect("termination settles its observed claim");
    let terminated = inner.get(&terminated.id).await.unwrap().unwrap();
    assert_eq!(terminated.session.status, SessionStatus::Terminated);
    assert_eq!(terminated.rotation_claim, None);
    assert_eq!(terminated.runtime_id, None);
    assert_eq!(
        adapter.terminated.lock().await.as_slice(),
        &[local_runtime.clone(), durable_runtime.clone()],
        "the stale local H1 and exact durable H2 must both be torn down"
    );
    let running = adapter.running.lock().await;
    assert!(!running.contains_key(&local_runtime));
    assert!(!running.contains_key(&durable_runtime));
}

#[tokio::test]
async fn compatibility_termination_repairs_a_terminal_row_that_retains_a_runtime_before_delete() {
    let inner = Arc::new(FakeStore::default());
    let store = Arc::new(CommitFaultStore::new(Arc::clone(&inner), never_matches));
    let adapter = Arc::new(FakeAdapter::default());
    let caller = user("legacy-terminal-runtime-owner");
    let svc = service_over(store, Arc::clone(&adapter));

    let created = svc.create_session(&caller, spec()).await.expect("created");
    await_settled(&svc, &caller, &created.id).await;
    svc.flush(&created.id).await;

    let mut terminal = inner.get(&created.id).await.unwrap().unwrap();
    let runtime = terminal
        .runtime_id
        .clone()
        .expect("the settled session retains its provider runtime");
    terminal.session.status = SessionStatus::Terminated;
    terminal.session.stop_reason = Some(StopReason::UserTerminated);
    terminal.session.recovery_state = RecoveryState::Retired;
    terminal.rotation_claim = None;
    terminal.revision = inner
        .commit_session_idempotent("test-terminal-retained-runtime", &terminal, &[])
        .await
        .expect("the legacy terminal row is durable");

    svc.terminate_session(&caller, &created.id)
        .await
        .expect("termination repairs the retained runtime");

    let repaired = inner.get(&created.id).await.unwrap().unwrap();
    assert_eq!(repaired.session.status, SessionStatus::Terminated);
    assert_eq!(
        repaired.session.stop_reason,
        Some(StopReason::UserTerminated)
    );
    assert_eq!(repaired.session.recovery_state, RecoveryState::Retired);
    assert_eq!(repaired.rotation_claim, None);
    assert_eq!(repaired.runtime_id, None);
    assert!(!adapter.running.lock().await.contains_key(&runtime));
    assert_eq!(
        adapter.terminated.lock().await.as_slice(),
        std::slice::from_ref(&runtime),
        "the exact retained runtime is torn down once"
    );
    {
        let entries = svc.entries.read().await;
        let entry = entries
            .get(&created.id)
            .expect("termination keeps the readable local session entry");
        assert_eq!(entry.session.status, SessionStatus::Terminated);
        assert!(entry.runtime.is_none());
        assert!(entry.input.is_none());
        assert!(entry.conversation.is_none());
    }

    svc.delete_session(&caller, &created.id)
        .await
        .expect("delete accepts the repaired terminal row");
    assert!(inner.get(&created.id).await.unwrap().is_none());
    assert!(!adapter.running.lock().await.contains_key(&runtime));
    assert_eq!(
        adapter.terminated.lock().await.as_slice(),
        &[runtime],
        "idempotent termination during delete does not repeat teardown"
    );
}

#[tokio::test]
async fn compatibility_termination_retry_clears_a_terminal_residual_handover_descriptor() {
    let store = Arc::new(FakeStore::default());
    store.disable_coordination();
    let adapter = Arc::new(FakeAdapter::default());
    let caller = user("terminal-residual-handover-owner");
    let original = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = original
        .create_session(&caller, spec())
        .await
        .expect("created");
    await_settled(&original, &caller, &created.id).await;
    original.flush(&created.id).await;
    original
        .terminate_session(&caller, &created.id)
        .await
        .expect("the session first reaches its normal terminal boundary");
    let mut residual = None;
    for _ in 0..400 {
        let record = store.get(&created.id).await.unwrap().unwrap();
        if record.skill_packs == crate::store::SkillPackState::Cleaned {
            residual = Some(record);
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let mut residual = residual.expect("termination's detached custody cleanup settled");
    assert_eq!(residual.session.status, SessionStatus::Terminated);
    assert_eq!(residual.runtime_id, None);
    assert_eq!(residual.rotation_claim, None);
    residual.pending_handover = Some(crate::store::PendingHandover {
        claim: "rotn_terminal_residual".into(),
        snapshot_ref: format!("sessions/{}/handover.snapshot", created.id.0),
        snapshot_at: "2026-08-27T02:03:04Z".into(),
        trigger_recovery: None,
        rotations_from: residual.session.rotations,
        rotations_to: residual.session.rotations + 1,
        target: crate::store::PendingHandoverTarget::RuntimeVersion {
            from_runtime_version: residual.session.runtime_version.clone(),
            runtime_version: "runtime:unconsumed".into(),
        },
    });
    residual.revision = store
        .commit_session_idempotent("test-terminal-residual-handover", &residual, &[])
        .await
        .expect("the interrupted terminal cleanup is durable");
    drop(original);

    let restarted = service(Arc::clone(&store), Arc::clone(&adapter));
    restarted
        .terminate_session(&caller, &created.id)
        .await
        .expect("terminal retry repairs the residual descriptor");

    let repaired = store.get(&created.id).await.unwrap().unwrap();
    assert_eq!(repaired.session.status, SessionStatus::Terminated);
    assert_eq!(
        repaired.session.stop_reason,
        Some(StopReason::UserTerminated)
    );
    assert_eq!(repaired.session.recovery_state, RecoveryState::Retired);
    assert_eq!(repaired.runtime_id, None);
    assert_eq!(repaired.rotation_claim, None);
    assert_eq!(repaired.pending_handover, None);
    assert!(repaired.revision > residual.revision);
    assert_eq!(adapter.terminated.lock().await.len(), 1);
}

#[tokio::test]
async fn compatibility_termination_retry_repairs_custody_after_a_clear_before_release_crash() {
    let store = Arc::new(FakeStore::default());
    store.disable_coordination();
    let adapter = Arc::new(FakeAdapter::default());
    let credentials = Arc::new(RecordingCredentialSource::default());
    let caller = user("terminal-custody-repair-owner");
    let original = service_with_credential_source(
        Arc::clone(&store),
        Arc::clone(&adapter),
        credentials.clone(),
    );
    let created = original
        .create_session(&caller, spec())
        .await
        .expect("created");
    await_settled(&original, &caller, &created.id).await;
    original.flush(&created.id).await;

    let mut terminal = store.get(&created.id).await.unwrap().unwrap();
    let runtime = terminal
        .runtime_id
        .clone()
        .expect("the session has a compatibility runtime");
    adapter
        .terminate(&RuntimeHandle(runtime))
        .await
        .expect("provider teardown completed before the crash");
    terminal.session.status = SessionStatus::Terminated;
    terminal.session.stop_reason = Some(StopReason::UserTerminated);
    terminal.session.recovery_state = RecoveryState::Retired;
    terminal.rotation_claim = None;
    terminal.runtime_id = None;
    terminal.revision = store
        .commit_session_idempotent("test-clear-before-custody-release-crash", &terminal, &[])
        .await
        .expect("the final runtime clear was durable before the crash");
    assert_eq!(credentials.release_calls(), 0);

    let restarted = service_with_credential_source(
        Arc::clone(&store),
        Arc::clone(&adapter),
        credentials.clone(),
    );
    restarted
        .terminate_session(&caller, &created.id)
        .await
        .expect("an exact terminal retry repairs credential custody");

    assert_eq!(credentials.release_calls(), 1);
    let unchanged = store.get(&created.id).await.unwrap().unwrap();
    assert_eq!(unchanged.session.status, SessionStatus::Terminated);
    assert_eq!(unchanged.rotation_claim, None);
    assert_eq!(unchanged.runtime_id, None);
    assert!(restarted.entries.read().await.get(&created.id).is_none());
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
async fn coordinated_stop_confirms_a_retirement_whose_two_responses_are_lost() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let caller = user("ambiguous-stop-owner");
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = svc.create_session(&caller, spec()).await.expect("created");
    await_settled(&svc, &caller, &created.id).await;
    let runtime = store
        .get(&created.id)
        .await
        .unwrap()
        .unwrap()
        .runtime_id
        .expect("the session has a runtime");
    adapter.terminal_inputs.lock().await.clear();

    store.lose_runtime_retirement_responses(2);
    svc.stop_session(&caller, &created.id)
        .await
        .expect("the held lease confirms its committed retirement");

    let durable = store.get(&created.id).await.unwrap().unwrap();
    assert_eq!(durable.session.status, SessionStatus::Idle);
    assert_eq!(durable.runtime_id, None);
    assert_eq!(
        store
            .runtime_authority(&created.id)
            .await
            .unwrap()
            .unwrap()
            .runtime_id,
        None
    );
    assert!(!svc.entries.read().await.contains_key(&created.id));
    assert!(
        adapter.running.lock().await.contains_key(&runtime),
        "an unknown response leaves exact cleanup debt rather than deleting by handle"
    );
    assert!(svc
        .send_input(&caller, &created.id, b"must stay fenced".to_vec())
        .await
        .is_err());
    assert!(adapter.terminal_inputs.lock().await.is_empty());
}

#[tokio::test]
async fn coordinated_stop_confirms_an_idle_lease_advance_with_a_lost_response() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let caller = user("ambiguous-stop-advance-owner");
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = svc.create_session(&caller, spec()).await.expect("created");
    await_settled(&svc, &caller, &created.id).await;

    store.lose_runtime_advance_responses(1);
    svc.stop_session(&caller, &created.id)
        .await
        .expect("renewal at the committed target confirms the hidden advance");

    let durable = store.get(&created.id).await.unwrap().unwrap();
    assert_eq!(durable.session.status, SessionStatus::Idle);
    assert_eq!(durable.runtime_id, None);
    assert!(!svc.entries.read().await.contains_key(&created.id));
    assert!(svc
        .send_input(&caller, &created.id, b"remain stopped".to_vec())
        .await
        .is_err());
}

#[tokio::test]
async fn a_stop_on_one_replica_fences_the_old_channel_on_another() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let caller = user("cross-replica-stop-owner");
    let stopping = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = stopping
        .create_session(&caller, spec())
        .await
        .expect("created");
    await_settled(&stopping, &caller, &created.id).await;
    let stale = service(Arc::clone(&store), Arc::clone(&adapter));
    assert_eq!(stale.reconcile().await.readopted_sessions, 1);
    adapter.terminal_inputs.lock().await.clear();

    let runtime = store
        .get(&created.id)
        .await
        .unwrap()
        .unwrap()
        .runtime_id
        .expect("the session has a runtime");
    let generation = store
        .runtime_authority(&created.id)
        .await
        .unwrap()
        .unwrap()
        .generation;
    adapter.generations.lock().await.insert(
        runtime.clone(),
        std::num::NonZeroU64::new(generation + 1).unwrap(),
    );

    stopping
        .stop_session(&caller, &created.id)
        .await
        .expect("the first replica stops the session");
    assert!(
        stale.entries.read().await.contains_key(&created.id),
        "the regression requires the second replica's stale local entry"
    );
    assert!(adapter.running.lock().await.contains_key(&runtime));

    assert!(stale
        .send_input(&caller, &created.id, b"never deliver".to_vec())
        .await
        .is_err());
    assert!(
        adapter.terminal_inputs.lock().await.is_empty(),
        "no-runtime durable authority must fence the other replica's old sender"
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
