use super::*;
use std::time::Duration;

#[tokio::test(start_paused = true)]
async fn local_subscription_without_admission_receiver_never_waits() {
    let recorder = super::super::DurableAgentUpdateRecorder::new(Arc::new(
        crate::sandbox::store::InMemorySandboxStore::default(),
    ));
    let mut subscription = recorder.subscribe_local(&AgentId("local-admission".into()));
    let started = Instant::now();
    subscription.admit().await.unwrap();
    assert_eq!(started.elapsed(), Duration::ZERO);
}

#[tokio::test(start_paused = true)]
async fn unanswered_admission_falls_back_at_the_supplied_deadline_once() {
    let (_, subscription) = AgentLiveSubscription::channel();
    let (_sender, receiver) = tokio::sync::oneshot::channel();
    let deadline = Duration::from_secs(3);
    let mut subscription = subscription.with_admission_deadline(receiver, deadline);
    let started = Instant::now();
    subscription.admit().await.unwrap();
    assert_eq!(started.elapsed(), deadline);
    subscription.admit().await.unwrap();
    assert_eq!(started.elapsed(), deadline, "admission is not repeated");
}

#[tokio::test(start_paused = true)]
async fn capacity_refusal_is_typed_and_disconnected_sources_allow_replay() {
    for outcome in [
        Ok(()),
        Err(AgentLiveError::Capacity),
        Err(AgentLiveError::Unavailable),
    ] {
        let (_, subscription) = AgentLiveSubscription::channel();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let mut subscription =
            subscription.with_admission_deadline(receiver, Duration::from_secs(3));
        sender.send(outcome).unwrap();
        let expected = if outcome == Err(AgentLiveError::Capacity) {
            outcome
        } else {
            Ok(())
        };
        assert_eq!(subscription.admit().await, expected);
    }
}
