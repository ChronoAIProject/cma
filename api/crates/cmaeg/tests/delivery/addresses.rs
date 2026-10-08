//! Routed sends and effective addresses on the real engine: an output for
//! the unavailable default destination is refused visibly, never redirected;
//! unrouted sends keep today's origin; hand-off cards are refused.
use super::{
    fake_http::FakeHttp,
    group_fixtures::{
        consumer_event, consumer_turn, handoff, reviewer, routed_activity, routed_channel,
    },
    recording_fixtures::BOT_TOKEN,
    recording_support::{Hooks, Recording},
};
use cmaeg::{
    channels::resolver::ResolvedChannel,
    delivery::intents::{DeliveryOutcome, ReplySource},
    turns::outcomes::TurnOutcome,
    DeliveryConfig,
};
use cmaeg_core::{
    activity::Activity,
    channel::ReplyChoice,
    seal::{EventRefPayload, ReplyAddress},
};
use serde_json::{json, Value};
use std::{
    io::Write,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);
impl Write for Logs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl Logs {
    /// Capture this thread's logs, which include the current-thread engine's.
    fn capture(&self) -> tracing::subscriber::DefaultGuard {
        let writer = self.clone();
        tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_ansi(false)
                .without_time()
                .with_writer(move || writer.clone())
                .finish(),
        )
    }
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

fn recording(multi_consumer_enabled: bool) -> Recording {
    let config = DeliveryConfig {
        multi_consumer_enabled,
        ..Default::default()
    };
    Recording::start(FakeHttp::platforms(), Hooks::answering([]), config)
}

fn completed(text: &str) -> TurnOutcome {
    TurnOutcome::Completed {
        final_text: Some(text.into()),
    }
}

fn texts(recorded: &Value) -> Vec<String> {
    recorded["calls"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|call| match call["transport"].as_str() {
            Some("relay") => call["body"]["reply"]["text"].as_str(),
            Some("telegram") => call["body"]["text"].as_str(),
            _ => None,
        })
        .map(str::to_owned)
        .collect()
}

/// Run `consumer`'s turn of `activity` on `channel` to completion.
async fn answer(
    recording: &Recording,
    channel: &Arc<ResolvedChannel>,
    activity: &Activity,
    event: &EventRefPayload,
    text: &str,
) {
    let turn = consumer_turn(&recording.keys, channel, activity.clone(), event);
    recording.outcome(&turn, completed(text)).await;
}

#[tokio::test(start_paused = true)]
async fn an_automatic_output_for_the_default_destination_is_rejected_visibly() {
    let logs = Logs::default();
    let _capture = logs.capture();
    let channel = routed_channel(0, |value| {
        value["routing"]["consumers"][0]["reply"] = json!({"destination": "default"});
    });
    let activity = routed_activity();
    let recording = recording(true);
    let lead = consumer_event(&channel, &activity, None);
    let review = consumer_event(&channel, &activity, Some(reviewer()));
    let review_turn = consumer_turn(&recording.keys, &channel, activity.clone(), &review);
    recording
        .outcome(
            &review_turn,
            TurnOutcome::MessageCommitted {
                text: "Private.".into(),
            },
        )
        .await;
    // Rejected, the reviewer is terminal: its failure is ignored, not sent.
    recording
        .outcome(
            &review_turn,
            TurnOutcome::Failed {
                code: "provider_failed".into(),
                message: "detail".into(),
            },
        )
        .await;
    answer(&recording, &channel, &activity, &lead, "Lead.").await;
    assert_eq!(texts(&recording.finish().await), ["[lead] Lead."]);
    let logs = logs.text();
    let rejected: Vec<_> = logs
        .lines()
        .filter(|line| line.contains("automatic output rejected"))
        .collect();
    assert_eq!(rejected.len(), 1, "{logs}");
    assert!(rejected[0].contains("WARN"), "{logs}");
    assert!(rejected[0].contains("default_reply_unavailable"), "{logs}");
    assert!(rejected[0].contains("consumer=csm_00000000000a"), "{logs}");
    assert!(!logs.contains("Private."), "{logs}");
}

