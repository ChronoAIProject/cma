//! Routed custom output identity, independent of chat labels and the current flag.
use super::{
    fake_http::FakeHttp,
    group_fixtures::{
        consumer_event, consumer_turn, reviewer, routed_custom_activity, routed_custom_channel,
        REVIEWER,
    },
    recording_support::{Hooks, Recording},
};
use cmaeg::{
    delivery::intents::{DeliveryCommand, DeliveryOutcome, ReplySource},
    turns::outcomes::TurnOutcome,
    DeliveryConfig,
};
use cmaeg_core::seal::{EventRefPayload, ReplyAddress};
use serde_json::{json, Value};
use std::{collections::BTreeSet, sync::Arc};

fn completed(text: &str) -> TurnOutcome {
    TurnOutcome::Completed {
        final_text: Some(text.into()),
    }
}
fn summary(id: &str, name: &str) -> Value {
    json!({"id":id,"name":name,"label":name})
}
fn recording(hooks: &Arc<Hooks>, enabled: bool) -> Recording {
    Recording::start(
        FakeHttp::platforms(),
        hooks.clone(),
        DeliveryConfig {
            multi_consumer_enabled: enabled,
            ..Default::default()
        },
    )
}
fn bodies(hooks: &Hooks) -> Vec<Value> {
    hooks
        .raw_bodies
        .lock()
        .unwrap()
        .iter()
        .map(|body| serde_json::from_str(body).unwrap())
        .collect()
}

