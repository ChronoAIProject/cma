//! Per-consumer failures and routing notices on the real engine: a fan-out
//! consumer that failed before launch gets its own labeled notice or webhook
//! envelope, at most once per event, and a notice reaches the origin once.
use super::{
    fake_http::{Answer, FakeHttp},
    group_fixtures::{
        consumer_event, consumer_turn, reviewer, routed_activity, routed_channel,
        routed_custom_activity, routed_custom_channel,
    },
    recording_fixtures::{keys, relay_reply},
    recording_support::{Hooks, Recording},
};
use cmaeg::{
    channels::resolver::ResolvedChannel,
    delivery::intents::{DeliveryCommand, ReplySource},
    turns::outcomes::TurnOutcome,
    DeliveryConfig,
};
use cmaeg_core::{
    activity::Activity,
    seal::{EventRefPayload, ReplyAddress},
};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::oneshot;

const FAILURE: &str = "Sorry, I couldn’t finish that request. Please try again.";

fn recording(http: FakeHttp) -> Recording {
    let config = DeliveryConfig {
        multi_consumer_enabled: true,
        ..Default::default()
    };
    Recording::start(http, Hooks::answering([]), config)
}

fn failed() -> TurnOutcome {
    TurnOutcome::Failed {
        code: "provider_unavailable".into(),
        message: "provider stream ended before acceptance".into(),
    }
}

fn completed(text: &str) -> TurnOutcome {
    TurnOutcome::Completed {
        final_text: Some(text.into()),
    }
}

fn consumer_failed(channel: &Arc<ResolvedChannel>, payload: &EventRefPayload) -> DeliveryCommand {
    DeliveryCommand::ConsumerFailed {
        channel: channel.clone(),
        payload: payload.clone(),
        consumer: reviewer(),
        code: "provider_unavailable",
        provider_code: Some("upstream_busy".into()),
    }
}

/// Every chat text sent, in order.
fn texts(recorded: &Value) -> Vec<String> {
    recorded["calls"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|call| call["transport"] == "relay")
        .map(|call| call["body"]["reply"]["text"].as_str().unwrap().to_owned())
        .collect()
}

/// Every result webhook envelope's type, in order.
fn envelopes(recorded: &Value) -> Vec<String> {
    recorded["calls"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|call| call["transport"] == "webhook")
        .map(|call| call["body"]["type"].as_str().unwrap().to_owned())
        .collect()
}

struct Event {
    channel: Arc<ResolvedChannel>,
    activity: Activity,
    lead: EventRefPayload,
    reviewer: EventRefPayload,
}

fn relay_event() -> Event {
    let channel = routed_channel(0, |_| {});
    let activity = routed_activity();
    Event {
        lead: consumer_event(&channel, &activity, None),
        reviewer: consumer_event(&channel, &activity, Some(reviewer())),
        channel,
        activity,
    }
}

impl Event {
    fn turn(
        &self,
        recording: &Recording,
        event: &EventRefPayload,
    ) -> Arc<cmaeg::ingress::launch::LaunchRequest> {
        consumer_turn(&recording.keys, &self.channel, self.activity.clone(), event)
    }
}

#[tokio::test(start_paused = true)]
async fn a_failure_without_any_outcome_creates_its_group_and_a_labeled_notice() {
    let recording = recording(FakeHttp::platforms());
    let event = relay_event();
    recording
        .submit(consumer_failed(&event.channel, &event.reviewer))
        .await;
    let lead = event.turn(&recording, &event.lead);
    recording.outcome(&lead, completed("Lead.")).await;
    assert_eq!(
        texts(&recording.finish().await),
        [format!("[reviewer] {FAILURE}"), "[lead] Lead.".into()]
    );
}

#[tokio::test(start_paused = true)]
async fn a_failure_joins_an_existing_group_without_disturbing_the_others() {
    let recording = recording(FakeHttp::platforms());
    let event = relay_event();
    let lead = event.turn(&recording, &event.lead);
    recording
        .outcome(
            &lead,
            TurnOutcome::MessageCommitted {
                text: "Lead.".into(),
            },
        )
        .await;
    recording
        .submit(consumer_failed(&event.channel, &event.reviewer))
        .await;
    recording.outcome(&lead, completed("Lead.")).await;
    assert_eq!(
        texts(&recording.finish().await),
        ["[lead] Lead.".to_owned(), format!("[reviewer] {FAILURE}")]
    );
}

