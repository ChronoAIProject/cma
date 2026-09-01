use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use super::checkpoint_service_tests::{build_service, fixture, host, use_package};
use super::*;
use crate::store::{ChatCheckpointStore, ChatHostStore, PageRequest, SessionStore};
use crate::test_support::user;
use crate::{
    Authorization, Caller, Capability, EventKind, RecoveryState, SessionSpec, SessionStatus,
    SessionSurface, StopReason, StoredEventKind, SubjectId,
};

fn reader() -> Caller {
    Caller::new(
        SubjectId("chat-host-reader".into()),
        Authorization::from_capabilities([Capability::ReadAllSessions]),
    )
}

fn operator() -> Caller {
    Caller::new(
        SubjectId("chat-host-operator".into()),
        Authorization::from_capabilities([Capability::ControlAnySession]),
    )
}

fn chat_spec() -> SessionSpec {
    SessionSpec {
        surface: Some(SessionSurface::Chat),
        ..crate::test_support::spec()
    }
}

#[tokio::test]
async fn host_inventory_requires_cross_owner_read_and_preserves_store_pagination() {
    let fixture = fixture("host-list-a", 1).await;
    fixture
        .service
        .create_session(&user("host-list-b"), chat_spec())
        .await
        .expect("a second owner creates another host");

    assert_eq!(
        fixture
            .service
            .list_chat_hosts(
                &fixture.caller,
                &ChatHostListQuery::new(PageRequest::new(1, None), Vec::new()),
            )
            .await,
        Err(crate::CoreError::Forbidden)
    );

    let first = fixture
        .service
        .list_chat_hosts(
            &reader(),
            &ChatHostListQuery::new(PageRequest::new(1, None), Vec::new()),
        )
        .await
        .expect("an operator reads one bounded host page");
    assert_eq!(first.items.len(), 1);
    let second = fixture
        .service
        .list_chat_hosts(
            &reader(),
            &ChatHostListQuery::new(PageRequest::new(1, first.next), Vec::new()),
        )
        .await
        .expect("the continuation reads the remaining host");
    assert_eq!(second.items.len(), 1);
    assert!(second.next.is_none());
    assert_ne!(first.items[0].owner, second.items[0].owner);
}

#[tokio::test]
async fn paused_retirement_revokes_threads_and_keeps_checkpoint_objects_and_history() {
    let fixture = fixture("retire-paused", 2).await;
    let before = host(&fixture).await;
    use_package(&fixture, 1, "paused-retirement").await;
    let outcome = fixture
        .service
        .pause_chat_host(&before.id, "2099-09-01T02:00:00Z")
        .await
        .expect("the host pauses with a verified checkpoint");
    let ChatPauseOutcome::Complete(checkpoint) = outcome else {
        panic!("expected a complete pause, got {outcome:?}");
    };

    let retired = fixture
        .service
        .retire_chat_host(&operator(), &before.id)
        .await
        .expect("a paused host retires");
    assert_eq!(retired.status, ChatHostStatus::Retired);
    assert_eq!(retired.checkpoint.as_ref(), Some(&checkpoint));
    assert!(retired.runtime_id.is_none());
    assert!(
        fixture
            .checkpoints
            .read_workspace(
                &retired.id,
                &checkpoint.workspace_object_reference,
                &checkpoint.workspace_archive_sha256,
            )
            .await
            .expect("checkpoint storage remains readable")
            .is_some(),
        "retirement must not delete the published checkpoint object"
    );

    for thread in &fixture.threads {
        let durable = fixture
            .sessions
            .get(&thread.id)
            .await
            .unwrap()
            .expect("thread history remains durable");
        assert_eq!(durable.session.status, SessionStatus::Terminated);
        assert_eq!(durable.session.stop_reason, Some(StopReason::Revoked));
        assert_eq!(durable.session.recovery_state, RecoveryState::Retired);
        let events = fixture
            .service
            .events_since(&fixture.caller, &thread.id, None)
            .await
            .expect("the owner can still read thread history");
        assert!(events.iter().any(|event| {
            matches!(
                &event.kind,
                EventKind::StatusChanged {
                    status: SessionStatus::Terminated,
                    stop_reason: Some(StopReason::Revoked),
                    detail: Some(detail),
                    ..
                } if detail.contains("Administrative chat-host retirement")
            )
        }));
    }

    let repeated = fixture
        .service
        .retire_chat_host(&operator(), &before.id)
        .await
        .expect("retiring a retired host is idempotent");
    assert_eq!(repeated.revision, retired.revision);
}

