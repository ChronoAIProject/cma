//! Sibling consumers share a source event, never delivery suppression or terminal state.
use super::{
    fake_http::FakeHttp,
    recording_fixtures::{event, relay_activity, relay_channel, relay_reply, turn},
    recording_support::{Hooks, Recording},
};
use cmaeg::{
    delivery::intents::ReplySource, ingress::launch::LaunchRequest, turns::outcomes::TurnOutcome,
    DeliveryConfig,
};
use cmaeg_core::{ids::ConsumerId, seal::EventRefPayload};
use serde_json::{json, Value};
use std::sync::Arc;

fn recording() -> Recording {
    Recording::start(
        FakeHttp::platforms(),
        Hooks::answering([]),
        DeliveryConfig::default(),
    )
}
fn consumer(id: &str) -> ConsumerId {
    ConsumerId::parse(id).unwrap()
}
fn target(recording: &Recording, id: Option<ConsumerId>) -> (EventRefPayload, Arc<LaunchRequest>) {
    let channel = relay_channel(0);
    let activity = relay_activity();
    let mut event = event(&channel, &activity, 1, relay_reply());
    event.consumer = id.clone();
    let mut turn = turn(&recording.keys, &channel, activity, &event);
    Arc::get_mut(&mut turn).unwrap().consumer = id;
    (event, turn)
}
fn accepted() -> TurnOutcome {
    TurnOutcome::Accepted {
        response_id: "response".into(),
        thread_reply: false,
    }
}
fn completed(text: &str) -> TurnOutcome {
    TurnOutcome::Completed {
        final_text: Some(text.into()),
    }
}
fn texts(recording: &Value) -> Vec<String> {
    let mut texts: Vec<_> = recording["calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|call| call["body"]["reply"]["text"].as_str().unwrap().to_owned())
        .collect();
    texts.sort();
    texts
}

#[tokio::test(start_paused = true)]
async fn same_event_siblings_keep_stream_and_completion_state_independent() {
    let recording = recording();
    let (_, a) = target(&recording, Some(consumer("csm_00000000000a")));
    let (_, b) = target(&recording, Some(consumer("csm_00000000000b")));
    for turn in [&a, &b] {
        recording.outcome(turn, accepted()).await;
        recording
            .outcome(
                turn,
                TurnOutcome::TextDelta {
                    text: "partial".into(),
                },
            )
            .await;
    }
    recording.outcome(&a, completed("A final")).await;
    recording
        .outcome(
            &b,
            TurnOutcome::MessageCommitted {
                text: "B stream".into(),
            },
        )
        .await;
    recording.outcome(&b, completed("B duplicate")).await;
    recording.outcome(&a, completed("A duplicate")).await;
    let result = recording.finish().await;
    assert_eq!(texts(&result), ["A final", "B stream"]);
    // The shared inbound reply token is still single use across consumers.
    assert_eq!(
        result["calls"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|call| call["credential"] == "reply_token")
            .count(),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn one_consumers_tool_reply_does_not_suppress_a_siblings_stream_or_final() {
    for first in [None, Some(consumer("csm_00000000000a"))] {
        let mut recording = recording();
        let (event, a) = target(&recording, first);
        let (_, b) = target(&recording, Some(consumer("csm_00000000000b")));
        recording.outcome(&a, accepted()).await;
        recording.outcome(&b, accepted()).await;
        let receipt = recording
            .reply(
                &a.channel,
                &event,
                json!({"text":"A tool"}),
                ReplySource::Tool,
            )
            .await;
        assert_eq!(receipt.delivery.kind(), "confirmed");
        recording.outcome(&a, completed("A suppressed")).await;
        recording
            .outcome(
                &b,
                TurnOutcome::TextDelta {
                    text: "B partial".into(),
                },
            )
            .await;
        recording.outcome(&b, completed("B final")).await;
        assert_eq!(texts(&recording.finish().await), ["A tool", "B final"]);
    }
}

#[tokio::test(start_paused = true)]
async fn one_consumers_failure_does_not_finish_a_sibling_or_suppress_its_answer() {
    for failure in [
        TurnOutcome::Failed {
            code: "provider_failed".into(),
            message: "private diagnostic".into(),
        },
        TurnOutcome::Incomplete {
            reason: "provider_incomplete".into(),
        },
    ] {
        let recording = recording();
        let (_, a) = target(&recording, Some(consumer("csm_00000000000a")));
        let (_, b) = target(&recording, Some(consumer("csm_00000000000b")));
        recording.outcome(&a, accepted()).await;
        recording.outcome(&b, accepted()).await;
        recording.outcome(&a, failure).await;
        recording.outcome(&b, completed("B completed")).await;
        let mut expected = vec![
            "B completed".to_owned(),
            DeliveryConfig::default().failure_text,
        ];
        expected.sort();
        assert_eq!(texts(&recording.finish().await), expected);
    }
}

#[tokio::test(start_paused = true)]
async fn mismatched_launch_consumer_or_conversation_never_reaches_delivery() {
    for (sealed, launched, wrong_conversation) in [
        (
            Some(consumer("csm_00000000000a")),
            Some(consumer("csm_00000000000b")),
            false,
        ),
        (None, Some(consumer("csm_00000000000a")), false),
        (Some(consumer("csm_00000000000a")), None, false),
        (
            Some(consumer("csm_00000000000a")),
            Some(consumer("csm_00000000000a")),
            true,
        ),
    ] {
        let recording = recording();
        let (_, mut forged) = target(&recording, sealed);
        let launch = Arc::get_mut(&mut forged).unwrap();
        launch.consumer = launched;
        if wrong_conversation {
            launch.conversation_id = "conv_00000000000000000000000000000002".parse().unwrap();
        }
        recording.outcome(&forged, completed("forged")).await;
        let (_, valid) = target(&recording, Some(consumer("csm_00000000000b")));
        recording.outcome(&valid, completed("valid")).await;
        assert_eq!(texts(&recording.finish().await), ["valid"]);
    }
}

#[tokio::test(start_paused = true)]
async fn explicit_default_launch_keeps_legacy_tool_reply_suppression() {
    let mut recording = recording();
    let (event, mut turn) = target(&recording, None);
    Arc::get_mut(&mut turn).unwrap().consumer = Some(ConsumerId::default_consumer());
    recording.outcome(&turn, accepted()).await;
    recording
        .reply(
            &turn.channel,
            &event,
            json!({"text":"default tool"}),
            ReplySource::Tool,
        )
        .await;
    recording.outcome(&turn, completed("suppressed")).await;
    assert_eq!(texts(&recording.finish().await), ["default tool"]);
}
