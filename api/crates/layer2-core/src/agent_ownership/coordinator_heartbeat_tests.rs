//! A transient member heartbeat failure skips one tick only while the
//! established membership deadline holds; it never extends any deadline.
use super::*;
use crate::agent_ownership::HEARTBEAT_INTERVAL_MS;

const TICK: Duration = Duration::from_millis(HEARTBEAT_INTERVAL_MS);

async fn renewing_fixture() -> Fixture {
    let fixture = Fixture::new(2).await;
    *fixture.store.reply.lock().unwrap() = Reply::Renew;
    fixture.store.all.store(true, Ordering::SeqCst);
    fixture
}

fn assert_serving(fixture: &Fixture) {
    assert!(fixture.manager.is_ready());
    assert!(!*fixture.manager.failure_signal().borrow());
    for authority in &fixture.owners {
        assert!(authority.is_current());
        assert!(!authority.receiver_cancelled());
    }
}

#[tokio::test(start_paused = true)]
async fn transient_heartbeat_failure_skips_one_tick_and_next_heartbeat_restores_renewal() {
    // Late committed reply, unavailable committed reply, uncommitted failure.
    for mode in [1, 6, 7] {
        let fixture = renewing_fixture().await;
        let original = fixture.leases().await;
        tokio::time::advance(TICK).await;
        fixture.store.heartbeat_mode.store(mode, Ordering::SeqCst);
        assert!(fixture.manager.maintain().await.unwrap().is_empty());
        assert_serving(&fixture);
        assert_eq!(fixture.store.calls.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.leases().await, original);

        fixture.store.heartbeat_mode.store(0, Ordering::SeqCst);
        fixture.manager.maintain().await.unwrap();
        assert_eq!(fixture.store.calls.load(Ordering::SeqCst), 2);
        for (before, after) in original.iter().zip(fixture.leases().await) {
            assert_eq!(after.token, before.token);
            assert!(after.expires_at_ms > before.expires_at_ms);
        }
        // Past the original lease, the renewed membership and owners still hold.
        tokio::time::advance(Duration::from_secs(13)).await;
        assert_serving(&fixture);
    }
}

#[tokio::test(start_paused = true)]
async fn skipped_ticks_extend_neither_membership_nor_any_authority_then_fail_as_before() {
    for mode in [6, 7] {
        let fixture = renewing_fixture().await;
        let original = fixture.leases().await;
        let started = Instant::now();
        fixture.store.heartbeat_mode.store(mode, Ordering::SeqCst);
        for _ in 0..4 {
            tokio::time::advance(TICK).await;
            assert!(fixture.manager.maintain().await.unwrap().is_empty());
            assert_serving(&fixture);
        }
        assert_eq!(fixture.store.calls.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.leases().await, original);
        // Registration and acquisition established 15 s deadlines at start.
        tokio::time::advance(
            Duration::from_secs(15) - started.elapsed() - Duration::from_millis(1),
        )
        .await;
        assert_serving(&fixture);
        tokio::time::advance(Duration::from_millis(1)).await;
        assert!(!fixture.manager.is_ready());
        assert!(fixture.owners.iter().all(|owner| !owner.is_current()));
        assert!(matches!(
            fixture.manager.maintain().await,
            Err(OwnershipError::Stale)
        ));
        fixture.assert_failed();
        assert_eq!(fixture.store.calls.load(Ordering::SeqCst), 0);
        assert!(fixture
            .owners
            .iter()
            .all(|owner| !owner.receiver_cancelled()));
    }
}

#[tokio::test(start_paused = true)]
async fn heartbeat_failure_at_or_after_the_established_deadline_is_fatal() {
    // The bounded call ends 2 s after it starts; membership ends at 15 s.
    for start_ms in [12_999, 13_000, 13_001] {
        let fixture = renewing_fixture().await;
        tokio::time::advance(Duration::from_millis(start_ms)).await;
        fixture.store.heartbeat_mode.store(1, Ordering::SeqCst);
        let started = Instant::now();
        let result = fixture.manager.maintain().await;
        match start_ms {
            12_999 => {
                assert!(result.unwrap().is_empty());
                assert_serving(&fixture);
                continue;
            }
            13_000 => assert!(matches!(result, Err(OwnershipError::Unavailable(_)))),
            _ => {
                assert!(matches!(result, Err(OwnershipError::Stale)));
                assert_eq!(started.elapsed(), Duration::from_millis(1_999));
            }
        }
        fixture.assert_failed();
        assert_eq!(fixture.store.calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test(start_paused = true)]
async fn drain_or_process_failure_during_an_unavailable_heartbeat_is_not_skipped() {
    for drain in [false, true] {
        let fixture = renewing_fixture().await;
        fixture.store.heartbeat_mode.store(11, Ordering::SeqCst);
        let running = tokio::spawn({
            let manager = fixture.manager.clone();
            async move { manager.maintain().await }
        });
        fixture.store.entered.notified().await;
        if drain {
            fixture.manager.stop_admission();
        } else {
            fixture.manager.fail().await;
        }
        fixture.store.resume.notify_one();
        assert!(matches!(
            running.await.unwrap(),
            Err(OwnershipError::Unavailable(_))
        ));
        fixture.assert_failed();
        assert_eq!(fixture.manager.is_draining(), drain);
        assert_eq!(fixture.store.calls.load(Ordering::SeqCst), 0);
    }
}