/// The runner already reported the refused child's own terminal outcome, so
/// the finalizer's failure for it changes nothing, in either order.
#[tokio::test(start_paused = true)]
async fn a_consumer_is_terminal_at_most_once_per_chat_event() {
    for runner_first in [true, false] {
        let recording = recording(FakeHttp::platforms());
        let event = relay_event();
        let refused = event.turn(&recording, &event.reviewer);
        if runner_first {
            recording.outcome(&refused, failed()).await;
        }
        recording
            .submit(consumer_failed(&event.channel, &event.reviewer))
            .await;
        if !runner_first {
            recording.outcome(&refused, failed()).await;
        }
        let lead = event.turn(&recording, &event.lead);
        recording.outcome(&lead, completed("Lead.")).await;
        assert_eq!(
            texts(&recording.finish().await),
            [format!("[reviewer] {FAILURE}"), "[lead] Lead.".into()],
            "runner first: {runner_first}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_custom_consumer_gets_exactly_one_failed_envelope() {
    for runner_first in [true, false] {
        let recording = recording(FakeHttp::platforms());
        let channel = routed_custom_channel();
        let activity = routed_custom_activity();
        let unanswered = |consumer| EventRefPayload {
            reply: ReplyAddress::None,
            ..consumer_event(&channel, &activity, consumer)
        };
        let (lead, review) = (unanswered(None), unanswered(Some(reviewer())));
        let refused = consumer_turn(&recording.keys, &channel, activity.clone(), &review);
        if runner_first {
            recording.outcome(&refused, failed()).await;
        }
        recording.submit(consumer_failed(&channel, &review)).await;
        if !runner_first {
            recording.outcome(&refused, failed()).await;
        }
        let lead = consumer_turn(&recording.keys, &channel, activity.clone(), &lead);
        recording.outcome(&lead, completed("Lead.")).await;
        assert_eq!(
            envelopes(&recording.finish().await),
            ["cmaeg.response.failed", "cmaeg.response.completed"],
            "runner first: {runner_first}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_notice_is_sent_once_to_the_origin_and_its_failure_is_dropped() {
    let http = FakeHttp::platforms().script([Answer::status(500)]);
    let recording = recording(http);
    let event = relay_event();
    for text in ["Nobody here answers that.", "Nor that."] {
        recording
            .submit(DeliveryCommand::Notice {
                trace: tracing::Span::none(),
                channel: event.channel.clone(),
                address: relay_reply(),
                event_id: event.lead.event_id.clone(),
                text: text.into(),
            })
            .await;
    }
    let recorded = recording.finish().await;
    let calls: Vec<_> = recorded["calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|call| {
            (
                call["credential"].as_str().unwrap().to_owned(),
                call["body"]["reply"]["text"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    // The first fails once and is not retried; the second goes with the
    // channel key, since the reply token was presented once.
    assert_eq!(
        calls,
        [
            ("reply_token".into(), "Nobody here answers that.".into()),
            ("channel_key".into(), "Nor that.".to_owned()),
        ]
    );
}

#[test]
fn every_command_of_an_event_names_the_same_event_key() {
    let event = relay_event();
    let turn = consumer_turn(
        &keys(),
        &event.channel,
        event.activity.clone(),
        &event.reviewer,
    );
    let (receipt, _) = oneshot::channel();
    let commands = [
        DeliveryCommand::StreamOutcome {
            turn,
            outcome: completed("Done."),
        },
        DeliveryCommand::Reply {
            trace: tracing::Span::none(),
            channel: event.channel.clone(),
            event: event.reviewer.clone(),
            message: serde_json::from_value(json!({"text":"Reply."})).unwrap(),
            source: ReplySource::Tool,
            receipt,
        },
        consumer_failed(&event.channel, &event.reviewer),
        DeliveryCommand::Notice {
            trace: tracing::Span::none(),
            channel: event.channel.clone(),
            address: relay_reply(),
            event_id: event.lead.event_id.clone(),
            text: "Notice.".into(),
        },
    ];
    let expected = (&event.channel.id, &event.lead.event_id);
    for command in &commands {
        assert_eq!(command.event_key(), expected, "{command:?}");
    }
}
