//! Default reply destinations on the real engine, under the paused clock:
//! each transport, the refusal of controls, the fallback of an origin that
//! cannot take replies, and today's sends wherever `default_ok` is false.
use super::{
    default_fixtures::{
        completed, custom_event, default_channel, recording, relay_default, run, sinks_default,
        telegram_default, telegram_messages, tool, with_sinks, CHAT,
    },
    default_relay::ROUTE,
    fake_http::FakeHttp,
    group_fixtures::reviewer,
};
use cmaeg::{delivery::intents::DeliveryOutcome, turns::outcomes::TurnOutcome};
use cmaeg_core::{
    channel::ReplyChoice,
    seal::{Lineage, ReplyAddress},
};
use serde_json::{json, Value};
use std::time::Duration;

fn rejected(reason: &str) -> DeliveryOutcome {
    DeliveryOutcome::Rejected {
        reason: reason.into(),
    }
}

fn confirmed(delivery: &DeliveryOutcome) -> bool {
    matches!(delivery, DeliveryOutcome::Confirmed { .. })
}

fn webhooks(recorded: &Value) -> Vec<Value> {
    recorded["calls"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|call| call["transport"] == "webhook")
        .map(|call| call["body"].clone())
        .collect()
}

#[tokio::test(start_paused = true)]
async fn a_custom_event_whose_default_is_a_telegram_chat_reaches_it_labeled() {
    let channel = default_channel(telegram_default(), |_| {});
    let recording = recording(FakeHttp::platforms(), true);
    let http = recording.http.clone();
    let event = custom_event(&channel, None, ReplyAddress::None);
    run(
        &recording,
        &channel,
        &event,
        [TurnOutcome::Accepted {
            response_id: "response-1".into(),
        }],
    )
    .await;
    // A custom origin never had an ack, and its default chat gets none.
    recording.at(5_000).await;
    run(&recording, &channel, &event, [completed("Done.")]).await;
    recording.finish().await;
    assert_eq!(
        telegram_messages(&http.sent()),
        [(CHAT.into(), Some("7".into()), "[lead] Done.".into())]
    );
}

#[tokio::test(start_paused = true)]
async fn a_relay_default_sends_through_the_proactive_send() {
    let channel = default_channel(relay_default(), |_| {});
    let mut recording = recording(FakeHttp::platforms(), true);
    let http = recording.http.clone();
    let event = custom_event(&channel, Some(reviewer()), ReplyAddress::None);
    let receipt = tool(&mut recording, &channel, &event, json!({"text":"Hello."})).await;
    assert!(confirmed(&receipt.delivery), "{:?}", receipt.delivery);
    assert!(receipt.message_ref.is_some());
    recording.finish().await;
    let sent = http.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].url.path(), "/api/v1/channel-relay/send");
    let body = sent[0].json();
    assert_eq!(body["conversation_id"], ROUTE);
    assert_eq!(body["message"]["text"], "[reviewer] Hello.");
    assert!(body["idempotency_key"]
        .as_str()
        .unwrap()
        .starts_with("tool_"));
}

#[tokio::test(start_paused = true)]
async fn a_sinks_default_posts_webhooks_tagged_as_the_default() {
    let channel = default_channel(sinks_default(), |value| {
        with_sinks(value);
        value["routing"]["consumers"][0]["reply"] = json!({"destination": "default"});
    });
    let mut recording = recording(FakeHttp::platforms(), true);
    // A Sync origin whose waiter is gone cannot take the tool's reply.
    let gone = ReplyAddress::Sync {
        waiter: "gone".into(),
    };
    let lead = custom_event(&channel, None, gone);
    let receipt = tool(&mut recording, &channel, &lead, json!({"text":"Tool."})).await;
    assert!(confirmed(&receipt.delivery), "{:?}", receipt.delivery);
    // The reviewer's own destination is the default.
    let review = custom_event(&channel, Some(reviewer()), ReplyAddress::None);
    run(&recording, &channel, &review, [completed("Review.")]).await;
    let hooks = webhooks(&recording.finish().await);
    assert_eq!(hooks.len(), 2, "{hooks:?}");
    assert_eq!(hooks[0]["type"], "cmaeg.response.message");
    assert_eq!(hooks[0]["data"]["text"], "Tool.");
    assert_eq!(hooks[1]["type"], "cmaeg.response.completed");
    assert_eq!(hooks[1]["data"]["text"], "Review.");
    for hook in &hooks {
        assert_eq!(hook["data"]["destination"], "default");
        assert_eq!(hook["subject"], "msg-1");
    }
}

#[tokio::test(start_paused = true)]
async fn controls_to_a_default_destination_are_refused() {
    let buttons = json!({"text":"Choose","blocks":[{"type":"buttons","id":"choice",
        "options":[{"id":"a","label":"A"},{"id":"b","label":"B"}]}]});
    for (default, sinks) in [
        (telegram_default(), false),
        (relay_default(), false),
        (sinks_default(), true),
    ] {
        let channel = default_channel(default, |value| {
            if sinks {
                with_sinks(value);
            }
        });
        let mut recording = recording(FakeHttp::platforms(), true);
        let http = recording.http.clone();
        let mut event = custom_event(&channel, None, ReplyAddress::None);
        event.lineage = Some(Lineage {
            root: "0123456789abcdef0123456789abcdef".into(),
            hop: 1,
            path: vec![reviewer()],
            reply: ReplyChoice::Default,
        });
        let receipt = tool(&mut recording, &channel, &event, buttons.clone()).await;
        assert_eq!(
            receipt.delivery,
            rejected("interactive_destination_unsupported")
        );
        assert!(receipt.message_ref.is_none());
        let recorded = recording.finish().await;
        assert!(http.sent().is_empty());
        assert!(webhooks(&recorded).is_empty());
    }
}

