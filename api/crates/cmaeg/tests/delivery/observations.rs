//! Each legacy delivery step reports its outcome for the event's default
//! consumer, to the event's origin.
use super::support::App;
use cmaeg::{
    delivery::intents::DeliveryOutcome,
    turns::{
        observer::{ActivityObserver, DeliveryStep, Destination},
        outcomes::TurnOutcome,
    },
};
use cmaeg_core::ids::{ConsumerId, EventId, RecordId};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Recording(Mutex<Vec<String>>);

impl ActivityObserver for Recording {
    fn delivered(
        &self,
        _record: &RecordId,
        _event_id: &EventId,
        consumer: &ConsumerId,
        step: DeliveryStep,
        outcome: &DeliveryOutcome,
        destination: Destination,
    ) {
        self.0.lock().unwrap().push(format!(
            "{}:{step:?}:{}:{destination:?}",
            consumer.as_str(),
            outcome.kind()
        ));
    }
}

#[tokio::test]
async fn answers_failures_and_tool_replies_report_their_step() {
    let recording = Arc::new(Recording::default());
    let mut app = App::observed(0, recording.clone()).await;
    app.outcome(TurnOutcome::Accepted {
        response_id: "response".into(),
        thread_reply: false,
    })
    .await;
    app.outcome(TurnOutcome::MessageCommitted {
        text: "an answer".into(),
    })
    .await;
    app.outcome(TurnOutcome::Failed {
        code: "failed".into(),
        message: "private diagnostic".into(),
    })
    .await;
    let receipt = app.reply(json!({"text":"from the tool"})).await;
    assert_eq!(receipt.delivery.kind(), "confirmed");
    app.drain().await;
    assert_eq!(
        *recording.0.lock().unwrap(),
        [
            "default:Answer:confirmed:Origin",
            "default:Failure:confirmed:Origin",
            "default:ToolReply:confirmed:Origin",
        ]
    );
}

#[tokio::test]
async fn named_replies_report_only_their_own_consumer() {
    let recording = Arc::new(Recording::default());
    let mut app = App::observed(0, recording.clone()).await;
    for id in ["csm_00000000000a", "csm_00000000000b"] {
        app.event.consumer = Some(ConsumerId::parse(id).unwrap());
        assert_eq!(
            app.reply(json!({"text":"reply"})).await.delivery.kind(),
            "confirmed"
        );
    }
    app.drain().await;
    assert_eq!(
        *recording.0.lock().unwrap(),
        [
            "csm_00000000000a:ToolReply:confirmed:Origin",
            "csm_00000000000b:ToolReply:confirmed:Origin",
        ]
    );
}
