use super::*;
use crate::discord::{
    journal_fixture::Fixture,
    status::{Outcome, Stage},
    NativeDiscordJournal,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::mpsc;

struct RecoveringPort {
    fixture: Fixture,
    calls: AtomicUsize,
    results: mpsc::UnboundedSender<Result<(), GatewayError>>,
}
impl SourcePort for RecoveringPort {
    fn revision<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<String, GatewayError>> {
        Box::pin(async { Ok("unchanged".into()) })
    }
    fn connect<'a>(
        &'a self,
        _: &'a str,
        mut stop: watch::Receiver<bool>,
        status: StatusAttempt,
    ) -> BoxFuture<'a, Attempt> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let journal = super::super::DurableJournal::acquire_store_observed(
                self.fixture.store.clone(),
                &self.fixture.record,
                self.fixture.api.clone(),
                Arc::new(crate::turns::observer::NoopObserver),
                status,
            )
            .await;
            let result = match journal {
                Err(error) => {
                    self.results.send(Err(error)).unwrap();
                    Err(error)
                }
                Ok(journal) => {
                    let authorized = journal.authorize_connection(None).await;
                    self.results.send(authorized).unwrap();
                    let _ = stop.changed().await;
                    journal.release().await.unwrap();
                    Ok(GatewayEnd {
                        reason: EndReason::Stopped,
                        checkpoint: None,
                        retry_after: Duration::ZERO,
                    })
                }
            };
            Attempt {
                result,
                cooldown: Duration::ZERO,
            }
        })
    }
}

#[tokio::test]
async fn real_discovery_refusal_recovers_without_revision_change_and_still_owns_one_lease() {
    let fixture = Fixture::new().await;
    fixture.memory.gateway.lock().unwrap()["session_start_limit"]["total"] = 0.into();
    let (results, mut rx) = mpsc::unbounded_channel();
    let port = Arc::new(RecoveringPort {
        fixture,
        calls: AtomicUsize::new(0),
        results,
    });
    let registry = StatusRegistry::default();
    let (stop, stopped) = watch::channel(false);
    let task = tokio::spawn(run(
        port.clone(),
        "source".into(),
        stopped,
        Timing::default(),
        registry.clone(),
    ));
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap(),
        Err(GatewayError::SessionLimitUnavailable)
    );
    assert!(port.fixture.memory.rows.lock().unwrap().is_empty());
    let view = registry.view("source", Some("unchanged"));
    assert_eq!(view.outcome, Outcome::Retrying);
    assert_eq!(view.last_failure_stage, Some(Stage::Discovery));
    assert_eq!(
        view.last_failure_code,
        Some(FailureCode::SessionLimitInvalid)
    );
    assert!(view.retry_at.is_some());
    port.fixture.memory.gateway.lock().unwrap()["session_start_limit"]["total"] = 10.into();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(12), rx.recv())
            .await
            .unwrap()
            .unwrap(),
        Ok(())
    );
    assert_eq!(port.calls.load(Ordering::SeqCst), 2);
    assert!(matches!(
        port.fixture.acquire().await,
        Err(GatewayError::Authority)
    ));
    stop.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
}

struct RefusingPort {
    error: GatewayError,
    calls: AtomicUsize,
}
impl SourcePort for RefusingPort {
    fn revision<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<String, GatewayError>> {
        Box::pin(async { Ok("unchanged".into()) })
    }
    fn connect<'a>(
        &'a self,
        _: &'a str,
        _: watch::Receiver<bool>,
        status: StatusAttempt,
    ) -> BoxFuture<'a, Attempt> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            status.step(Stage::Discovery);
            Attempt {
                result: Err(self.error),
                cooldown: Duration::ZERO,
            }
        })
    }
}

#[tokio::test(start_paused = true)]
async fn repeated_invalid_responses_cannot_hot_loop_and_shutdown_interrupts_backoff() {
    let port = Arc::new(RefusingPort {
        error: GatewayError::SessionLimitUnavailable,
        calls: AtomicUsize::new(0),
    });
    let (stop, stopped) = watch::channel(false);
    let task = tokio::spawn(run(
        port.clone(),
        "source".into(),
        stopped,
        Timing::default(),
        StatusRegistry::default(),
    ));
    tokio::task::yield_now().await;
    assert_eq!(port.calls.load(Ordering::SeqCst), 1);
    for (attempt, (floor, ceiling)) in [
        (5_000, 7_500),
        (10_000, 15_000),
        (20_000, 30_000),
        (40_000, 60_000),
    ]
    .into_iter()
    .enumerate()
    {
        tokio::time::advance(Duration::from_millis(floor - 1)).await;
        tokio::task::yield_now().await;
        assert_eq!(port.calls.load(Ordering::SeqCst), attempt + 1);
        tokio::time::advance(Duration::from_millis(ceiling - floor + 1)).await;
        tokio::task::yield_now().await;
        assert_eq!(port.calls.load(Ordering::SeqCst), attempt + 2);
    }
    stop.send(true).unwrap();
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn definitive_discovery_authentication_failure_remains_terminal() {
    let port = Arc::new(RefusingPort {
        error: GatewayError::Authentication,
        calls: AtomicUsize::new(0),
    });
    let (stop, stopped) = watch::channel(false);
    let registry = StatusRegistry::default();
    let task = tokio::spawn(run(
        port.clone(),
        "source".into(),
        stopped,
        Timing::default(),
        registry.clone(),
    ));
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(180)).await;
    tokio::task::yield_now().await;
    assert_eq!(port.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        registry.view("source", Some("unchanged")).outcome,
        Outcome::Blocked
    );
    stop.send(true).unwrap();
    task.await.unwrap();
}

#[test]
fn exponential_jitter_bounds_and_provider_cooldown_are_preserved() {
    for (attempt, floor) in [
        (0, 5_000),
        (1, 5_000),
        (2, 10_000),
        (3, 20_000),
        (4, 40_000),
        (u32::MAX, 40_000),
    ] {
        assert_eq!(
            session_limit_delay(attempt, 0),
            Duration::from_millis(floor)
        );
        assert_eq!(
            session_limit_delay(attempt, floor / 2),
            Duration::from_millis(floor * 3 / 2)
        );
        for jitter in [1, 99, u64::MAX] {
            let d = session_limit_delay(attempt, jitter);
            assert!(d >= Duration::from_millis(floor) && d <= Duration::from_millis(floor * 3 / 2));
        }
    }
    let attempt = Attempt {
        result: Err(GatewayError::SessionLimitUnavailable),
        cooldown: Duration::from_secs(120),
    };
    for _ in 0..100 {
        assert_eq!(
            retry_delay(&attempt, u32::MAX, Duration::ZERO),
            Duration::from_secs(120)
        );
    }
}
