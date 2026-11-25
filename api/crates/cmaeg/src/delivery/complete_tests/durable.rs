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

#[derive(Default)]
struct SideAndAnswer {
    side_started: Notify,
    release_side: Notify,
    bearers: Mutex<Vec<String>>,
}

impl RelayTransport for SideAndAnswer {
    fn post(
        &self,
        _: Url,
        headers: HeaderMap,
        body: Vec<u8>,
        _: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        let payload = String::from_utf8(body).unwrap();
        let side = payload.contains("side notice");
        let bearer = headers[reqwest::header::AUTHORIZATION].to_str().unwrap();
        self.bearers.lock().unwrap().push(bearer.into());
        Box::pin(async move {
            if side {
                self.side_started.notify_one();
                self.release_side.notified().await;
                return Err(TransportError::Unavailable);
            }
            Ok(HttpReply::new(
                StatusCode::OK,
                br#"{"message_id":"out-answer","platform_message_id":"p-answer"}"#.to_vec(),
            ))
        })
    }
}

#[tokio::test]
async fn event_side_message_pending_relay_send_leaves_live_answer_and_reply_token_available() {
    use crate::delivery::{
        gate::Ungated,
        intents::{DeliveryCommand, ReplySource},
        router::DeliverySink as _,
    };

    let (sender, _, mut channel, event) = setup();
    Arc::get_mut(&mut channel)
        .unwrap()
        .definition
        .secrets
        .channel_key = Some(Secret::new("side-channel-fixture"));
    let transport = Arc::new(SideAndAnswer::default());
    let relay = sender.relay.with_transport(transport.clone());
    let (tx, rx) = tokio::sync::mpsc::channel(2);
    let tracker = crate::turns::TurnTracker::new(Default::default());
    let engine = DeliveryEngine::start(
        rx,
        sender.keys,
        relay,
        crate::DeliveryConfig::default(),
        tracker,
        sender.custom,
    );
    let delivery = engine.side_messages_if(true).unwrap();
    let notice = message("side notice".into());
    let mut side = Box::pin(delivery.send(&channel, &event, &notice, &Ungated));
    tokio::select! {
        _ = transport.side_started.notified() => {},
        _ = &mut side => panic!("the pending side send settled before the transport ran"),
    }

    let scheduler = ShardedScheduler::new(tx);
    let (receipt, receive) = tokio::sync::oneshot::channel();
    scheduler
        .try_submit(DeliveryCommand::Reply {
            trace: tracing::Span::none(),
            channel: channel.clone(),
            event: event.clone(),
            message: message("ordinary answer".into()),
            source: ReplySource::Stream,
            receipt,
        })
        .unwrap();
    let answer = tokio::time::timeout(Duration::from_secs(2), receive)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(answer.delivery, DeliveryOutcome::Confirmed { .. }));
    transport.release_side.notify_one();
    assert!(matches!(
        side.await.delivery,
        DeliveryOutcome::Unknown { .. }
    ));

    assert_eq!(
        *transport.bearers.lock().unwrap(),
        [
            "Bearer side-channel-fixture",
            "Bearer single-use-test-fixture"
        ]
    );
    drop(scheduler);
    assert!(
        engine
            .drain(std::time::Instant::now() + Duration::from_secs(1))
            .await
    );
}