#[tokio::test(start_paused = true)]
async fn an_incapable_origin_falls_back_to_the_default_only_when_default_ok() {
    let channel = default_channel(telegram_default(), |_| {});
    let event = custom_event(&channel, Some(reviewer()), ReplyAddress::None);
    // The reference is bound, so the send is routed with the flag off, but
    // default_ok needs the flag: the origin keeps today's answer.
    let mut off = recording(FakeHttp::platforms(), false);
    let http = off.http.clone();
    let receipt = tool(&mut off, &channel, &event, json!({"text":"Hi."})).await;
    assert_eq!(receipt.delivery, DeliveryOutcome::Unsupported);
    off.finish().await;
    assert!(http.sent().is_empty());
    let mut on = recording(FakeHttp::platforms(), true);
    let http = on.http.clone();
    let receipt = tool(&mut on, &channel, &event, json!({"text":"Hi."})).await;
    assert!(confirmed(&receipt.delivery));
    on.finish().await;
    assert_eq!(
        telegram_messages(&http.sent()),
        [(CHAT.into(), Some("7".into()), "[reviewer] Hi.".into())]
    );
}

#[tokio::test(start_paused = true)]
async fn a_failed_sync_emit_resolves_the_default_only_when_default_ok() {
    let gone = || ReplyAddress::Sync {
        waiter: "gone".into(),
    };
    for flag in [true, false] {
        for sinks in [true, false] {
            let channel = default_channel(telegram_default(), |value| {
                if sinks {
                    with_sinks(value);
                }
            });
            let event = custom_event(&channel, Some(reviewer()), gone());
            let mut recording = recording(FakeHttp::platforms(), flag);
            let http = recording.http.clone();
            let receipt = tool(&mut recording, &channel, &event, json!({"text":"Hi."})).await;
            let hooks = webhooks(&recording.finish().await);
            let chats = telegram_messages(&http.sent());
            let case = format!("flag={flag} sinks={sinks}");
            if flag {
                assert!(confirmed(&receipt.delivery), "{case}");
                assert_eq!(chats.len(), 1, "{case}");
                assert!(hooks.is_empty(), "{case}");
            } else if sinks {
                assert!(confirmed(&receipt.delivery), "{case}");
                assert!(chats.is_empty(), "{case}");
                assert_eq!(hooks.len(), 1, "{case}");
                assert!(hooks[0]["data"].get("destination").is_none(), "{case}");
            } else {
                assert_eq!(receipt.delivery, DeliveryOutcome::Unsupported, "{case}");
                assert!(chats.is_empty() && hooks.is_empty(), "{case}");
            }
        }
    }
}

#[tokio::test(start_paused = true)]
async fn an_unrouted_turns_outputs_never_take_a_default_transport() {
    let channel = default_channel(telegram_default(), |value| {
        value["routing"]["default_consumer"]["reply"] = json!({"destination": "default"});
    });
    let recording = recording(FakeHttp::platforms(), true);
    let http = recording.http.clone();
    let event = custom_event(&channel, None, ReplyAddress::None);
    let mut unrouted = super::group_fixtures::routed_custom_activity();
    unrouted.routing = None;
    let turn = super::group_fixtures::consumer_turn(&recording.keys, &channel, unrouted, &event);
    recording.outcome(&turn, completed("Done.")).await;
    let recorded = recording.finish().await;
    // Today's custom answer goes to the sinks, and this channel has none.
    assert!(http.sent().is_empty());
    assert!(webhooks(&recorded).is_empty());
}

#[tokio::test(start_paused = true)]
async fn each_send_resolves_the_default_its_own_snapshot_names() {
    let before = default_channel(telegram_default(), |_| {});
    let after = default_channel(telegram_default(), |value| {
        value["routing"]["default_reply"]["chat_id"] = "-1009".into();
    });
    let detached = default_channel(telegram_default(), |value| {
        value["sources"].as_array_mut().unwrap().remove(1);
    });
    let mut recording = recording(FakeHttp::platforms(), true);
    let http = recording.http.clone();
    // A turn admitted under the earlier snapshot keeps it to its end.
    let turn_event = custom_event(&before, None, ReplyAddress::None);
    run(&recording, &before, &turn_event, [completed("Turn.")]).await;
    // A later tool call carries the snapshot its handler resolved.
    let mut later = custom_event(&after, Some(reviewer()), ReplyAddress::None);
    later.event_id = "msg-2".parse().unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;
    let receipt = tool(&mut recording, &after, &later, json!({"text":"Later."})).await;
    assert!(confirmed(&receipt.delivery));
    // Once its source is detached, a send asking for the default is refused.
    let mut asked = custom_event(&detached, Some(reviewer()), ReplyAddress::None);
    asked.lineage = Some(Lineage {
        root: "0123456789abcdef0123456789abcdef".into(),
        hop: 1,
        path: vec![reviewer()],
        reply: ReplyChoice::Default,
    });
    let refused = tool(&mut recording, &detached, &asked, json!({"text":"No."})).await;
    assert_eq!(refused.delivery, rejected("default_reply_unavailable"));
    recording.finish().await;
    let chats: Vec<_> = telegram_messages(&http.sent())
        .into_iter()
        .map(|(chat, _, text)| (chat, text))
        .collect();
    assert_eq!(
        chats,
        [
            (CHAT.to_owned(), "[lead] Turn.".to_owned()),
            ("-1009".to_owned(), "[reviewer] Later.".to_owned())
        ]
    );
}
