use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use super::checkpoint_service_tests::{build_service, fixture, host, use_package};
use super::*;
use crate::store::{ChatHostStore, PageRequest, SessionStore};
use crate::test_support::{spec, user};
use crate::{
    ChatPauseOutcome, EventKind, RecoveryState, SessionSpec, SessionStatus, SessionSurface,
    StopReason, StoredEvent, StoredEventKind,
};

const OLD_ACTIVITY: &str = "2026-09-01T01:45:00Z";
const AT_BOUNDARY: &str = "2026-09-01T02:00:00Z";
const AFTER_BOUNDARY: &str = "2026-09-01T02:00:01Z";
const NOTICE: &str = "Your current chat session has been idle for 15min and has been auto-paused.";

fn chat_spec() -> SessionSpec {
    SessionSpec {
        surface: Some(SessionSurface::Chat),
        ..spec()
    }
}

async fn age_all_events(fixture: &super::checkpoint_service_tests::CheckpointFixture) {
    for event in fixture.sessions.events.lock().await.iter_mut() {
        event.appended_at = OLD_ACTIVITY.into();
    }
}

async fn append_old_event(
    fixture: &super::checkpoint_service_tests::CheckpointFixture,
    thread_index: usize,
    kind: StoredEventKind,
) {
    let id = fixture.threads[thread_index].id.clone();
    let mut events = fixture.sessions.events.lock().await;
    let seq = events
        .iter()
        .filter(|event| event.session_id == id)
        .map(|event| event.seq)
        .max()
        .unwrap_or(0)
        + 1;
    events.push(StoredEvent {
        session_id: id,
        seq,
        appended_at: OLD_ACTIVITY.into(),
        kind,
    });
}

async fn assert_thread_restored(
    fixture: &super::checkpoint_service_tests::CheckpointFixture,
    thread_index: usize,
) {
    let record = fixture
        .sessions
        .get(&fixture.threads[thread_index].id)
        .await
        .unwrap()
        .expect("the chat thread remains durable");
    assert_eq!(record.session.status, SessionStatus::Ready);
    assert_eq!(record.session.recovery_state, RecoveryState::Normal);
    assert_eq!(record.session.stop_reason, None);
    assert_eq!(
        record
            .session
            .chat
            .as_ref()
            .and_then(|chat| chat.paused_notice.as_deref()),
        None
    );
    assert_eq!(
        record
            .chat_origin
            .as_ref()
            .and_then(|origin| origin.paused_notice.as_deref()),
        None
    );
}

async fn idle_timeout_events(
    fixture: &super::checkpoint_service_tests::CheckpointFixture,
    thread_index: usize,
) -> usize {
    let id = &fixture.threads[thread_index].id;
    fixture
        .sessions
        .events
        .lock()
        .await
        .iter()
        .filter(|event| {
            event.session_id == *id
                && matches!(
                    &event.kind,
                    StoredEventKind::Known(EventKind::StatusChanged {
                        status: SessionStatus::Idle,
                        stop_reason: Some(StopReason::IdleTimeout),
                        ..
                    })
                )
        })
        .count()
}

async fn candidates(
    fixture: &super::checkpoint_service_tests::CheckpointFixture,
    now: &str,
) -> Vec<ChatHostRecord> {
    fixture
        .service
        .chat_pause_candidates(now, PageRequest::new(200, None))
        .await
        .expect("candidate scan succeeds")
        .items
}

#[tokio::test]
async fn every_thread_must_be_strictly_older_than_the_pause_boundary() {
    let fixture = fixture("pause-boundary", 2).await;
    age_all_events(&fixture).await;

    assert!(candidates(&fixture, AT_BOUNDARY).await.is_empty());
    assert_eq!(candidates(&fixture, AFTER_BOUNDARY).await.len(), 1);

    let active = fixture.threads[1].id.clone();
    fixture
        .sessions
        .sessions
        .lock()
        .await
        .get_mut(active.as_str())
        .expect("thread exists")
        .session
        .status = SessionStatus::Running;
    assert!(candidates(&fixture, AFTER_BOUNDARY).await.is_empty());
}