#[tokio::test(start_paused = true)]
async fn unrouted_sends_keep_todays_origin_whatever_the_destination() {
    let channel = routed_channel(0, |value| {
        value["routing"]["default_consumer"]["reply"] = json!({"destination": "default"});
    });
    let activity = routed_activity();
    let lead = consumer_event(&channel, &activity, None);
    // With the flag off, a default reference on a routed channel is today's.
    let mut off = recording(false);
    answer(&off, &channel, &activity, &lead, "Lead.").await;
    let receipt = off
        .reply(&channel, &lead, json!({"text":"Tool."}), ReplySource::Tool)
        .await;
    assert!(matches!(
        receipt.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    assert_eq!(texts(&off.finish().await), ["Lead.", "Tool."]);
    // A turn admitted unrouted keeps its origin after its channel is routed.
    let mut unrouted = activity.clone();
    unrouted.routing = None;
    let on = recording(true);
    answer(&on, &channel, &unrouted, &lead, "Lead.").await;
    assert_eq!(texts(&on.finish().await), ["Lead."]);
}

#[tokio::test(start_paused = true)]
async fn a_reply_target_used_after_its_group_expired_classes_alike() {
    let channel = routed_channel(0, |_| {});
    let activity = routed_activity();
    let lead = consumer_event(&channel, &activity, None);
    let review = consumer_event(&channel, &activity, Some(reviewer()));
    let mut deferred = handoff(review.clone());
    deferred.lineage.as_mut().unwrap().reply = ReplyChoice::Default;
    let mut recording = recording(false);
    answer(&recording, &channel, &activity, &lead, "Lead.").await;
    answer(&recording, &channel, &activity, &review, "Review.").await;
    recording.at(40_000).await;
    let mut receipts = Vec::new();
    for event in [&review, &lead, &deferred] {
        let receipt = recording
            .reply(&channel, event, json!({"text":"Later."}), ReplySource::Tool)
            .await;
        receipts.push(receipt.delivery);
    }
    assert!(matches!(receipts[0], DeliveryOutcome::Confirmed { .. }));
    assert!(matches!(receipts[1], DeliveryOutcome::Confirmed { .. }));
    assert_eq!(
        receipts[2],
        DeliveryOutcome::Rejected {
            reason: "default_reply_unavailable".into()
        }
    );
    assert_eq!(
        texts(&recording.finish().await),
        ["Lead.", "[reviewer] Review.", "[reviewer] Later.", "Later."]
    );
}

#[tokio::test(start_paused = true)]
async fn handoff_controls_are_refused_on_both_telegram_transports() {
    let relay = routed_channel(0, |_| {});
    let native = routed_channel(0, |value| {
        value["sources"] = json!([{"type":"telegram","bot_id":"77","bot_username":"fixture_bot","admission":{"type":"all"}}]);
        value["secrets"]["telegram_bot_token"] = BOT_TOKEN.into();
    });
    let activity = routed_activity();
    let relayed = handoff(consumer_event(&relay, &activity, Some(reviewer())));
    let mut natively = handoff(consumer_event(&native, &activity, Some(reviewer())));
    natively.reply = ReplyAddress::Telegram {
        bot_id: "77".into(),
        chat_id: "42".into(),
        message_id: "5".into(),
        thread_id: None,
    };
    let buttons = json!({"text":"Choose","blocks":[{"type":"buttons","id":"choice","options":[{"id":"first","label":"First"},{"id":"second","label":"Second"}]}]});
    let mut recording = recording(true);
    for (channel, event) in [(&relay, &relayed), (&native, &natively)] {
        let refused = recording
            .reply(channel, event, buttons.clone(), ReplySource::Tool)
            .await;
        assert_eq!(
            refused.delivery,
            DeliveryOutcome::Rejected {
                reason: "interactive_destination_unsupported".into()
            }
        );
        assert!(refused.message_ref.is_none());
        let text = recording
            .reply(channel, event, json!({"text":"Plain."}), ReplySource::Tool)
            .await;
        assert!(matches!(text.delivery, DeliveryOutcome::Confirmed { .. }));
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    assert_eq!(
        texts(&recording.finish().await),
        ["[reviewer] Plain.", "[reviewer] Plain."]
    );
}
