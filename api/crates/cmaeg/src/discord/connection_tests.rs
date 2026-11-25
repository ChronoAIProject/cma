use super::super::{
    journal::DurableJournal, journal_fixture::*, journal_model::Lease, status::StatusRegistry,
    Checkpoint, EndReason, GatewayEnd,
};
use std::{sync::Arc, time::Duration};
use tokio::sync::{watch, Notify};

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
    let entered = Arc::new(Notify::new());
    let held = Arc::new(Notify::new());
    *f.memory.channel_gate.lock().unwrap() = Some((entered.clone(), held.clone()));
    let connection = async move {
        entered.notified().await;
        Ok(GatewayEnd {
            reason: EndReason::Reconnect,
            checkpoint: Some(Checkpoint {
                session_id: "fixture-session".into(),
                resume_url: "wss://gateway-us-east1-b.discord.gg".into(),
                sequence: 7,
            }),
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
    assert!(lease.checkpoint.is_none() || lease.checkpoint == end.checkpoint);
    let view = statuses.view(&f.record, None);
    assert_eq!(view.lease, "released");
    assert_eq!(view.outcome, super::super::status::Outcome::Pending);
    assert_eq!(
        view.last_failure_code,
        Some(super::super::status::FailureCode::Reconnect)
    );
}
