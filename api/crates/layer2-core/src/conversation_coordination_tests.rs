//! Durable inbound-conversation authority contract tests for the canonical fake.

use std::time::Duration;

use crate::coordination::{
    ConversationOpeningLeaseRequest, ConversationWriteAuthority, SessionBoundary,
    SessionLeaseRequest, SessionOperation,
};
use crate::event::{Event, EventKind};
use crate::ids::{KernelId, SessionId, SubjectId};
use crate::session::{RecoveryState, Session, SessionStatus};
use crate::store::{SessionRecord, SessionStore, StoreError};
use crate::test_support::{service, FakeAdapter, FakeStore};
use crate::{AgentOptions, TerminalGeometry};

fn id() -> SessionId {
    SessionId("sesn_conversation_coordination".into())
}

fn record(runtime_id: &str) -> SessionRecord {
    let mut record = SessionRecord::new(Session {
        id: id(),
        owner: SubjectId("subject-conversation-coordination".into()),
        kernel: KernelId::new("fake"),
        status: SessionStatus::Ready,
        recovery_state: RecoveryState::Normal,
        stop_reason: None,
        options: AgentOptions::default(),
        environment_profile: None,
        geometry: TerminalGeometry::default(),
        last_seq: 0,
        runtime_version: None,
        rotations: 0,
        rotated_at: None,
        capabilities: Default::default(),
        granted_services: None,
        granted_skills: None,
        pricing: None,
        created_at: "2026-08-26T00:00:00Z".into(),
    });
    record.runtime_id = Some(runtime_id.into());
    record
}

fn record_snapshot(record: &SessionRecord) -> serde_json::Value {
    serde_json::json!({
        "session": &record.session,
        "runtime_id": &record.runtime_id,
        "context_ref": &record.context_ref,
        "rotation_claim": &record.rotation_claim,
        "revision": record.revision,
    })
}

fn compatibility_request(
    token: &str,
    runtime_id: &str,
    ttl: Duration,
) -> ConversationOpeningLeaseRequest {
    ConversationOpeningLeaseRequest {
        session_id: id(),
        holder: format!("holder-{token}"),
        token: token.into(),
        runtime_id: runtime_id.into(),
        runtime_generation: 0,
        runtime_candidate_token: None,
        ttl,
    }
}

fn message(seq: u64, content: &str) -> Event {
    Event {
        session_id: id(),
        seq,
        kind: EventKind::MessageCommitted {
            content: content.into(),
        },
        appended_at: String::new(),
    }
}

#[tokio::test]
async fn pending_claims_preserve_the_active_link_and_release_exactly() {
    let store = FakeStore::default();
    store.seed(&record("runtime-a")).await.unwrap();

    let first = store
        .acquire_conversation_opening(
            &compatibility_request("opening-a", "runtime-a", Duration::from_secs(30)),
            None,
        )
        .await
        .unwrap();
    let first_link = store.publish_conversation_link(&first, None).await.unwrap();
    assert!(first_link.epoch > 0);

    let second = store
        .acquire_conversation_opening(
            &compatibility_request("opening-b", "runtime-a", Duration::from_secs(30)),
            None,
        )
        .await
        .unwrap();
    store
        .validate_conversation_link(&first_link)
        .await
        .expect("a pending successor must not displace the active link");
    assert!(matches!(
        store
            .acquire_conversation_opening(
                &compatibility_request("opening-c", "runtime-a", Duration::from_secs(30)),
                None,
            )
            .await,
        Err(StoreError::ConversationOpeningHeld { id: held }) if held == id()
    ));

    store.release_conversation_opening(&second).await.unwrap();
    store.release_conversation_opening(&second).await.unwrap();
    let third = store
        .acquire_conversation_opening(
            &compatibility_request("opening-c", "runtime-a", Duration::from_secs(30)),
            None,
        )
        .await
        .unwrap();
    store.release_conversation_opening(&second).await.unwrap();
    store
        .renew_conversation_opening(&third, None, Duration::from_secs(30))
        .await
        .expect("a stale release cannot clear its successor");
    let third_link = store.publish_conversation_link(&third, None).await.unwrap();
    assert!(third_link.epoch > first_link.epoch);
    store.release_conversation_opening(&second).await.unwrap();
    store.validate_conversation_link(&third_link).await.unwrap();
    assert!(matches!(
        store.validate_conversation_link(&first_link).await,
        Err(StoreError::ConversationLinkChanged { .. })
    ));
}