#[tokio::test]
async fn pending_approval_blocks_pause_even_when_its_event_is_old() {
    let fixture = fixture("pause-approval", 1).await;
    age_all_events(&fixture).await;
    let id = fixture.threads[0].id.clone();
    let tail = fixture
        .sessions
        .events
        .lock()
        .await
        .iter()
        .filter(|event| event.session_id == id)
        .map(|event| event.seq)
        .max()
        .unwrap();
    fixture.sessions.events.lock().await.push(StoredEvent {
        session_id: id,
        seq: tail + 1,
        appended_at: OLD_ACTIVITY.into(),
        kind: StoredEventKind::Known(EventKind::ApprovalRequested {
            request_id: "approval-pending".into(),
            action: Some("write a file".into()),
            detail: None,
            locator: None,
        }),
    });

    assert!(candidates(&fixture, AFTER_BOUNDARY).await.is_empty());
}

#[tokio::test]
async fn a_durably_resolved_approval_permits_pause() {
    let fixture = fixture("pause-resolved-approval", 1).await;
    age_all_events(&fixture).await;
    append_old_event(
        &fixture,
        0,
        StoredEventKind::Known(EventKind::ApprovalRequested {
            request_id: "approval-resolved".into(),
            action: Some("write a file".into()),
            detail: None,
            locator: None,
        }),
    )
    .await;
    append_old_event(
        &fixture,
        0,
        StoredEventKind::Known(EventKind::ApprovalResolved {
            request_id: "approval-resolved".into(),
            allowed: false,
        }),
    )
    .await;

    assert_eq!(candidates(&fixture, AFTER_BOUNDARY).await.len(), 1);
}

#[tokio::test]
async fn provisioning_and_rotating_threads_each_block_pause() {
    let fixture = fixture("pause-transitional-status", 1).await;
    age_all_events(&fixture).await;
    let id = fixture.threads[0].id.clone();

    for status in [SessionStatus::Provisioning, SessionStatus::Rotating] {
        fixture
            .sessions
            .sessions
            .lock()
            .await
            .get_mut(id.as_str())
            .unwrap()
            .session
            .status = status;
        assert!(
            candidates(&fixture, AFTER_BOUNDARY).await.is_empty(),
            "{status:?} must keep the shared runtime alive"
        );
    }
}

#[tokio::test]
async fn removed_threads_do_not_participate_in_host_quietness() {
    let fixture = fixture("pause-removed-thread", 2).await;
    age_all_events(&fixture).await;
    let removed = fixture.threads[1].id.clone();
    fixture
        .sessions
        .sessions
        .lock()
        .await
        .get_mut(removed.as_str())
        .unwrap()
        .session
        .status = SessionStatus::Running;
    {
        let mut hosts = fixture.hosts.records.lock().await;
        let host = hosts.values_mut().next().expect("the shared host exists");
        host.threads
            .iter_mut()
            .find(|thread| thread.session_id == removed)
            .expect("the removed thread has a host slot")
            .removed = true;
    }

    assert_eq!(candidates(&fixture, AFTER_BOUNDARY).await.len(), 1);
}

