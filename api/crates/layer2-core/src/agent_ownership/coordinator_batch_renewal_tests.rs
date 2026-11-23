use super::support::{BatchReply, RenewalEvent};
use super::*;

#[tokio::test(start_paused = true)]
async fn batch_renewal_orders_exact_tokens_and_bounds_chunks() {
    let fixture = Fixture::with_batch(17, true, true).await;
    let now = Instant::now();
    for (index, owner) in fixture.owners.iter().enumerate() {
        owner
            .renew_until(now + Duration::from_millis(14_000 - index as u64))
            .unwrap();
    }
    fixture.manager.maintain().await.unwrap();
    let expected: Vec<_> = fixture
        .owners
        .iter()
        .rev()
        .map(|owner| owner.token.agent_id.clone())
        .collect();
    let mut events = vec![RenewalEvent::Heartbeat];
    events.extend(
        expected
            .chunks(8)
            .map(|ids| RenewalEvent::Chunk(ids.to_vec())),
    );
    assert_eq!(*fixture.store.batch_events.lock().unwrap(), events);
    assert!(fixture
        .owners
        .iter()
        .all(|owner| owner.established_deadline() > Some(now + Duration::from_secs(14))));
}

#[tokio::test(start_paused = true)]
async fn batch_renewal_preserves_the_disabled_and_unsupported_paths() {
    for (supported, enabled) in [(true, false), (false, true)] {
        let fixture = Fixture::with_batch(1, supported, enabled).await;
        *fixture.store.reply.lock().unwrap() = Reply::Renew;
        fixture.manager.maintain().await.unwrap();
        assert_eq!(fixture.store.calls.load(Ordering::SeqCst), 1);
        assert!(fixture
            .store
            .batch_events
            .lock()
            .unwrap()
            .iter()
            .all(|event| !matches!(event, RenewalEvent::Chunk(_))));
    }
    let empty = Fixture::with_batch(0, true, true).await;
    empty.manager.maintain().await.unwrap();
    assert_eq!(
        *empty.store.batch_events.lock().unwrap(),
        [RenewalEvent::Heartbeat]
    );
}

#[tokio::test(start_paused = true)]
async fn batch_renewal_refreshes_membership_between_chunks() {
    let fixture = Fixture::with_batch(17, true, true).await;
    *fixture.store.batch_delay.lock().unwrap() = Duration::from_millis(1600);
    fixture.manager.maintain().await.unwrap();
    let events = fixture.store.batch_events.lock().unwrap();
    assert!(matches!(
        events.as_slice(),
        [
            RenewalEvent::Heartbeat,
            RenewalEvent::Chunk(_),
            RenewalEvent::Chunk(_),
            RenewalEvent::Heartbeat,
            RenewalEvent::Chunk(_)
        ]
    ));
    assert!(fixture.owners.iter().all(|owner| owner.is_current()));
}

#[tokio::test(start_paused = true)]
async fn batch_fallback_checks_heartbeat_before_continuing() {
    for skip in [false, true] {
        let fixture = Fixture::with_batch(3, true, true).await;
        *fixture.store.batch_reply.lock().unwrap() = BatchReply::Partial;
        *fixture.store.reply.lock().unwrap() = Reply::Renew;
        *fixture.store.delay.lock().unwrap() = Duration::from_millis(1600);
        fixture.store.all.store(true, Ordering::SeqCst);
        let tick = tokio::spawn({
            let manager = fixture.manager.clone();
            async move { manager.maintain().await }
        });
        if skip {
            tokio::time::sleep(Duration::from_millis(3100)).await;
            fixture.store.heartbeat_mode.store(7, Ordering::SeqCst);
        }
        tick.await.unwrap().unwrap();
        let events = fixture.store.batch_events.lock().unwrap();
        assert!(matches!(
            &events[0..5],
            [
                RenewalEvent::Heartbeat,
                RenewalEvent::Chunk(_),
                RenewalEvent::Individual(_),
                RenewalEvent::Individual(_),
                RenewalEvent::Heartbeat,
            ]
        ));
        assert_eq!(events.len(), if skip { 5 } else { 6 });
        assert!(fixture.manager.is_ready());
    }
}

