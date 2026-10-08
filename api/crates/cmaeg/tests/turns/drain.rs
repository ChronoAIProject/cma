use super::{fake_provider::FakeProviderConfig, support::Fixture};
use cmaeg::ingress::launch::{LaunchOutcome, TurnLauncher};
use cmaeg_core::problem::ProblemCode;
use std::time::{Duration, Instant};

#[tokio::test]
async fn drain_waits_for_consumers_refuses_new_launches_and_fails_readiness() {
    let mut fixture = Fixture::new(
        FakeProviderConfig {
            terminal_delay: Duration::from_millis(180),
            ..Default::default()
        },
        Default::default(),
        4,
    )
    .await;
    assert!(matches!(
        fixture.runner.launch(fixture.request("first")).await,
        LaunchOutcome::Accepted { .. }
    ));
    assert!(fixture.tracker.oldest_started_at().is_some());
    fixture.tracker.start_draining();
    assert!(!fixture.readiness.report().ready);
    assert!(fixture.readiness.report().reasons.contains(&"draining"));
    assert!(
        matches!(fixture.runner.launch(fixture.request("second")).await,LaunchOutcome::Refused(problem) if problem.problem.code==ProblemCode::Draining && problem.problem.status==503)
    );
    let started = Instant::now();
    assert!(
        fixture
            .tracker
            .drain(Instant::now() + Duration::from_secs(2))
            .await
    );
    assert!(started.elapsed() >= Duration::from_millis(140));
    assert!(fixture.tracker.is_empty());
    fixture.terminal().await;
    fixture.fake.close().await;
}
#[tokio::test]
async fn caller_cancellation_does_not_cancel_admitted_work() {
    let mut fixture = Fixture::new(
        FakeProviderConfig {
            response_delay: Duration::from_millis(150),
            ..Default::default()
        },
        Default::default(),
        4,
    )
    .await;
    assert!(tokio::time::timeout(
        Duration::from_millis(20),
        fixture.runner.launch(fixture.request("cancelled-caller"))
    )
    .await
    .is_err());
    assert_eq!(fixture.tracker.len(), 1);
    let outcomes = fixture.terminal().await;
    assert_eq!(outcomes.last().unwrap().kind(), "completed");
    assert!(
        fixture
            .tracker
            .drain(Instant::now() + Duration::from_secs(1))
            .await
    );
    fixture.fake.close().await;
}
#[tokio::test]
async fn deadline_aborts_idle_consumers_and_duplicate_events_have_separate_guards() {
    let fixture = Fixture::new(
        FakeProviderConfig {
            response_delay: Duration::from_secs(2),
            ..Default::default()
        },
        cmaeg::TurnConfig {
            accept_budget: Duration::from_millis(10),
            ..Default::default()
        },
        4,
    )
    .await;
    assert!(matches!(
        fixture.runner.launch(fixture.request("same")).await,
        LaunchOutcome::Deferred
    ));
    assert!(matches!(
        fixture.runner.launch(fixture.request("same")).await,
        LaunchOutcome::Deferred
    ));
    assert_eq!(fixture.tracker.len(), 2);
    assert!(
        !fixture
            .tracker
            .drain(Instant::now() + Duration::from_millis(10))
            .await
    );
    tokio::time::timeout(Duration::from_millis(100), async {
        while !fixture.tracker.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    fixture.fake.close().await;
}
#[tokio::test]
async fn tool_reply_dedupe_is_bound_to_exact_reference_and_expiry() {
    let fixture = Fixture::new(Default::default(), Default::default(), 4).await;
    let request = fixture.request("event");
    let reference = request.event_ref.as_str();
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    fixture
        .tracker
        .record_tool_reply(reference, now + 10, now)
        .unwrap();
    assert!(fixture.tracker.has_tool_reply(reference, now + 9));
    assert!(!fixture
        .tracker
        .has_tool_reply(fixture.request("event").event_ref.as_str(), now + 9));
    assert!(!fixture.tracker.has_tool_reply(reference, now + 10));
    assert!(fixture
        .tracker
        .record_tool_reply(reference, now, now)
        .is_err());
    assert!(fixture
        .tracker
        .record_tool_reply(reference, now + 1801, now)
        .is_err());
    fixture.fake.close().await;
}