#[tokio::test(start_paused = true)]
async fn tool_reply_and_each_consumer_terminal_have_distinct_tagged_envelopes() {
    let channel = routed_custom_channel();
    let activity = routed_custom_activity();
    let hooks = Hooks::answering([]);
    let mut recording = recording(&hooks, true);
    let event = |consumer| EventRefPayload {
        reply: ReplyAddress::None,
        ..consumer_event(&channel, &activity, consumer)
    };
    let (lead, reviewer) = (event(None), event(Some(reviewer())));
    let receipt = recording
        .reply(
            &channel,
            &reviewer,
            json!({"text":"Tool."}),
            ReplySource::Tool,
        )
        .await;
    assert!(matches!(
        receipt.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    for (event, text) in [(&lead, "Lead."), (&reviewer, "Review.")] {
        let turn = consumer_turn(&recording.keys, &channel, activity.clone(), event);
        recording.outcome(&turn, completed(text)).await;
    }
    recording.finish().await;
    let bodies = bodies(&hooks);
    assert_eq!(bodies.len(), 3);
    assert_eq!(
        bodies
            .iter()
            .map(|body| body["id"].as_str().unwrap())
            .collect::<BTreeSet<_>>()
            .len(),
        3
    );
    for (body, (kind, identity, text)) in bodies.iter().zip([
        ("message", summary(REVIEWER, "reviewer"), "Tool."),
        ("completed", summary("default", "lead"), "Lead."),
        ("completed", summary(REVIEWER, "reviewer"), "Review."),
    ]) {
        assert_eq!(body["type"], format!("cmaeg.response.{kind}"));
        assert_eq!(body["subject"], "msg-1");
        assert_eq!(body["data"]["event_id"], "msg-1");
        assert_eq!(body["data"]["consumer"], identity);
        assert_eq!(body["data"]["text"], text, "custom text has no chat prefix");
    }
}

#[tokio::test(start_paused = true)]
async fn automatic_tags_follow_the_admitted_activity_even_with_the_flag_off() {
    for enabled in [false, true] {
        for routed in [false, true] {
            let channel = routed_custom_channel();
            let mut activity = routed_custom_activity();
            if !routed {
                activity.routing = None;
            }
            let hooks = Hooks::answering([]);
            let mut recording = recording(&hooks, enabled);
            let event = EventRefPayload {
                reply: ReplyAddress::Sync {
                    waiter: recording.waiter(),
                },
                ..consumer_event(&channel, &activity, None)
            };
            let turn = consumer_turn(&recording.keys, &channel, activity, &event);
            recording
                .outcome(
                    &turn,
                    TurnOutcome::TextDelta {
                        text: "Delta.".into(),
                    },
                )
                .await;
            recording
                .outcome(
                    &turn,
                    TurnOutcome::MessageCommitted {
                        text: "Message.".into(),
                    },
                )
                .await;
            recording.outcome(&turn, completed("Final.")).await;
            let result = recording.finish().await;
            let frames = result["sync"].as_array().unwrap();
            assert_eq!(frames.len(), 3);
            assert_eq!(
                frames
                    .iter()
                    .map(|frame| frame["event"].as_str().unwrap())
                    .collect::<Vec<_>>(),
                ["cmaeg.text.delta", "cmaeg.message", "cmaeg.completed"]
            );
            let bodies = bodies(&hooks);
            assert_eq!(bodies.len(), 1);
            for data in frames
                .iter()
                .map(|frame| &frame["data"])
                .chain([&bodies[0]["data"]])
            {
                if routed {
                    assert_eq!(data["consumer"], summary("default", "lead"));
                } else {
                    assert!(
                        data.get("consumer").is_none(),
                        "unrouted output gains no identity"
                    );
                }
            }
            assert_eq!(frames[0]["data"]["text"], "Delta.");
            assert_eq!(frames[1]["data"]["text"], "Message.");
            assert_eq!(frames[2]["data"]["text"], "Final.");
        }
    }
}

#[tokio::test(start_paused = true)]
async fn refused_children_keep_identity_even_when_the_consumer_is_missing() {
    for removed in [false, true] {
        let mut channel = routed_custom_channel();
        if removed {
            let routing = Arc::get_mut(&mut channel)
                .unwrap()
                .definition
                .routing
                .as_mut()
                .unwrap();
            routing.consumers.clear();
            routing.default_targets.retain(|id| id.is_default());
        }
        let activity = routed_custom_activity();
        let hooks = Hooks::answering([]);
        let mut recording = recording(&hooks, true);
        let event = EventRefPayload {
            reply: ReplyAddress::Sync {
                waiter: recording.waiter(),
            },
            ..consumer_event(&channel, &activity, Some(reviewer()))
        };
        recording
            .submit(DeliveryCommand::ConsumerFailed {
                channel,
                payload: event,
                consumer: reviewer(),
                code: if removed {
                    "consumer_removed"
                } else {
                    "channel_capacity"
                },
                provider_code: None,
            })
            .await;
        let result = recording.finish().await;
        let frames = result["sync"].as_array().unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["event"], "cmaeg.failed");
        assert_eq!(frames[0]["data"]["refused"], true);
        let bodies = bodies(&hooks);
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0]["type"], "cmaeg.response.failed");
        let expected = summary(REVIEWER, if removed { REVIEWER } else { "reviewer" });
        assert_eq!(frames[0]["data"]["consumer"], expected);
        assert_eq!(bodies[0]["data"]["consumer"], expected);
    }
}

#[tokio::test(start_paused = true)]
async fn ordinary_runner_failures_are_tagged_without_becoming_refusals() {
    let channel = routed_custom_channel();
    let activity = routed_custom_activity();
    let hooks = Hooks::answering([]);
    let mut recording = recording(&hooks, true);
    let event = EventRefPayload {
        reply: ReplyAddress::Sync {
            waiter: recording.waiter(),
        },
        ..consumer_event(&channel, &activity, Some(reviewer()))
    };
    let turn = consumer_turn(&recording.keys, &channel, activity, &event);
    recording
        .outcome(
            &turn,
            TurnOutcome::Failed {
                code: "provider_failed".into(),
                message: "private diagnostic".into(),
            },
        )
        .await;
    let result = recording.finish().await;
    let frame = &result["sync"][0];
    assert_eq!(frame["event"], "cmaeg.failed");
    assert_eq!(frame["data"]["consumer"], summary(REVIEWER, "reviewer"));
    assert!(frame["data"].get("refused").is_none());
    assert_eq!(
        bodies(&hooks)[0]["data"]["consumer"],
        summary(REVIEWER, "reviewer")
    );
    assert!(!result.to_string().contains("private diagnostic"));
}