#[tokio::test]
async fn unknown_and_malformed_event_tails_fail_closed() {
    let unknown = fixture("pause-unknown-event", 1).await;
    age_all_events(&unknown).await;
    append_old_event(
        &unknown,
        0,
        StoredEventKind::Unknown {
            tag: "future_chat_activity".into(),
            payload: serde_json::Value::Null,
        },
    )
    .await;
    assert!(matches!(
        unknown
            .service
            .chat_pause_candidates(AFTER_BOUNDARY, PageRequest::default())
            .await,
        Err(crate::CoreError::RuntimeUnavailable(_))
    ));
    assert_eq!(unknown.adapter.checkpoint_calls.load(Ordering::SeqCst), 0);

    let malformed = fixture("pause-malformed-event-time", 1).await;
    age_all_events(&malformed).await;
    let id = malformed.threads[0].id.clone();
    malformed
        .sessions
        .events
        .lock()
        .await
        .iter_mut()
        .filter(|event| event.session_id == id)
        .max_by_key(|event| event.seq)
        .unwrap()
        .appended_at = "not-a-timestamp".into();
    assert!(candidates(&malformed, AFTER_BOUNDARY).await.is_empty());
    assert_eq!(malformed.adapter.checkpoint_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn filtered_empty_page_preserves_the_active_host_continuation() {
    let fixture = fixture("pause-page-a", 1).await;
    for owner in ["pause-page-b", "pause-page-c"] {
        fixture
            .service
            .create_session(&user(owner), chat_spec())
            .await
            .expect("additional chat host is created");
    }
    age_all_events(&fixture).await;
    let first = fixture
        .hosts
        .list_candidates(&ChatHostListQuery::new(
            PageRequest::new(1, None),
            vec![ChatHostStatus::Active],
        ))
        .await
        .unwrap();
    let first_session = first.items[0].threads[0].session_id.clone();
    fixture
        .sessions
        .sessions
        .lock()
        .await
        .get_mut(first_session.as_str())
        .unwrap()
        .session
        .status = SessionStatus::Running;

    let first_candidates = fixture
        .service
        .chat_pause_candidates(AFTER_BOUNDARY, PageRequest::new(1, None))
        .await
        .unwrap();
    assert!(first_candidates.items.is_empty());
    let continuation = first_candidates
        .next
        .expect("the filtered page keeps its underlying continuation");
    let second_candidates = fixture
        .service
        .chat_pause_candidates(AFTER_BOUNDARY, PageRequest::new(1, Some(continuation)))
        .await
        .unwrap();
    assert_eq!(second_candidates.items.len(), 1);
}

#[tokio::test]
async fn successful_pause_parks_every_thread_then_fenced_deletes_the_runtime() {
    let fixture = fixture("pause-complete", 2).await;
    age_all_events(&fixture).await;
    let before = host(&fixture).await;
    let runtime = before.runtime_id.clone().unwrap();
    use_package(&fixture, 1, "idle-pause").await;

    let outcome = fixture
        .service
        .pause_chat_host(&before.id, AFTER_BOUNDARY)
        .await
        .expect("pause settles");
    let ChatPauseOutcome::Complete(checkpoint) = outcome else {
        panic!("quiet host did not pause: {outcome:?}");
    };
    assert_eq!(checkpoint.checkpoint_version, 1);

    let paused = host(&fixture).await;
    assert_eq!(paused.status, ChatHostStatus::Paused);
    assert_eq!(paused.runtime_id, None);
    assert_eq!(paused.paused_at.as_deref(), Some(AFTER_BOUNDARY));
    assert_eq!(paused.checkpoint, Some(checkpoint));
    assert_eq!(paused.last_transition_claim, None);
    assert_eq!(paused.last_operator_note, None);
    assert_eq!(
        fixture.adapter.terminated.lock().await.as_slice(),
        &[runtime]
    );
    assert!(fixture.adapter.running.lock().await.is_empty());
    assert!(fixture.adapter.workspace_resumes.lock().await.is_empty());

    for thread in &fixture.threads {
        let record = fixture
            .sessions
            .get(&thread.id)
            .await
            .unwrap()
            .expect("paused thread remains durable");
        assert_eq!(record.session.status, SessionStatus::Idle);
        assert_eq!(record.session.recovery_state, RecoveryState::Idle);
        assert_eq!(record.session.stop_reason, Some(StopReason::IdleTimeout));
        assert_eq!(
            record
                .session
                .chat
                .as_ref()
                .and_then(|chat| chat.paused_notice.as_deref()),
            Some(NOTICE)
        );
        assert_eq!(
            record
                .chat_origin
                .as_ref()
                .and_then(|origin| origin.paused_notice.as_deref()),
            Some(NOTICE)
        );
        let event = fixture
            .sessions
            .events
            .lock()
            .await
            .iter()
            .filter(|event| event.session_id == thread.id)
            .max_by_key(|event| event.seq)
            .cloned()
            .unwrap();
        assert!(matches!(
            event.kind,
            StoredEventKind::Known(EventKind::StatusChanged {
                status: SessionStatus::Idle,
                stop_reason: Some(StopReason::IdleTimeout),
                detail: Some(ref detail),
                capability: None,
            }) if detail == NOTICE
        ));
    }
}

#[tokio::test]
async fn capture_failure_reopens_writers_and_leaves_the_host_active() {
    let fixture = fixture("pause-capture-refusal", 1).await;
    age_all_events(&fixture).await;
    let before = host(&fixture).await;
    let runtime = before.runtime_id.clone().unwrap();
    use_package(&fixture, 1, "capture-refusal").await;
    fixture.adapter.conversation_snapshots.lock().await.clear();

    assert_eq!(
        fixture
            .service
            .pause_chat_host(&before.id, AFTER_BOUNDARY)
            .await
            .unwrap(),
        ChatPauseOutcome::Refused
    );
    let after = host(&fixture).await;
    assert_eq!(after.status, ChatHostStatus::Active);
    assert_eq!(after.runtime_id.as_deref(), Some(runtime.as_str()));
    assert_eq!(after.last_transition_claim, None);
    assert_eq!(
        after
            .last_operator_note
            .as_ref()
            .map(|note| note.code.as_str()),
        Some("pause_refused")
    );
    assert!(fixture.adapter.terminated.lock().await.is_empty());
    assert_eq!(fixture.adapter.workspace_resumes.lock().await.len(), 1);
    let thread = fixture
        .sessions
        .get(&fixture.threads[0].id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(thread.session.status, SessionStatus::Ready);
    assert_eq!(thread.session.stop_reason, None);

    assert!(candidates(&fixture, "2026-09-01T02:01:01Z")
        .await
        .is_empty());
    assert_eq!(candidates(&fixture, "2026-09-01T02:05:01Z").await.len(), 1);
}

#[tokio::test]
async fn failed_writer_resumption_leaves_an_explicitly_recoverable_pausing_host() {
    let fixture = fixture("pause-writer-resume-refusal", 1).await;
    age_all_events(&fixture).await;
    let before = host(&fixture).await;
    let runtime = before.runtime_id.clone().unwrap();
    use_package(&fixture, 1, "resume-refusal").await;
    fixture.adapter.conversation_snapshots.lock().await.clear();
    fixture
        .adapter
        .fail_workspace_resumes
        .store(1, Ordering::SeqCst);

    assert_eq!(
        fixture
            .service
            .pause_chat_host(&before.id, AFTER_BOUNDARY)
            .await
            .unwrap(),
        ChatPauseOutcome::Refused
    );
    let after = host(&fixture).await;
    assert_eq!(after.status, ChatHostStatus::Pausing);
    assert_eq!(after.runtime_id.as_deref(), Some(runtime.as_str()));
    assert_eq!(after.last_transition_claim, None);
    assert_eq!(after.transition_started_at, None);
    assert_eq!(
        after
            .last_operator_note
            .as_ref()
            .map(|note| (note.code.as_str(), note.detail.as_str())),
        Some((
            "pause_refused",
            "Workspace writer resumption could not be verified."
        ))
    );
    assert!(fixture.adapter.terminated.lock().await.is_empty());
    assert!(fixture.adapter.running.lock().await.contains_key(&runtime));
    assert_thread_restored(&fixture, 0).await;
}

#[tokio::test]
async fn host_publication_failure_reopens_writers_and_restores_every_thread() {
    let fixture = fixture("pause-host-publication-failure", 2).await;
    age_all_events(&fixture).await;
    let before = host(&fixture).await;
    let runtime = before.runtime_id.clone().unwrap();
    use_package(&fixture, 1, "host-publication-failure").await;
    fixture
        .adapter
        .hold_checkpoint
        .store(true, Ordering::SeqCst);
    let service = Arc::clone(&fixture.service);
    let host_id = before.id.clone();
    let attempt =
        tokio::spawn(async move { service.pause_chat_host(&host_id, AFTER_BOUNDARY).await });
    tokio::time::timeout(
        Duration::from_secs(2),
        fixture.adapter.checkpoint_started.acquire(),
    )
    .await
    .expect("pause reaches the held workspace capture")
    .expect("the adapter remains alive")
    .forget();
    fixture.hosts.fail_updates.store(1, Ordering::SeqCst);
    fixture.adapter.continue_checkpoint.add_permits(1);

    assert_eq!(attempt.await.unwrap().unwrap(), ChatPauseOutcome::Refused);
    let after = host(&fixture).await;
    assert_eq!(after.status, ChatHostStatus::Active);
    assert_eq!(after.runtime_id.as_deref(), Some(runtime.as_str()));
    assert_eq!(after.last_transition_claim, None);
    assert_eq!(
        after
            .last_operator_note
            .as_ref()
            .map(|note| note.code.as_str()),
        Some("pause_refused")
    );
    assert_eq!(fixture.adapter.workspace_resumes.lock().await.len(), 1);
    assert!(fixture.adapter.terminated.lock().await.is_empty());
    assert!(fixture.adapter.running.lock().await.contains_key(&runtime));
    assert_thread_restored(&fixture, 0).await;
    assert_thread_restored(&fixture, 1).await;
}

#[tokio::test]
async fn fenced_deletion_failure_restores_writers_threads_and_active_host() {
    let fixture = fixture("pause-delete-failure", 2).await;
    age_all_events(&fixture).await;
    let before = host(&fixture).await;
    let runtime = before.runtime_id.clone().unwrap();
    use_package(&fixture, 1, "delete-failure").await;
    fixture.adapter.fail_terminations(1);

    assert_eq!(
        fixture
            .service
            .pause_chat_host(&before.id, AFTER_BOUNDARY)
            .await
            .unwrap(),
        ChatPauseOutcome::Refused
    );
    let after = host(&fixture).await;
    assert_eq!(after.status, ChatHostStatus::Active);
    assert_eq!(after.runtime_id.as_deref(), Some(runtime.as_str()));
    assert_eq!(
        after
            .checkpoint
            .as_ref()
            .map(|value| value.checkpoint_version),
        Some(1)
    );
    assert_eq!(after.paused_at, None);
    assert_eq!(after.last_transition_claim, None);
    assert_eq!(
        after
            .last_operator_note
            .as_ref()
            .map(|note| (note.code.as_str(), note.detail.as_str())),
        Some((
            "pause_refused",
            "The verified chat runtime could not be fenced-deleted."
        ))
    );
    assert_eq!(fixture.adapter.workspace_resumes.lock().await.len(), 1);
    assert!(fixture.adapter.terminated.lock().await.is_empty());
    assert!(fixture.adapter.running.lock().await.contains_key(&runtime));
    assert_thread_restored(&fixture, 0).await;
    assert_thread_restored(&fixture, 1).await;
}

#[tokio::test]
async fn a_partial_thread_park_is_rolled_back_before_the_runtime_is_touched() {
    let fixture = fixture("pause-partial-thread-park", 2).await;
    age_all_events(&fixture).await;
    let before = host(&fixture).await;
    use_package(&fixture, 1, "partial-thread-park").await;
    let mut ordered = fixture
        .threads
        .iter()
        .map(|thread| thread.id.clone())
        .collect::<Vec<_>>();
    ordered.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    let terminal = ordered.pop().unwrap();
    fixture
        .sessions
        .sessions
        .lock()
        .await
        .get_mut(terminal.as_str())
        .unwrap()
        .session
        .status = SessionStatus::Terminated;

    assert_eq!(
        fixture
            .service
            .pause_chat_host(&before.id, AFTER_BOUNDARY)
            .await
            .unwrap(),
        ChatPauseOutcome::Refused
    );
    let after = host(&fixture).await;
    assert_eq!(after.status, ChatHostStatus::Active);
    assert_eq!(after.runtime_id, before.runtime_id);
    assert_eq!(fixture.adapter.workspace_resumes.lock().await.len(), 1);
    assert!(fixture.adapter.terminated.lock().await.is_empty());
    for (index, thread) in fixture.threads.iter().enumerate() {
        if thread.id == terminal {
            let record = fixture.sessions.get(&thread.id).await.unwrap().unwrap();
            assert_eq!(record.session.status, SessionStatus::Terminated);
        } else {
            assert_thread_restored(&fixture, index).await;
        }
    }
}

#[tokio::test]
async fn retrying_an_already_paused_host_does_not_duplicate_idle_timeout_events() {
    let fixture = fixture("pause-idempotent-retry", 1).await;
    age_all_events(&fixture).await;
    let id = host(&fixture).await.id;
    use_package(&fixture, 1, "idempotent-retry").await;
    assert!(matches!(
        fixture
            .service
            .pause_chat_host(&id, AFTER_BOUNDARY)
            .await
            .unwrap(),
        ChatPauseOutcome::Complete(_)
    ));
    assert_eq!(idle_timeout_events(&fixture, 0).await, 1);

    assert_eq!(
        fixture
            .service
            .pause_chat_host(&id, "2026-09-01T03:00:00Z")
            .await
            .unwrap(),
        ChatPauseOutcome::NotEligible
    );
    assert_eq!(idle_timeout_events(&fixture, 0).await, 1);
    assert_eq!(fixture.adapter.terminated.lock().await.len(), 1);
}

#[tokio::test]
async fn turn_start_between_scan_and_fence_close_aborts_without_capture() {
    let fixture = fixture("pause-turn-race", 1).await;
    age_all_events(&fixture).await;
    let before = host(&fixture).await;
    assert_eq!(candidates(&fixture, AFTER_BOUNDARY).await.len(), 1);
    let session_id = fixture.threads[0].id.clone();
    let input = fixture.service.input_gates.gate(&session_id);
    let held_turn = input.read_owned().await;
    let service = Arc::clone(&fixture.service);
    let host_id = before.id.clone();
    let pause =
        tokio::spawn(async move { service.pause_chat_host(&host_id, AFTER_BOUNDARY).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if host(&fixture).await.status == ChatHostStatus::Pausing {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("pause claims the host before waiting for the turn");
    fixture
        .sessions
        .sessions
        .lock()
        .await
        .get_mut(session_id.as_str())
        .unwrap()
        .session
        .status = SessionStatus::Running;
    drop(held_turn);

    assert_eq!(pause.await.unwrap().unwrap(), ChatPauseOutcome::NotEligible);
    assert_eq!(host(&fixture).await.status, ChatHostStatus::Active);
    assert_eq!(fixture.adapter.checkpoint_calls.load(Ordering::SeqCst), 0);
    assert!(fixture.adapter.terminated.lock().await.is_empty());
}

#[tokio::test]
async fn two_replicas_cannot_capture_or_delete_the_same_host_twice() {
    let fixture = fixture("pause-two-replicas", 1).await;
    age_all_events(&fixture).await;
    let before = host(&fixture).await;
    use_package(&fixture, 1, "two-replicas").await;
    fixture
        .adapter
        .hold_checkpoint
        .store(true, Ordering::SeqCst);
    let first_service = Arc::clone(&fixture.service);
    let first_id = before.id.clone();
    let first = tokio::spawn(async move {
        first_service
            .pause_chat_host(&first_id, AFTER_BOUNDARY)
            .await
    });
    tokio::time::timeout(
        Duration::from_secs(2),
        fixture.adapter.checkpoint_started.acquire(),
    )
    .await
    .expect("first worker reaches capture")
    .expect("adapter remains alive")
    .forget();

    let second = build_service(
        Arc::clone(&fixture.adapter),
        Arc::clone(&fixture.hosts),
        Arc::clone(&fixture.sessions),
        Arc::clone(&fixture.checkpoints),
    );
    assert!(matches!(
        second
            .pause_chat_host(&before.id, AFTER_BOUNDARY)
            .await
            .unwrap(),
        ChatPauseOutcome::NotEligible | ChatPauseOutcome::Busy
    ));
    fixture.adapter.continue_checkpoint.add_permits(1);
    assert!(matches!(
        first.await.unwrap().unwrap(),
        ChatPauseOutcome::Complete(_)
    ));
    assert_eq!(fixture.adapter.checkpoint_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.adapter.terminated.lock().await.len(), 1);
}

#[tokio::test]
async fn invalid_time_fails_before_candidate_or_provider_work() {
    let fixture = fixture("pause-invalid-time", 1).await;
    assert!(matches!(
        fixture
            .service
            .chat_pause_candidates("invalid", PageRequest::default())
            .await,
        Err(crate::CoreError::InvalidRequest(_))
    ));
    assert!(matches!(
        fixture
            .service
            .pause_chat_host(&host(&fixture).await.id, "invalid")
            .await,
        Err(crate::CoreError::InvalidRequest(_))
    ));
    assert_eq!(fixture.adapter.checkpoint_calls.load(Ordering::SeqCst), 0);
}
