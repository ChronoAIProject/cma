//! Per-consumer delivery groups on the real engine, under the paused clock:
//! two consumers of one relay event share its ack and heartbeats, and each
//! gets its own answer, auto-final suppression and failure text.
use super::{
    fake_http::FakeHttp,
    group_fixtures::{
        consumer_event, consumer_turn, handoff, reviewer, routed_activity, routed_channel,
    },
    recording_support::{Hooks, Recording},
};
use cmaeg::{
    delivery::intents::{DeliveryCommand, ReplySource},
    ingress::launch::LaunchRequest,
    turns::outcomes::TurnOutcome,
    DeliveryConfig,
};
use cmaeg_core::seal::EventRefPayload;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use tokio::sync::oneshot;

const ACK: &str = "Working on it…";
const HEARTBEAT: &str = "Still working…";
const FAILURE: &str = "Sorry, I couldn’t finish that request. Please try again.";

/// One relay event's two consumers: their events and turns.
struct Event {
    lead: (EventRefPayload, Arc<LaunchRequest>),
    reviewer: (EventRefPayload, Arc<LaunchRequest>),
}

fn start(ack_after_ms: u32, config: DeliveryConfig) -> (Recording, Event) {
    let recording = Recording::start(FakeHttp::platforms(), Hooks::answering([]), config);
    let channel = routed_channel(ack_after_ms, |_| {});
    let activity = routed_activity();
    let turn = |consumer| {
        let event = consumer_event(&channel, &activity, consumer);
        let turn = consumer_turn(&recording.keys, &channel, activity.clone(), &event);
        (event, turn)
    };
    let event = Event {
        lead: turn(None),
        reviewer: turn(Some(reviewer())),
    };
    (recording, event)
}

fn accepted() -> TurnOutcome {
    TurnOutcome::Accepted {
        response_id: "response-1".into(),
    }
}
fn committed(text: &str) -> TurnOutcome {
    TurnOutcome::MessageCommitted { text: text.into() }
}
fn completed(text: &str) -> TurnOutcome {
    TurnOutcome::Completed {
        final_text: Some(text.into()),
    }
}
fn failed() -> TurnOutcome {
    TurnOutcome::Failed {
        code: "provider_failed".into(),
        message: "provider detail".into(),
    }
}