#[tokio::test(start_paused = true)]
async fn batch_renewal_rejects_wrong_receipts_and_never_falls_back_on_uncertainty() {
    let wrong = Fixture::with_batch(2, true, true).await;
    *wrong.store.batch_reply.lock().unwrap() = BatchReply::DifferentToken;
    assert!(matches!(
        wrong.manager.maintain().await,
        Err(OwnershipError::Invalid(_))
    ));
    wrong.assert_failed();
    assert_eq!(wrong.store.batch_events.lock().unwrap().len(), 2);

    let uncertain = Fixture::with_batch(2, true, true).await;
    *uncertain.store.batch_reply.lock().unwrap() =
        BatchReply::Failure(OwnershipError::Unavailable("uncertain commit".into()));
    uncertain.manager.maintain().await.unwrap();
    assert!(uncertain
        .owners
        .iter()
        .all(|owner| owner.receiver_cancelled()));
    assert!(uncertain.manager.is_ready());
    assert_eq!(uncertain.store.batch_events.lock().unwrap().len(), 2);

    let stale = Fixture::with_batch(1, true, true).await;
    *stale.store.batch_reply.lock().unwrap() = BatchReply::Failure(OwnershipError::Stale);
    assert!(matches!(
        stale.manager.maintain().await,
        Err(OwnershipError::Stale)
    ));
    stale.assert_failed();
    assert_eq!(stale.store.batch_events.lock().unwrap().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn batch_renewal_keeps_deliberate_retirement_from_reviving() {
    let fixture = Fixture::with_batch(2, true, true).await;
    fixture.store.gate.store(true, Ordering::SeqCst);
    let manager = fixture.manager.clone();
    let task = tokio::spawn(async move { manager.maintain().await });
    fixture.store.entered.notified().await;
    fixture.owners[0].retiring.store(true, Ordering::Release);
    fixture.owners[0].revoke();
    fixture.store.resume.notify_one();
    task.await.unwrap().unwrap();
    assert!(!fixture.owners[0].is_current());
    assert!(fixture.owners[1].is_current());
    assert!(fixture.manager.is_ready());
}

#[tokio::test(start_paused = true)]
async fn batch_renewal_is_fenced_by_the_existing_owner_and_member_deadlines() {
    for member in [true, false] {
        let fixture = Fixture::with_batch(1, true, true).await;
        if member {
            fixture.store.heartbeat_mode.store(5, Ordering::SeqCst);
        } else {
            fixture.owners[0]
                .renew_until(Instant::now() + Duration::from_millis(500))
                .unwrap();
        }
        fixture.store.gate.store(true, Ordering::SeqCst);
        assert!(matches!(
            fixture.manager.maintain().await,
            Err(OwnershipError::Stale)
        ));
        fixture.assert_failed();
    }
}

#[tokio::test(start_paused = true)]
async fn cancelling_batch_maintenance_drops_the_call_and_releases_the_lane() {
    let fixture = Fixture::with_batch(1, true, true).await;
    fixture.store.gate.store(true, Ordering::SeqCst);
    let manager = fixture.manager.clone();
    let task = tokio::spawn(async move { manager.maintain().await });
    fixture.store.entered.notified().await;
    let established = fixture.owners[0].established_deadline();
    task.abort();
    assert!(matches!(task.await, Err(error) if error.is_cancelled()));
    assert_eq!(fixture.owners[0].established_deadline(), established);
    fixture.store.gate.store(false, Ordering::SeqCst);
    tokio::time::advance(Duration::from_millis(300)).await;
    fixture.manager.maintain().await.unwrap();
    assert!(fixture.owners[0].established_deadline() > established);
}
