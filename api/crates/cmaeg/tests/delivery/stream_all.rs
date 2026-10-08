//! R6 streaming order and refused-child finalization on the real worker.
use super::{
    group_fixtures::REVIEWER,
    stream_all_fixture::{Frame, Harness},
};
use cmaeg::{
    delivery::{
        intents::{DeliveryCommand, DeliveryOutcome},
        router::DeliverySink,
    },
    ingress::launch::{LaunchOutcome, LaunchRequest, TurnLauncher},
    turns::{
        capacity::RoutedCapacity, fanout::FanoutLauncher, observer::NoopObserver,
        outcomes::TurnOutcome,
    },
};
use futures_util::future::BoxFuture;
use serde_json::json;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

fn completed() -> TurnOutcome {
    TurnOutcome::Completed {
        final_text: Some("final answer".into()),
    }
}
fn delta() -> TurnOutcome {
    TurnOutcome::TextDelta {
        text: "delta".into(),
    }
}
fn child<'a>(frames: &'a [Frame], id: &str) -> Vec<&'a Frame> {
    frames
        .iter()
        .filter(|frame| frame.2["consumer"]["id"] == id)
        .collect()
}
fn ended(frames: &[Frame]) {
    assert_eq!(
        frames
            .iter()
            .filter(|frame| frame.1 == "cmaeg.fanout.completed")
            .count(),
        1
    );
    assert_eq!(frames.last().unwrap().1, "cmaeg.fanout.completed");
    assert_eq!(
        frames.last().unwrap().2["consumers"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test(start_paused = true)]
async fn stream_all_reply_precedes_consumer_terminal() {
    let mut h = Harness::new(true, 16, Duration::from_millis(100));
    h.start();
    let mut receipts = vec![];
    let mut stalled = vec![];
    for (at, turn) in h.turns.iter().enumerate() {
        let earlier = format!("earlier webhook {at}");
        receipts.push(h.reply(turn, true, &earlier).await);
        stalled.push(h.until_hook(&earlier).await);
        // The earlier default-only webhook is still pending; enqueue the
        // origin reply and its terminal without waiting for either receipt.
        h.outcome(turn, delta()).await;
        receipts.push(h.reply(turn, false, "tool answer").await);
        h.outcome(turn, completed()).await;
    }
    for receipt in receipts {
        assert!(matches!(
            receipt.await.unwrap().delivery,
            DeliveryOutcome::Confirmed { .. }
        ));
    }
    let (frames, hooks) = h.finish().await;
    ended(&frames);
    for (id, stalled) in ["default", REVIEWER].into_iter().zip(stalled) {
        let child = child(&frames, id);
        assert_eq!(
            child
                .iter()
                .map(|frame| frame.1.as_str())
                .collect::<Vec<_>>(),
            [
                "cmaeg.consumer.delta",
                "cmaeg.consumer.message",
                "cmaeg.consumer.completed"
            ]
        );
        assert!(
            child[0].0 >= stalled + 100,
            "the earlier webhook stalled these queued frames"
        );
        assert!(
            child[2].0 >= child[1].0 + 100,
            "tool webhook delay was exercised"
        );
    }
    assert_eq!(
        hooks
            .iter()
            .filter(|body| body["type"] == "cmaeg.response.message")
            .count(),
        4
    );
}

#[tokio::test(start_paused = true)]
async fn routed_stream_all_emits_consumer_delta() {
    let mut h = Harness::new(true, 16, Duration::ZERO);
    h.start();
    for turn in &h.turns {
        h.outcome(turn, delta()).await;
    }
    for turn in h.turns.iter().rev() {
        h.outcome(
            turn,
            TurnOutcome::MessageCommitted {
                text: "message".into(),
            },
        )
        .await;
        h.outcome(turn, completed()).await;
    }
    let (frames, _) = h.finish().await;
    ended(&frames);
    for id in ["default", REVIEWER] {
        assert_eq!(
            child(&frames, id)
                .iter()
                .map(|frame| frame.1.as_str())
                .collect::<Vec<_>>(),
            [
                "cmaeg.consumer.delta",
                "cmaeg.consumer.message",
                "cmaeg.consumer.completed"
            ]
        );
    }
}

#[tokio::test(start_paused = true)]
async fn routed_plain_stream_emits_text_delta() {
    let mut h = Harness::new(false, 8, Duration::ZERO);
    h.start();
    h.outcome(&h.turns[0], delta()).await;
    h.outcome(&h.turns[0], completed()).await;
    let (frames, _) = h.finish().await;
    assert_eq!(
        frames
            .iter()
            .map(|frame| frame.1.as_str())
            .collect::<Vec<_>>(),
        ["cmaeg.text.delta", "cmaeg.completed"]
    );
    assert_eq!(frames[0].2["consumer"]["id"], "default");
}

#[tokio::test(start_paused = true)]
async fn webhook_only_reply_never_enters_the_waiter_and_stream_still_ends() {
    let mut h = Harness::new(true, 16, Duration::from_millis(100));
    h.start();
    let receipt = h.reply(&h.turns[1], true, "tool answer").await;
    for turn in &h.turns {
        h.outcome(turn, completed()).await;
    }
    assert!(matches!(
        receipt.await.unwrap().delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    let (frames, hooks) = h.finish().await;
    ended(&frames);
    assert_eq!(frames.len(), 3);
    assert!(!frames
        .iter()
        .any(|frame| frame.1 == "cmaeg.consumer.message"));
    let replies: Vec<_> = hooks
        .iter()
        .filter(|body| body["type"] == "cmaeg.response.message")
        .collect();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0]["data"]["consumer"]["id"], REVIEWER);
    assert_eq!(replies[0]["data"]["destination"], "default");
    assert_eq!(replies[0]["data"]["text"], "tool answer");
}

#[derive(Default)]
struct Accepted(Mutex<Vec<Arc<LaunchRequest>>>);
impl TurnLauncher for Accepted {
    fn launch(&self, turn: LaunchRequest) -> BoxFuture<'_, LaunchOutcome> {
        self.0.lock().unwrap().push(Arc::new(turn));
        Box::pin(async {
            LaunchOutcome::Accepted {
                response_id: "accepted".into(),
            }
        })
    }
}

#[tokio::test(start_paused = true)]
async fn refused_child_ends_stream() {
    let mut h = Harness::new(true, 1, Duration::ZERO);
    let capacity = Arc::new(RoutedCapacity::new(Default::default()));
    let held: Vec<_> = (0..3)
        .map(|_| {
            capacity
                .try_lease(
                    &h.channel.pointer.record_id,
                    &h.channel.definition.owner.subject,
                )
                .unwrap()
        })
        .collect();
    let inner = Arc::new(Accepted::default());
    let launcher = FanoutLauncher::new(
        inner.clone(),
        h.keys.clone(),
        h.sink.clone(),
        capacity.clone(),
        h.tracker.clone(),
        Duration::from_secs(30),
        Arc::new(NoopObserver),
    );
    h.sink
        .try_submit(DeliveryCommand::StreamOutcome {
            turn: h.turns[0].clone(),
            outcome: TurnOutcome::Accepted {
                response_id: "filler".into(),
            },
        })
        .unwrap();
    let request = &h.turns[0];
    // Build an independently owned request with the same sealed admitted activity.
    let request = LaunchRequest {
        channel: request.channel.clone(),
        activity: request.activity.clone(),
        event_ref: request.event_ref.clone(),
        conversation_id: request.conversation_id.clone(),
        idempotency_key: request.idempotency_key.clone(),
        received_at: request.received_at,
        trace: request.trace.clone(),
        timing: None,
        consumer: None,
        sync_fanout: true,
        lease: Mutex::new(None),
    };
    let result = launcher.launch_routed(request, None).await;
    assert!(matches!(result.combined, LaunchOutcome::Accepted { .. }));
    assert!(
        matches!(&result.consumers[1].outcome, LaunchOutcome::Refused(error) if error.code == "channel_capacity")
    );
    tokio::task::yield_now().await;
    assert_eq!(h.receiver.as_ref().unwrap().len(), 1);
    assert_eq!(h.receiver.as_ref().unwrap().capacity(), 0);
    assert_eq!(h.tracker.len(), 1);
    assert!(h.frames.lock().unwrap().is_empty());
    assert_eq!(inner.0.lock().unwrap().len(), 1);
    let admitted = inner.0.lock().unwrap()[0].clone();
    assert!(admitted.consumer.is_none());
    assert!(capacity
        .try_lease(
            &h.channel.pointer.record_id,
            &h.channel.definition.owner.subject
        )
        .is_err());
    h.start();
    h.until("cmaeg.consumer.failed").await;
    h.outcome(&admitted, completed()).await;
    let (frames, hooks) = h.finish().await;
    ended(&frames);
    assert_eq!(frames.len(), 3);
    assert_eq!(frames[0].1, "cmaeg.consumer.failed");
    assert_eq!(frames[0].2["consumer"]["id"], REVIEWER);
    assert_eq!(frames[0].2["refused"], true);
    assert_eq!(frames[1].1, "cmaeg.consumer.completed");
    assert_eq!(frames[1].2["consumer"]["id"], "default");
    assert_eq!(
        frames[2].2["consumers"],
        json!([
        {"id":"default","name":"lead","outcome":"completed"},
        {"id":REVIEWER,"name":"reviewer","outcome":"refused"}])
    );
    let failed: Vec<_> = hooks
        .iter()
        .filter(|body| body["type"] == "cmaeg.response.failed")
        .collect();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0]["data"]["consumer"]["id"], REVIEWER);
    drop(held);
}