#[tokio::test]
async fn active_retirement_never_deletes_before_checkpoint_verification() {
    let fixture = fixture("retire-active-verification", 1).await;
    let before = host(&fixture).await;
    use_package(&fixture, 1, "unverified-retirement").await;
    fixture
        .checkpoints
        .tamper_next_read
        .store(true, Ordering::SeqCst);

    assert!(fixture
        .service
        .retire_chat_host(&operator(), &before.id)
        .await
        .is_err());
    assert!(fixture.adapter.terminated.lock().await.is_empty());
    assert_eq!(host(&fixture).await.status, ChatHostStatus::Active);
    let thread = fixture
        .sessions
        .get(&fixture.threads[0].id)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(thread.session.stop_reason, Some(StopReason::Revoked));

    use_package(&fixture, 2, "verified-retirement").await;
    let retired = fixture
        .service
        .retire_chat_host(&operator(), &before.id)
        .await
        .expect("a later verified capture permits retirement");
    assert_eq!(retired.status, ChatHostStatus::Retired);
    assert!(retired.checkpoint.is_some());
    assert_eq!(fixture.adapter.terminated.lock().await.len(), 1);
}

#[tokio::test]
async fn active_retirement_retains_every_checkpoint_object_generation() {
    let fixture = fixture("retire-active-retention", 1).await;
    let before = host(&fixture).await;
    use_package(&fixture, 1, "checkpoint-before-retirement").await;
    let first = match fixture
        .service
        .checkpoint_chat_host(&before.id, "2026-09-01T02:00:00Z")
        .await
        .expect("the active host records an earlier checkpoint")
    {
        ChatCheckpointOutcome::Complete(checkpoint) => checkpoint,
        outcome => panic!("expected a complete checkpoint, got {outcome:?}"),
    };

    use_package(&fixture, 2, "checkpoint-at-retirement").await;
    let retired = fixture
        .service
        .retire_chat_host(&operator(), &before.id)
        .await
        .expect("the active host retires after another verified capture");
    assert_ne!(retired.checkpoint.as_ref(), Some(&first));
    assert!(
        fixture
            .checkpoints
            .read_workspace(
                &before.id,
                &first.workspace_object_reference,
                &first.workspace_archive_sha256,
            )
            .await
            .expect("the earlier workspace checkpoint remains readable")
            .is_some(),
        "retirement must not prune an earlier workspace generation"
    );
    for conversation in &first.conversations {
        assert!(
            fixture
                .checkpoints
                .read_conversation(
                    &conversation.session_id,
                    &conversation.object_reference,
                    &conversation.sha256,
                )
                .await
                .expect("the earlier conversation checkpoint remains readable")
                .is_some(),
            "retirement must not prune an earlier conversation generation"
        );
    }
}

#[tokio::test]
async fn restart_replays_the_exact_retirement_capture_claim() {
    let fixture = fixture("retire-restart-capture", 2).await;
    let before = host(&fixture).await;
    let old_runtime = before.runtime_id.clone().expect("the host is active");
    use_package(&fixture, 1, "retirement-restart-capture").await;
    fixture
        .adapter
        .hold_checkpoint
        .store(true, Ordering::SeqCst);

    let service = Arc::clone(&fixture.service);
    let host_id = before.id.clone();
    let retiring =
        tokio::spawn(async move { service.retire_chat_host(&operator(), &host_id).await });
    tokio::time::timeout(
        Duration::from_secs(2),
        fixture.adapter.checkpoint_started.acquire(),
    )
    .await
    .expect("retirement reaches the held checkpoint")
    .expect("the fake adapter remains available")
    .forget();
    let claimed = host(&fixture).await;
    assert_eq!(claimed.status, ChatHostStatus::Pausing);
    assert_eq!(claimed.checkpoint_sequence, 1);
    assert!(claimed
        .last_transition_claim
        .as_deref()
        .is_some_and(|claim| claim.starts_with("chrt_")));
    retiring.abort();
    assert!(retiring.await.unwrap_err().is_cancelled());

    let restarted = build_service(
        Arc::clone(&fixture.adapter),
        Arc::clone(&fixture.hosts),
        Arc::clone(&fixture.sessions),
        Arc::clone(&fixture.checkpoints),
    );
    restarted.reconcile().await;

    let retired = host(&fixture).await;
    assert_eq!(retired.status, ChatHostStatus::Retired);
    assert_eq!(retired.runtime_id, None);
    assert_eq!(retired.last_transition_claim, None);
    assert_eq!(
        retired
            .checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.checkpoint_version),
        Some(1)
    );
    assert!(fixture
        .adapter
        .terminated
        .lock()
        .await
        .contains(&old_runtime));
    let requests = fixture.adapter.checkpoint_requests.lock().await;
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|(_, request)| request.version == 1));
}