/// Every chat message sent: its offset, credential class and text.
fn chat(recorded: &Value) -> Vec<(u64, String, String)> {
    recorded["calls"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|call| call["transport"] == "relay")
        .map(|call| {
            (
                call["at_ms"].as_u64().unwrap(),
                call["credential"].as_str().unwrap().to_owned(),
                call["body"]["reply"]["text"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn sent(expected: &[(u64, &str, &str)]) -> Vec<(u64, String, String)> {
    expected
        .iter()
        .map(|(at, credential, text)| (*at, (*credential).to_owned(), (*text).to_owned()))
        .collect()
}

#[tokio::test(start_paused = true)]
async fn two_consumers_share_one_ack_and_each_gets_its_answer() {
    let (recording, event) = start(1500, Default::default());
    recording.outcome(&event.lead.1, accepted()).await;
    recording.outcome(&event.reviewer.1, accepted()).await;
    recording.at(4000).await;
    recording.outcome(&event.lead.1, committed("Lead.")).await;
    recording.outcome(&event.lead.1, completed("Lead.")).await;
    recording
        .outcome(&event.reviewer.1, committed("Review."))
        .await;
    recording
        .outcome(&event.reviewer.1, completed("Review."))
        .await;
    let recorded = recording.finish().await;
    assert_eq!(
        chat(&recorded),
        sent(&[
            (1500, "reply_token", ACK),
            (4500, "channel_key", "Lead."),
            (7500, "channel_key", "Review."),
        ])
    );
}

#[tokio::test(start_paused = true)]
async fn heartbeats_continue_while_any_consumer_is_unanswered() {
    let config = DeliveryConfig {
        heartbeat: Duration::from_secs(10),
        ..Default::default()
    };
    let (recording, event) = start(1000, config);
    recording.outcome(&event.lead.1, accepted()).await;
    recording.outcome(&event.reviewer.1, accepted()).await;
    recording.at(5000).await;
    recording.outcome(&event.lead.1, completed("Lead.")).await;
    recording.at(70_000).await;
    recording
        .outcome(&event.reviewer.1, completed("Review."))
        .await;
    let recorded = recording.finish().await;
    assert_eq!(
        chat(&recorded),
        sent(&[
            (1000, "reply_token", ACK),
            (5000, "channel_key", "Lead."),
            (11_000, "channel_key", HEARTBEAT),
            (21_000, "channel_key", HEARTBEAT),
            (31_000, "channel_key", HEARTBEAT),
            (41_000, "channel_key", HEARTBEAT),
            (51_000, "channel_key", HEARTBEAT),
            (70_000, "channel_key", "Review."),
        ])
    );
}

#[tokio::test(start_paused = true)]
async fn one_consumer_finishing_never_swallows_the_others_answer() {
    let (recording, event) = start(0, Default::default());
    recording.outcome(&event.lead.1, accepted()).await;
    recording.outcome(&event.reviewer.1, accepted()).await;
    recording.outcome(&event.lead.1, completed("Lead.")).await;
    // A late outcome of the finished consumer is ignored for it alone.
    recording.outcome(&event.lead.1, failed()).await;
    recording
        .outcome(&event.reviewer.1, committed("Review."))
        .await;
    recording
        .outcome(&event.reviewer.1, completed("Review."))
        .await;
    let recorded = recording.finish().await;
    assert_eq!(
        chat(&recorded),
        sent(&[
            (0, "reply_token", "Lead."),
            (3000, "channel_key", "Review."),
        ])
    );
}

#[tokio::test(start_paused = true)]
async fn a_tool_reply_suppresses_only_its_own_consumers_auto_final() {
    let (mut recording, event) = start(0, Default::default());
    recording.outcome(&event.lead.1, accepted()).await;
    recording.outcome(&event.reviewer.1, accepted()).await;
    let channel = event.lead.1.channel.clone();
    recording
        .reply(
            &channel,
            &event.lead.0,
            json!({"text":"From the tool."}),
            ReplySource::Tool,
        )
        .await;
    recording
        .outcome(&event.lead.1, committed("Automatic."))
        .await;
    recording
        .outcome(&event.lead.1, completed("Automatic."))
        .await;
    recording
        .outcome(&event.reviewer.1, completed("Review."))
        .await;
    let recorded = recording.finish().await;
    assert_eq!(
        chat(&recorded),
        sent(&[
            (0, "reply_token", "From the tool."),
            (3000, "channel_key", "Review."),
        ])
    );
}

/// The reply is queued before its consumer's `Completed` and its handler
/// records no tool-reply marker, so only the worker's FIFO order on the
/// consumer's entry can suppress the auto-final.
#[tokio::test(start_paused = true)]
async fn a_reply_queued_before_completed_suppresses_that_auto_final() {
    let (recording, event) = start(0, Default::default());
    recording.outcome(&event.lead.1, accepted()).await;
    recording.outcome(&event.reviewer.1, accepted()).await;
    let (receipt, received) = oneshot::channel();
    recording
        .submit(DeliveryCommand::Reply {
            trace: tracing::Span::none(),
            channel: event.lead.1.channel.clone(),
            event: event.lead.0.clone(),
            message: serde_json::from_value(json!({"text":"From the tool."})).unwrap(),
            source: ReplySource::Tool,
            receipt,
        })
        .await;
    recording
        .outcome(&event.lead.1, completed("Automatic."))
        .await;
    recording
        .outcome(&event.reviewer.1, completed("Review."))
        .await;
    drop(received.await.unwrap());
    let recorded = recording.finish().await;
    assert_eq!(
        chat(&recorded),
        sent(&[
            (0, "reply_token", "From the tool."),
            (3000, "channel_key", "Review."),
        ])
    );
}

#[tokio::test(start_paused = true)]
async fn a_failed_consumer_gets_its_failure_text_while_the_other_continues() {
    let (recording, event) = start(0, Default::default());
    recording.outcome(&event.lead.1, accepted()).await;
    recording.outcome(&event.reviewer.1, accepted()).await;
    recording.outcome(&event.reviewer.1, failed()).await;
    recording.outcome(&event.lead.1, committed("Lead.")).await;
    recording.outcome(&event.lead.1, completed("Lead.")).await;
    let recorded = recording.finish().await;
    assert_eq!(
        chat(&recorded),
        sent(&[(0, "reply_token", FAILURE), (3000, "channel_key", "Lead."),])
    );
}

/// After its last consumer finishes, a group ignores that consumer's late
/// outcomes for 30 s, then expires: the same outcome later starts a new group,
/// which acknowledges again.
#[tokio::test(start_paused = true)]
async fn a_group_expires_30_seconds_after_its_last_consumer_finishes() {
    let (recording, event) = start(1000, Default::default());
    recording.outcome(&event.lead.1, accepted()).await;
    recording.outcome(&event.reviewer.1, accepted()).await;
    recording.outcome(&event.lead.1, completed("Lead.")).await;
    recording
        .outcome(&event.reviewer.1, completed("Review."))
        .await;
    // The reviewer's answer is paced to 3 s, when the group becomes terminal.
    recording.at(32_000).await;
    recording.outcome(&event.reviewer.1, accepted()).await;
    recording.at(34_100).await;
    recording.outcome(&event.reviewer.1, accepted()).await;
    recording.at(36_000).await;
    recording
        .outcome(&event.reviewer.1, completed("Again."))
        .await;
    let recorded = recording.finish().await;
    assert_eq!(
        chat(&recorded),
        sent(&[
            (0, "reply_token", "Lead."),
            (3000, "channel_key", "Review."),
            (35_100, "channel_key", ACK),
            (38_100, "channel_key", "Again."),
        ])
    );
}

#[tokio::test(start_paused = true)]
async fn a_handoff_group_sends_no_ack_and_no_heartbeat() {
    let config = DeliveryConfig {
        heartbeat: Duration::from_secs(10),
        ..Default::default()
    };
    let recording = Recording::start(FakeHttp::platforms(), Hooks::answering([]), config);
    let channel = routed_channel(1000, |_| {});
    let activity = routed_activity();
    let event = handoff(consumer_event(&channel, &activity, Some(reviewer())));
    let turn = consumer_turn(&recording.keys, &channel, activity, &event);
    recording.outcome(&turn, accepted()).await;
    recording.at(70_000).await;
    recording.outcome(&turn, completed("Handed off.")).await;
    let recorded = recording.finish().await;
    assert_eq!(
        chat(&recorded),
        sent(&[(70_000, "reply_token", "Handed off.")])
    );
}