#[tokio::test]
async fn an_unknown_opening_acquisition_retries_the_exact_committed_claim() {
    let store = std::sync::Arc::new(FakeStore::default());
    store.seed(&record("runtime-a")).await.unwrap();
    store.lose_next_conversation_acquire_response();
    let service = service(
        std::sync::Arc::clone(&store),
        std::sync::Arc::new(FakeAdapter::default()),
    );

    let claim = tokio::time::timeout(
        Duration::from_secs(1),
        service.acquire_conversation_opening(&id(), "runtime-a", 0, None, None),
    )
    .await
    .expect("the exact acquisition retry does not stall")
    .expect("the committed acquisition is reconciled");

    assert_eq!(store.conversation_acquire_calls(), 2);
    assert_eq!(
        store.active_conversation_opening(&id()).await,
        Some(claim.authority().clone())
    );
    claim.revoke().await;
}

#[tokio::test(start_paused = true)]
async fn cancelling_an_unknown_opening_acquisition_releases_the_exact_claim() {
    let store = std::sync::Arc::new(FakeStore::default());
    store.seed(&record("runtime-a")).await.unwrap();
    store.lose_next_conversation_acquire_response();
    let service = service(
        std::sync::Arc::clone(&store),
        std::sync::Arc::new(FakeAdapter::default()),
    );

    let acquiring = tokio::spawn(async move {
        service
            .acquire_conversation_opening(&id(), "runtime-a", 0, None, None)
            .await
    });
    for _ in 0..32 {
        if store.active_conversation_opening(&id()).await.is_some() {
            break;
        }
        assert!(store.conversation_acquire_calls() <= 1);
        tokio::task::yield_now().await;
    }
    assert_eq!(store.conversation_acquire_calls(), 1);
    assert!(store.active_conversation_opening(&id()).await.is_some());

    acquiring.abort();
    assert!(matches!(acquiring.await, Err(error) if error.is_cancelled()));
    tokio::time::advance(Duration::from_millis(5)).await;
    for _ in 0..32 {
        if store.active_conversation_opening(&id()).await.is_none() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        store.conversation_acquire_calls(),
        2,
        "cleanup resolves the exact unknown acquisition before releasing it"
    );
    assert_eq!(store.active_conversation_opening(&id()).await, None);

    let successor = store
        .acquire_conversation_opening(
            &compatibility_request("opening-after-cancel", "runtime-a", Duration::from_secs(30)),
            None,
        )
        .await
        .expect("cancellation cleanup permits an immediate successor");
    store
        .release_conversation_opening(&successor)
        .await
        .unwrap();
}

#[tokio::test(start_paused = true)]
async fn an_unknown_acquisition_releases_if_backing_changes_before_retry() {
    let store = std::sync::Arc::new(FakeStore::default());
    store.disable_coordination();
    store.seed(&record("runtime-a")).await.unwrap();
    store.lose_next_conversation_acquire_response();
    let service = service(
        std::sync::Arc::clone(&store),
        std::sync::Arc::new(FakeAdapter::default()),
    );

    let acquiring = tokio::spawn(async move {
        service
            .acquire_conversation_opening(&id(), "runtime-a", 0, None, None)
            .await
    });
    for _ in 0..32 {
        if store.active_conversation_opening(&id()).await.is_some() {
            break;
        }
        assert!(store.conversation_acquire_calls() <= 1);
        tokio::task::yield_now().await;
    }
    assert_eq!(store.conversation_acquire_calls(), 1);
    assert!(store.active_conversation_opening(&id()).await.is_some());

    let mut changed = store.get(&id()).await.unwrap().unwrap();
    changed.runtime_id = Some("runtime-b".into());
    store
        .commit_session_idempotent("change-opening-backing", &changed, &[])
        .await
        .unwrap();
    assert!(store.active_conversation_opening(&id()).await.is_some());

    tokio::time::advance(Duration::from_millis(5)).await;
    let error = match acquiring.await.unwrap() {
        Ok(_) => panic!("changed backing must reject the exact acquisition retry"),
        Err(error) => error,
    };
    assert!(matches!(error, StoreError::ConversationOpeningLost { .. }));
    assert_eq!(store.conversation_acquire_calls(), 2);
    assert_eq!(store.active_conversation_opening(&id()).await, None);

    let successor = store
        .acquire_conversation_opening(
            &compatibility_request(
                "opening-after-backing-change",
                "runtime-b",
                Duration::from_secs(30),
            ),
            None,
        )
        .await
        .expect("ambiguous-acquisition recovery permits an immediate successor");
    store
        .release_conversation_opening(&successor)
        .await
        .unwrap();
}