#[tokio::test]
async fn restart_finishes_retirement_after_verified_runtime_teardown() {
    let fixture = fixture("retire-restart-after-teardown", 2).await;
    let before = host(&fixture).await;
    let old_runtime = before.runtime_id.clone().expect("the host is active");
    use_package(&fixture, 1, "retirement-before-teardown").await;
    let checkpoint = match fixture
        .service
        .checkpoint_chat_host(&before.id, "2026-09-01T02:00:00Z")
        .await
        .expect("the verified retirement boundary is durable")
    {
        ChatCheckpointOutcome::Complete(checkpoint) => checkpoint,
        outcome => panic!("expected a complete checkpoint, got {outcome:?}"),
    };
    let checkpointed = host(&fixture).await;
    let mut torn_down = checkpointed.clone();
    torn_down.status = ChatHostStatus::Paused;
    torn_down.last_transition_claim = Some("chrt_restart_after_teardown".into());
    torn_down.transition_started_at = Some(checkpoint.captured_at.clone());
    fixture
        .hosts
        .update_if_revision(checkpointed.revision, &torn_down)
        .await
        .expect("the host store remains available")
        .expect("the teardown boundary wins its CAS");
    fixture.adapter.stop_serving(&old_runtime).await;

    let restarted = build_service(
        Arc::clone(&fixture.adapter),
        Arc::clone(&fixture.hosts),
        Arc::clone(&fixture.sessions),
        Arc::clone(&fixture.checkpoints),
    );
    restarted.reconcile().await;

    let retired = host(&fixture).await;
    assert_eq!(retired.status, ChatHostStatus::Retired);
    assert_eq!(retired.runtime_id, None);
    assert_eq!(retired.checkpoint, Some(checkpoint));
    assert_eq!(retired.last_transition_claim, None);
    for thread in &fixture.threads {
        let record = fixture
            .sessions
            .get(&thread.id)
            .await
            .expect("the session store remains available")
            .expect("the retired thread remains durable");
        assert_eq!(record.session.status, SessionStatus::Terminated);
        assert_eq!(record.session.stop_reason, Some(StopReason::Revoked));
    }
}

#[tokio::test]
async fn next_new_chat_reincarnates_a_retired_host_without_losing_old_history() {
    let fixture = fixture("retire-reincarnate", 1).await;
    let before = host(&fixture).await;
    let old_thread = fixture.threads[0].id.clone();
    use_package(&fixture, 1, "reincarnation-boundary").await;
    let retired = fixture
        .service
        .retire_chat_host(&operator(), &before.id)
        .await
        .expect("the original host retires");

    let fresh_thread = fixture
        .service
        .create_session(&fixture.caller, chat_spec())
        .await
        .expect("New Chat provisions a fresh host incarnation");
    let fresh = host(&fixture).await;
    assert_eq!(fresh.id, retired.id);
    assert_eq!(fresh.status, ChatHostStatus::Active);
    assert!(fresh.runtime_generation > retired.runtime_generation);
    assert_eq!(fresh.checkpoint, None);
    assert_eq!(fresh.pinned_settings.settings_revision, 11);
    assert_eq!(fresh.threads.len(), 1);
    assert_eq!(fresh.threads[0].session_id, fresh_thread.id);
    assert_ne!(fresh.threads[0].session_id, old_thread);

    let old = fixture
        .service
        .get_session(&fixture.caller, &old_thread)
        .await
        .expect("the retired incarnation's history remains readable");
    assert_eq!(old.status, SessionStatus::Terminated);
    assert_eq!(old.stop_reason, Some(StopReason::Revoked));
    assert!(fixture
        .sessions
        .events
        .lock()
        .await
        .iter()
        .filter(|event| event.session_id == old_thread)
        .any(|event| matches!(event.kind, StoredEventKind::Known(_))));
}
