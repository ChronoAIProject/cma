use super::*;
use crate::delivery::{
    durable::DeliveryCoordinator, engine::DeliveryEngine, router::ShardedScheduler,
};
use tokio::sync::Notify;

#[derive(Default)]
struct HeldTransport {
    started: Notify,
    finish: Notify,
    calls: std::sync::atomic::AtomicUsize,
}
impl RelayTransport for HeldTransport {
    fn post(
        &self,
        _: Url,
        headers: HeaderMap,
        _: Vec<u8>,
        _: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        use std::sync::atomic::Ordering;
        assert!(!headers
            .values()
            .any(|value| value.as_bytes() == b"single-use-test-fixture"));
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        Box::pin(async move {
            self.finish.notified().await;
            Ok(HttpReply::new(
                StatusCode::OK,
                br#"{"message_id":"out-1","platform_message_id":"p-1"}"#.to_vec(),
            ))
        })
    }
}

#[tokio::test]
async fn coordinator_waits_for_actual_engine_send_and_preserves_consumer_sharding() {
    use std::sync::atomic::Ordering;
    let (sender, _, mut channel, mut event) = setup();
    Arc::get_mut(&mut channel)
        .unwrap()
        .definition
        .secrets
        .channel_key = Some(Secret::new("durable-channel-fixture"));
    event.consumer = Some(cmaeg_core::ids::ConsumerId::parse("csm_0123456789ab").unwrap());
    let transport = Arc::new(HeldTransport::default());
    let relay = sender.relay.with_transport(transport.clone());
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let engine = DeliveryEngine::start(
        rx,
        sender.keys,
        relay,
        crate::DeliveryConfig::default(),
        crate::turns::TurnTracker::new(Default::default()),
        sender.custom,
    );
    let coordinator = DeliveryCoordinator::new(Arc::new(ShardedScheduler::new(tx)));
    let gate = Arc::new(RecordingGate::new(true));
    let mut pending = Box::pin(coordinator.reply(
        channel,
        event,
        message("confirmed answer".into()),
        gate.clone(),
    ));
    tokio::select! {
        _ = transport.started.notified() => {},
        _ = &mut pending => panic!("queue acceptance was mistaken for completed send"),
    }
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    transport.finish.notify_one();
    let receipt = pending.await;
    assert_eq!(
        receipt.delivery,
        DeliveryOutcome::Confirmed {
            platform_message_id: Some("p-1".into())
        }
    );
    assert!(gate.calls().contains(&"attempt"));
    assert!(gate.calls().contains(&"commit"));
    drop(coordinator);
    assert!(
        engine
            .drain(std::time::Instant::now() + Duration::from_secs(1))
            .await
    );
}
