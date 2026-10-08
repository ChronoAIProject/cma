//! Consumer labels on the real engine: which sends carry them, on both
//! Telegram transports, and that an edited message keeps its author's.
use super::{
    fake_http::FakeHttp,
    group_fixtures::{consumer_event, consumer_turn, reviewer, routed_activity, routed_channel},
    recording_fixtures::{activity, BOT_TOKEN},
    recording_support::{Hooks, Recording},
};
use cmaeg::{
    channels::resolver::ResolvedChannel, delivery::intents::DeliveryOutcome,
    turns::outcomes::TurnOutcome, DeliveryConfig,
};
use cmaeg_core::{
    activity::{Activity, SourceKind},
    channel::{Platform, SourceId},
    seal::{MessageRefPayload, ReplyAddress},
};
use serde_json::{json, Value};
use std::sync::Arc;

fn config(multi_consumer_enabled: bool) -> DeliveryConfig {
    DeliveryConfig {
        multi_consumer_enabled,
        ..Default::default()
    }
}

fn completed(text: &str) -> TurnOutcome {
    TurnOutcome::Completed {
        final_text: Some(text.into()),
    }
}

/// Every chat text sent, in order, with its transport and endpoint.
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

/// Run `lead` and `reviewer` turns of `activity` on `channel` to completion.
async fn answers(
    channel: Arc<ResolvedChannel>,
    activity: Activity,
    multi_consumer_enabled: bool,
) -> Vec<String> {
    let recording = Recording::start(
        FakeHttp::platforms(),
        Hooks::answering([]),
        config(multi_consumer_enabled),
    );
    for consumer in [None, Some(reviewer())] {
        let event = consumer_event(&channel, &activity, consumer);
        let turn = consumer_turn(&recording.keys, &channel, activity.clone(), &event);
        recording.outcome(&turn, completed("Done.")).await;
    }
    texts(&recording.finish().await)
}

#[tokio::test(start_paused = true)]
async fn with_the_flag_off_only_bound_references_are_labeled() {
    // A `default` reference carries no marker, so with the flag off its
    // send is exactly today's; the reviewer's reference is bound to it.
    assert_eq!(
        answers(routed_channel(0, |_| {}), routed_activity(), false).await,
        ["Done.", "[reviewer] Done."]
    );
    assert_eq!(
        answers(routed_channel(0, |_| {}), routed_activity(), true).await,
        ["[lead] Done.", "[reviewer] Done."]
    );
}

#[tokio::test(start_paused = true)]
async fn label_modes_and_unrouted_turns() {
    let never = routed_channel(0, |value| value["routing"]["reply_label"] = "never".into());
    assert_eq!(
        answers(never, routed_activity(), true).await,
        ["Done.", "Done."]
    );
    // One non-pending consumer: `auto` labels nobody, `always` labels it.
    let pending = |mode: &'static str| {
        routed_channel(0, move |value| {
            value["routing"]["reply_label"] = mode.into();
            value["routing"]["consumers"][0]["pending"] = true.into();
        })
    };
    assert_eq!(
        answers(pending("auto"), routed_activity(), true).await,
        ["Done.", "Done."]
    );
    assert_eq!(
        answers(pending("always"), routed_activity(), true).await,
        ["[lead] Done.", "[reviewer] Done."]
    );
    // A turn admitted unrouted keeps today's outputs on a routed channel.
    let mut unrouted = routed_activity();
    unrouted.routing = None;
    assert_eq!(
        answers(routed_channel(0, |_| {}), unrouted, true).await,
        ["Done.", "Done."]
    );
}

/// The routed relay channel moved to native Telegram, with the reviewer
/// labeled `a_b*`.
fn native_channel() -> Arc<ResolvedChannel> {
    routed_channel(0, |value| {
        value["sources"] = json!([{"type":"telegram","bot_id":"77","bot_username":"fixture_bot","admission":{"type":"all"}}]);
        value["secrets"]["telegram_bot_token"] = BOT_TOKEN.into();
        value["routing"]["consumers"][0]["label"] = "a_b*".into();
    })
}

fn native_activity() -> Activity {
    let mut native = activity(
        SourceKind::Telegram {
            platform: Platform::Telegram,
            bot_id: "77".into(),
        },
        "msg-1",
    );
    let mut routing = routed_activity().routing.unwrap();
    routing.source_id = SourceId::parse("telegram").unwrap();
    native.routing = Some(routing);
    native
}

fn message_ref(consumer: bool) -> MessageRefPayload {
    MessageRefPayload {
        channel_id: native_channel().id.clone(),
        event_id: "msg-1".parse().unwrap(),
        platform_message_id: "1".into(),
        route_id: "telegram:77:42".into(),
        interaction_record_id: None,
        consumer: consumer.then(reviewer),
    }
}

#[tokio::test(start_paused = true)]
async fn native_replies_and_edits_keep_the_authors_label() {
    let channel = native_channel();
    let activity = native_activity();
    let mut recording =
        Recording::start(FakeHttp::platforms(), Hooks::answering([]), config(false));
    let mut event = consumer_event(&channel, &activity, Some(reviewer()));
    event.reply = ReplyAddress::Telegram {
        bot_id: "77".into(),
        chat_id: "42".into(),
        message_id: "5".into(),
        thread_id: None,
    };
    let turn = consumer_turn(&recording.keys, &channel, activity, &event);
    recording.outcome(&turn, completed("Answer.")).await;
    let edited = recording
        .update(&channel, message_ref(true), json!({"text":"Edited."}))
        .await;
    assert!(matches!(edited.delivery, DeliveryOutcome::Confirmed { .. }));
    // The label is applied before the one-part check: a full-length edit
    // fits only without it.
    let full = json!({ "text": "a".repeat(4000) });
    let refused = recording
        .update(&channel, message_ref(true), full.clone())
        .await;
    assert_eq!(
        refused.delivery,
        DeliveryOutcome::Rejected {
            reason: "interactive_message_must_fit_one_part".into()
        }
    );
    let unlabeled = recording.update(&channel, message_ref(false), full).await;
    assert!(matches!(
        unlabeled.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    let texts = texts(&recording.finish().await);
    assert_eq!(
        texts[..2],
        [
            "[a\\_b\\*] Answer.".to_owned(),
            "[a\\_b\\*] Edited.".to_owned()
        ]
    );
    assert_eq!(
        texts[2],
        "a".repeat(4000),
        "the default's edit stays unlabeled"
    );
    assert_eq!(texts.len(), 3);
}

#[tokio::test(start_paused = true)]
async fn relay_replies_carry_the_same_prefix_bytes() {
    let channel = routed_channel(0, |value| {
        value["routing"]["consumers"][0]["label"] = "a_b*".into();
    });
    let activity = routed_activity();
    let recording = Recording::start(FakeHttp::platforms(), Hooks::answering([]), config(false));
    let event = consumer_event(&channel, &activity, Some(reviewer()));
    let turn = consumer_turn(&recording.keys, &channel, activity, &event);
    recording.outcome(&turn, completed("Answer.")).await;
    assert_eq!(texts(&recording.finish().await), ["[a\\_b\\*] Answer."]);
}
