//! Delivery-held request clones must never extend a routed turn's lease.
use super::{fake_provider::FakeProviderConfig, support::Fixture};
use cmaeg::{
    ingress::launch::{LaunchOutcome, TurnLauncher},
    turns::{
        capacity::{CapacityScope, RoutedCapacity},
        outcomes::DeliveryCommand,
    },
    RoutedCapacityConfig, TurnConfig,
};
use std::time::{Duration, Instant};

fn capacity() -> RoutedCapacity {
    RoutedCapacity::new(RoutedCapacityConfig {
        per_channel: 1,
        per_owner: 1,
    })
}
fn leased(fixture: &Fixture, capacity: &RoutedCapacity) -> cmaeg::ingress::launch::LaunchRequest {
    let request = fixture.request("leased-turn");
    *request.lease.lock().unwrap() = Some(
        capacity
            .try_lease(
                &fixture.channel.pointer.record_id,
                &fixture.channel.definition.owner.subject,
            )
            .unwrap(),
    );
    request
}
fn available(fixture: &Fixture, capacity: &RoutedCapacity) {
    let permit = capacity
        .try_lease(
            &fixture.channel.pointer.record_id,
            &fixture.channel.definition.owner.subject,
        )
        .unwrap();
    drop(permit);
}
fn occupied(fixture: &Fixture, capacity: &RoutedCapacity) {
    assert_eq!(
        capacity
            .try_lease(
                &fixture.channel.pointer.record_id,
                &fixture.channel.definition.owner.subject
            )
            .unwrap_err(),
        CapacityScope::Channel
    );
}

#[tokio::test]
async fn completion_releases_a_lease_while_delivery_retains_the_request() {
    let mut fixture = Fixture::new(
        FakeProviderConfig {
            terminal_delay: Duration::from_millis(150),
            ..Default::default()
        },
        Default::default(),
        4,
    )
    .await;
    let capacity = capacity();
    assert!(matches!(
        fixture.runner.launch(leased(&fixture, &capacity)).await,
        LaunchOutcome::Accepted { .. }
    ));
    occupied(&fixture, &capacity);
    assert!(
        fixture
            .tracker
            .drain(Instant::now() + Duration::from_secs(2))
            .await
    );
    // No delivery commands have been consumed yet; they all retain Arc<LaunchRequest>.
    assert!(!fixture.outcomes.is_empty());
    available(&fixture, &capacity);
    while let Ok(command) = fixture.outcomes.try_recv() {
        let DeliveryCommand::StreamOutcome { turn, .. } = command else {
            panic!("stream outcome");
        };
        assert!(turn.lease.lock().unwrap().is_none());
    }
    fixture.fake.close().await;
}

#[tokio::test]
async fn timeout_releases_the_turn_lease() {
    let mut fixture = Fixture::new(
        FakeProviderConfig {
            terminal_delay: Duration::from_secs(5),
            ..Default::default()
        },
        TurnConfig {
            turn_timeout: Duration::from_millis(150),
            ..Default::default()
        },
        4,
    )
    .await;
    let capacity = capacity();
    assert!(matches!(
        fixture.runner.launch(leased(&fixture, &capacity)).await,
        LaunchOutcome::Accepted { .. }
    ));
    occupied(&fixture, &capacity);
    assert_eq!(fixture.terminal().await.last().unwrap().kind(), "failed");
    assert!(
        fixture
            .tracker
            .drain(Instant::now() + Duration::from_secs(1))
            .await
    );
    available(&fixture, &capacity);
    fixture.fake.close().await;
}

#[tokio::test]
async fn spawn_refusal_releases_capacity_without_starting_a_provider() {
    let fixture = Fixture::new(Default::default(), Default::default(), 4).await;
    let capacity = capacity();
    fixture.tracker.start_draining();
    let outcome = fixture.runner.launch(leased(&fixture, &capacity)).await;
    assert!(
        matches!(outcome, LaunchOutcome::Refused(problem) if problem.problem.detail.as_deref() == Some("draining"))
    );
    assert!(fixture.fake.requests().is_empty());
    available(&fixture, &capacity);
    fixture.fake.close().await;
}

#[tokio::test]
async fn drain_cancellation_releases_capacity() {
    let fixture = Fixture::new(
        FakeProviderConfig {
            terminal_delay: Duration::from_secs(5),
            ..Default::default()
        },
        Default::default(),
        4,
    )
    .await;
    let capacity = capacity();
    assert!(matches!(
        fixture.runner.launch(leased(&fixture, &capacity)).await,
        LaunchOutcome::Accepted { .. }
    ));
    assert!(!fixture.tracker.drain(Instant::now()).await);
    tokio::time::timeout(Duration::from_secs(1), async {
        while !fixture.tracker.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    available(&fixture, &capacity);
    fixture.fake.close().await;
}

#[tokio::test]
async fn legacy_turns_have_no_lease_and_ignore_exhausted_routed_capacity() {
    let fixture = Fixture::new(Default::default(), Default::default(), 4).await;
    let capacity = capacity();
    let held = capacity
        .try_lease(
            &fixture.channel.pointer.record_id,
            &fixture.channel.definition.owner.subject,
        )
        .unwrap();
    let request = fixture.request("legacy-turn");
    assert!(request.activity.routing.is_none());
    assert!(request.lease.lock().unwrap().is_none());
    assert!(!request.sync_fanout);
    assert!(matches!(
        fixture.runner.launch(request).await,
        LaunchOutcome::Accepted { .. }
    ));
    assert!(
        fixture
            .tracker
            .drain(Instant::now() + Duration::from_secs(1))
            .await
    );
    occupied(&fixture, &capacity);
    drop(held);
    available(&fixture, &capacity);
    fixture.fake.close().await;
}