#[tokio::test]
async fn an_unknown_publication_retries_without_advancing_the_epoch_twice() {
    let store = std::sync::Arc::new(FakeStore::default());
    store.seed(&record("runtime-a")).await.unwrap();
    let service = service(
        std::sync::Arc::clone(&store),
        std::sync::Arc::new(FakeAdapter::default()),
    );
    let claim = service
        .acquire_conversation_opening(&id(), "runtime-a", 0, None, None)
        .await
        .unwrap();
    store.lose_next_conversation_publish_response();
    let opening_store: std::sync::Arc<dyn SessionStore> = store.clone();
    let (_updates, opening) =
        crate::conversation_opening::ConversationOpening::channel(1, claim, opening_store);

    let (_, link, _, _, publication) =
        tokio::time::timeout(Duration::from_secs(1), opening.publish(None))
            .await
            .expect("the exact publication retry does not stall")
            .expect("the committed publication is reconciled");

    assert_eq!(store.conversation_publish_calls(), 2);
    assert_eq!(link.epoch, 1, "the response retry must not republish");
    store.validate_conversation_link(&link).await.unwrap();
    publication.revoke().await;
}

#[tokio::test]
async fn expiry_rejects_the_old_opening_without_mutation_or_a_write_receipt() {
    let store = FakeStore::default();
    store.seed(&record("runtime-a")).await.unwrap();
    let expired = store
        .acquire_conversation_opening(
            &compatibility_request("expired", "runtime-a", Duration::from_millis(1)),
            None,
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;
    let successor = store
        .acquire_conversation_opening(
            &compatibility_request("successor", "runtime-a", Duration::from_secs(30)),
            None,
        )
        .await
        .expect("store-clock expiry permits a successor");

    let before = store.get(&id()).await.unwrap().unwrap();
    let mut stale = before.clone();
    stale.revision = 99;
    stale.session.last_seq = 99;
    assert!(matches!(
        store
            .commit_conversation_idempotent(
                "expired-opening-write",
                &ConversationWriteAuthority::Opening {
                    opening: expired,
                    session_lease: None,
                },
                &stale,
                &[message(99, "must not commit")],
            )
            .await,
        Err(StoreError::ConversationOpeningLost { .. })
    ));
    assert_eq!(
        record_snapshot(&store.get(&id()).await.unwrap().unwrap()),
        record_snapshot(&before)
    );
    assert!(store.events_since(&id(), 0).await.unwrap().is_empty());

    let revision = store
        .commit_conversation_idempotent(
            "expired-opening-write",
            &ConversationWriteAuthority::Opening {
                opening: successor,
                session_lease: None,
            },
            &before,
            &[],
        )
        .await
        .expect("the rejected stale write did not reserve its write ID");
    assert_eq!(revision, before.revision + 1);
}

#[tokio::test]
async fn publication_is_idempotent_and_stale_active_writes_have_no_effect() {
    let store = FakeStore::default();
    store.seed(&record("runtime-a")).await.unwrap();
    let first = store
        .acquire_conversation_opening(
            &compatibility_request("opening-a", "runtime-a", Duration::from_secs(30)),
            None,
        )
        .await
        .unwrap();
    let first_link = store.publish_conversation_link(&first, None).await.unwrap();
    assert_eq!(
        store.publish_conversation_link(&first, None).await.unwrap(),
        first_link
    );

    let second = store
        .acquire_conversation_opening(
            &compatibility_request("opening-b", "runtime-a", Duration::from_secs(30)),
            None,
        )
        .await
        .unwrap();
    let second_link = store
        .publish_conversation_link(&second, None)
        .await
        .unwrap();
    assert!(second_link.epoch > first_link.epoch);
    assert!(matches!(
        store.publish_conversation_link(&first, None).await,
        Err(StoreError::ConversationOpeningLost { .. })
    ));

    let before = store.get(&id()).await.unwrap().unwrap();
    let mut stale = before.clone();
    stale.revision = 41;
    stale.session.last_seq = 41;
    assert!(matches!(
        store
            .commit_conversation_idempotent(
                "stale-active-write",
                &ConversationWriteAuthority::Active(first_link),
                &stale,
                &[message(41, "must not commit")],
            )
            .await,
        Err(StoreError::ConversationLinkChanged { .. })
    ));
    assert_eq!(
        record_snapshot(&store.get(&id()).await.unwrap().unwrap()),
        record_snapshot(&before)
    );
    assert!(store.events_since(&id(), 0).await.unwrap().is_empty());
    store
        .commit_conversation_idempotent(
            "stale-active-write",
            &ConversationWriteAuthority::Active(second_link.clone()),
            &before,
            &[],
        )
        .await
        .expect("the rejected stale link did not reserve its write ID");

    store.release_conversation_opening(&first).await.unwrap();
    store
        .validate_conversation_link(&second_link)
        .await
        .unwrap();
    store.release_conversation_opening(&second).await.unwrap();
    assert!(matches!(
        store.validate_conversation_link(&second_link).await,
        Err(StoreError::ConversationLinkChanged { .. })
    ));
    let successor = store
        .acquire_conversation_opening(
            &compatibility_request("opening-c", "runtime-a", Duration::from_secs(30)),
            None,
        )
        .await
        .expect("cancellation cleanup does not block a successor");
    store
        .publish_conversation_link(&successor, None)
        .await
        .expect("a successor can immediately replace an unobserved publication");
}

#[tokio::test]
async fn released_publication_tokens_cannot_collide_with_later_opening_tokens() {
    let store = FakeStore::default();
    store.seed(&record("runtime-a")).await.unwrap();
    let first = store
        .acquire_conversation_opening(
            &compatibility_request("opening-a", "runtime-a", Duration::from_secs(30)),
            None,
        )
        .await
        .unwrap();
    let first_link = store.publish_conversation_link(&first, None).await.unwrap();
    store.release_conversation_opening(&first).await.unwrap();
    assert!(matches!(
        store.validate_conversation_link(&first_link).await,
        Err(StoreError::ConversationLinkChanged { .. })
    ));

    let collision = store
        .acquire_conversation_opening(
            &compatibility_request("released:opening-a", "runtime-a", Duration::from_secs(30)),
            None,
        )
        .await
        .expect("opening tokens are opaque and may contain a former sentinel prefix");
    let successor = store
        .publish_conversation_link(&collision, None)
        .await
        .expect("the arbitrary successor token publishes a new epoch");
    assert!(successor.epoch > first_link.epoch);
    store.validate_conversation_link(&successor).await.unwrap();
}

#[tokio::test]
async fn nonzero_openings_require_the_exact_lease_and_registered_candidate() {
    let store = FakeStore::default();
    store.seed(&record("runtime-zero")).await.unwrap();
    let authority = store.runtime_authority(&id()).await.unwrap().unwrap();
    let request = SessionLeaseRequest::new(
        id(),
        "runtime-holder",
        SessionBoundary {
            status: SessionStatus::Ready,
            authority,
        },
        SessionOperation::Handover,
        Duration::from_secs(30),
    );
    let mut lease = store.acquire_session_lease(&request).await.unwrap();
    let reservation = store
        .reserve_runtime_generation(&lease, "reservation-one")
        .await
        .unwrap();
    let candidate = store
        .register_runtime_candidate(&lease, &reservation, "runtime-one")
        .await
        .unwrap();

    let no_lease = ConversationOpeningLeaseRequest {
        session_id: id(),
        holder: "forged-no-lease-holder".into(),
        token: "forged-no-lease".into(),
        runtime_id: "runtime-one".into(),
        runtime_generation: candidate.generation.get(),
        runtime_candidate_token: None,
        ttl: Duration::from_secs(30),
    };
    assert!(matches!(
        store.acquire_conversation_opening(&no_lease, None).await,
        Err(StoreError::ConversationOpeningLost { .. })
    ));

    let before = store.get(&id()).await.unwrap().unwrap();
    let forged_candidate = ConversationOpeningLeaseRequest {
        session_id: id(),
        holder: lease.holder.clone(),
        token: "forged-candidate-opening".into(),
        runtime_id: "runtime-forged".into(),
        runtime_generation: candidate.generation.get() + 1,
        runtime_candidate_token: Some("unregistered-candidate".into()),
        ttl: Duration::from_secs(30),
    };
    assert!(matches!(
        store
            .acquire_conversation_opening(&forged_candidate, Some(&lease))
            .await,
        Err(StoreError::ConversationOpeningLost { .. })
    ));
    assert_eq!(
        record_snapshot(&store.get(&id()).await.unwrap().unwrap()),
        record_snapshot(&before)
    );
    assert_eq!(store.active_conversation_opening(&id()).await, None);

    let wrong_generation = ConversationOpeningLeaseRequest {
        session_id: id(),
        holder: lease.holder.clone(),
        token: "wrong-generation-opening".into(),
        runtime_id: candidate.runtime_id.clone(),
        runtime_generation: candidate.generation.get() + 1,
        runtime_candidate_token: Some(candidate.token.clone()),
        ttl: Duration::from_secs(30),
    };
    assert!(matches!(
        store
            .acquire_conversation_opening(&wrong_generation, Some(&lease))
            .await,
        Err(StoreError::ConversationOpeningLost { .. })
    ));
    assert_eq!(
        record_snapshot(&store.get(&id()).await.unwrap().unwrap()),
        record_snapshot(&before)
    );
    assert_eq!(store.active_conversation_opening(&id()).await, None);

    let wrong_runtime = ConversationOpeningLeaseRequest {
        session_id: id(),
        holder: lease.holder.clone(),
        token: "wrong-runtime-opening".into(),
        runtime_id: "runtime-forged".into(),
        runtime_generation: candidate.generation.get(),
        runtime_candidate_token: Some(candidate.token.clone()),
        ttl: Duration::from_secs(30),
    };
    assert!(matches!(
        store
            .acquire_conversation_opening(&wrong_runtime, Some(&lease))
            .await,
        Err(StoreError::ConversationOpeningLost { .. })
    ));
    assert_eq!(
        record_snapshot(&store.get(&id()).await.unwrap().unwrap()),
        record_snapshot(&before)
    );
    assert_eq!(store.active_conversation_opening(&id()).await, None);

    let candidate_request = ConversationOpeningLeaseRequest {
        session_id: id(),
        holder: lease.holder.clone(),
        token: "candidate-opening".into(),
        runtime_id: candidate.runtime_id.clone(),
        runtime_generation: candidate.generation.get(),
        runtime_candidate_token: Some(candidate.token.clone()),
        ttl: Duration::from_secs(30),
    };
    let candidate_opening = store
        .acquire_conversation_opening(&candidate_request, Some(&lease))
        .await
        .expect("the exact registered candidate may acquire before the switch");
    assert_eq!(
        store.active_conversation_opening(&id()).await,
        Some(candidate_opening.clone())
    );
    store
        .switch_runtime_authority(&mut lease, &candidate)
        .await
        .unwrap();
    let candidate_link = store
        .publish_conversation_link(&candidate_opening, Some(&lease))
        .await
        .expect("the exact candidate publishes after becoming authoritative");
    store
        .validate_conversation_link(&candidate_link)
        .await
        .unwrap();
    store
        .release_conversation_opening(&candidate_opening)
        .await
        .unwrap();
    assert!(matches!(
        store.validate_conversation_link(&candidate_link).await,
        Err(StoreError::ConversationLinkChanged { .. })
    ));

    let readoption = ConversationOpeningLeaseRequest {
        session_id: id(),
        holder: lease.holder.clone(),
        token: "readoption-opening".into(),
        runtime_id: "runtime-one".into(),
        runtime_generation: candidate.generation.get(),
        runtime_candidate_token: None,
        ttl: Duration::from_secs(30),
    };
    let opening = store
        .acquire_conversation_opening(&readoption, Some(&lease))
        .await
        .expect("a current nonzero readoption carries its exact lease");
    let link = store
        .publish_conversation_link(&opening, Some(&lease))
        .await
        .unwrap();

    let reservation = store
        .reserve_runtime_generation(&lease, "reservation-two")
        .await
        .unwrap();
    let candidate = store
        .register_runtime_candidate(&lease, &reservation, "runtime-two")
        .await
        .unwrap();
    let mut stale_record = store.get(&id()).await.unwrap().unwrap();
    stale_record.session.last_seq += 1;
    let stale_event = Event {
        session_id: id(),
        seq: stale_record.session.last_seq,
        kind: EventKind::TurnStarted,
        appended_at: String::new(),
    };
    store
        .switch_runtime_authority(&mut lease, &candidate)
        .await
        .unwrap();
    assert!(matches!(
        store.validate_conversation_link(&link).await,
        Err(StoreError::ConversationLinkChanged { .. })
    ));
    assert!(matches!(
        store
            .publish_conversation_link(&opening, Some(&lease))
            .await,
        Err(StoreError::ConversationOpeningLost { .. })
    ));
    let switched_tail = store.event_tail(&id()).await.unwrap();
    assert!(matches!(
        store
            .commit_conversation_idempotent(
                "stale-runtime-one-append",
                &ConversationWriteAuthority::Active(link),
                &stale_record,
                &[stale_event],
            )
            .await,
        Err(StoreError::ConversationLinkChanged { .. })
    ));
    assert_eq!(
        store.event_tail(&id()).await.unwrap(),
        switched_tail,
        "the old authority cannot append after the successor marker"
    );
}
