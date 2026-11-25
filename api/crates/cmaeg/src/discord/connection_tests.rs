use super::super::{
    journal::DurableJournal, journal_fixture::*, journal_model::Lease, status::StatusRegistry,
    Checkpoint, Dispatch, EndReason, GatewayEnd, GatewayError, NativeDiscordJournal,
};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio::sync::{watch, Notify};

async fn save_session(journal: &DurableJournal) -> Checkpoint {
    let ready = Dispatch {
        name: "READY".into(),
        sequence: 7,
        data: json!({
            "session_id": "fixture-session",
            "resume_gateway_url": "wss://gateway.discord.gg/",
            "user": {"id": "77", "bot": true},
        }),
    };
    let checkpoint = super::super::protocol::checkpoint(None, &ready, "77").unwrap();
    journal.authorize_connection(None).await.unwrap();
    journal.admit(&checkpoint, &ready).await.unwrap();
    checkpoint
}

async fn invalid_session(journal: &DurableJournal, closed: bool) -> GatewayEnd {
    let (stop, _stopped) = watch::channel(false);
    let (_shutdown, shutdown) = watch::channel(false);
    journal
        .supervise_connection(
            async {
                Ok(GatewayEnd {
                    reason: EndReason::InvalidSession,
                    checkpoint: None,
                    retry_after: Duration::from_secs(5),
                    transport_closed: closed,
                })
            },
            stop,
            shutdown,
            Duration::from_secs(10),
        )
        .await
        .unwrap()
}

// Discord's op 7 reconnect can arrive while a lease renewal is inside the store.
// That renewal holds the journal's serial guard; cleanup must still finish.
#[tokio::test]
async fn reconnect_during_renewal_still_releases_the_lease_and_returns() {
    let f = Fixture::new().await;
    let statuses = StatusRegistry::default();
    let journal = DurableJournal::acquire_store_observed(
        f.store.clone(),
        &f.record,
        f.api.clone(),
        Arc::new(crate::turns::observer::NoopObserver),
        statuses.begin(&f.record, None),
    )
    .await
    .unwrap();
    let checkpoint = save_session(&journal).await;
    let entered = Arc::new(Notify::new());
    let held = Arc::new(Notify::new());
    *f.memory.channel_gate.lock().unwrap() = Some((entered.clone(), held.clone()));
    let expected = checkpoint.clone();
    let connection = async move {
        entered.notified().await;
        Ok(GatewayEnd {
            reason: EndReason::Reconnect,
            checkpoint: Some(expected),
            retry_after: Duration::from_secs(5),
            transport_closed: true,
        })
    };
    let (stop, _stopped) = watch::channel(false);
    let (_shutdown, shutdown) = watch::channel(false);
    let end = tokio::time::timeout(
        Duration::from_secs(20),
        journal.supervise_connection(connection, stop, shutdown, Duration::from_millis(10)),
    )
    .await
    .expect("cleanup must not wait behind the suspended renewal")
    .unwrap();
    *f.memory.channel_gate.lock().unwrap() = None;
    held.notify_waiters();
    assert_eq!(end.reason, EndReason::Reconnect);
    assert!(end.checkpoint.is_some(), "op 7 keeps the resume checkpoint");
    let (lease, _) = f
        .store
        .load::<Lease>(&journal.id, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lease.lease_until, 0, "the next attempt can take the lease");
    assert!(lease.checkpoint.as_ref() == Some(&checkpoint));
    let successor = f.acquire().await.unwrap();
    assert!(successor.checkpoint().await.unwrap() == Some(checkpoint));
    let view = statuses.view(&f.record, None);
    assert_eq!(view.lease, "released");
    assert_eq!(view.outcome, super::super::status::Outcome::Pending);
    assert_eq!(
        view.last_failure_code,
        Some(super::super::status::FailureCode::Reconnect)
    );
}

#[tokio::test]
async fn invalid_session_clear_timeout_keeps_the_lease_until_cleanup_is_confirmed() {
    let f = Fixture::new().await;
    let journal = f.acquire().await.unwrap();
    let checkpoint = save_session(&journal).await;
    let entered = Arc::new(Notify::new());
    let held = Arc::new(Notify::new());
    *f.memory.next_write_gate.lock().unwrap() = Some((entered.clone(), held.clone()));
    let cleanup = invalid_session(&journal, true);
    tokio::pin!(cleanup);
    tokio::select! {
        _ = entered.notified() => {}
        _ = &mut cleanup => panic!("cleanup returned before attempting the fenced write"),
    }
    tokio::time::pause();
    tokio::time::advance(super::CLEANUP_TIMEOUT).await;
    tokio::time::resume();
    assert_eq!(cleanup.await.reason, EndReason::InvalidSession);

    // The stalled write has not applied. A second release must not expose this checkpoint.
    assert!(journal.checkpoint().await.unwrap() == Some(checkpoint));
    assert!(matches!(f.acquire().await, Err(GatewayError::Authority)));
    held.notify_one();
}

#[tokio::test]
async fn confirmed_invalid_session_releases_without_a_checkpoint_even_if_ack_is_lost() {
    for lose_ack in [false, true] {
        let f = Fixture::new().await;
        let journal = f.acquire().await.unwrap();
        let checkpoint = save_session(&journal).await;
        let (_, version) = f
            .store
            .load::<Lease>(&journal.id, false)
            .await
            .unwrap()
            .unwrap();
        if lose_ack {
            f.fail(&journal.id, true);
        }
        assert_eq!(
            invalid_session(&journal, true).await.reason,
            EndReason::InvalidSession
        );
        let (lease, released_version) = f
            .store
            .load::<Lease>(&journal.id, false)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(released_version, version + 1);
        assert_eq!(lease.lease_until, 0);
        assert!(lease.checkpoint.is_none());

        let successor = f.acquire().await.unwrap();
        assert!(successor.checkpoint().await.unwrap().is_none());
        assert_eq!(
            successor.authorize_connection(Some(&checkpoint)).await,
            Err(GatewayError::Authority)
        );
        f.advance(5_001);
        successor.authorize_connection(None).await.unwrap();
    }
}

#[tokio::test]
async fn unconfirmed_invalid_session_discards_the_checkpoint_and_retains_the_lease() {
    let f = Fixture::new().await;
    let journal = f.acquire().await.unwrap();
    save_session(&journal).await;
    assert_eq!(
        invalid_session(&journal, false).await.reason,
        EndReason::InvalidSession
    );
    assert!(journal.checkpoint().await.unwrap().is_none());
    assert!(matches!(f.acquire().await, Err(GatewayError::Authority)));
    f.advance(30_001);
    assert!(f
        .acquire()
        .await
        .unwrap()
        .checkpoint()
        .await
        .unwrap()
        .is_none());
}
