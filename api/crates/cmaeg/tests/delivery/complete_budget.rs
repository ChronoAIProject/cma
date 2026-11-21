//! Whole-reply latency differs from one platform part; no network is used here.
use super::{
    fake_http::{Answer, FakeHttp},
    recording_fixtures::{event, relay_activity, relay_channel, relay_reply, turn},
    recording_support::{Hooks, Recording},
};
use cmaeg::{
    delivery::intents::{DeliveryOutcome, ReplySource},
    turns::outcomes::TurnOutcome,
    DeliveryConfig,
};
use serde_json::json;
use std::time::Duration;
use tokio::time::Instant;

#[tokio::test(start_paused = true)]
async fn complete_ordinary_reply_can_confirm_after_sequential_parts_exceed_five_seconds() {
    for complete in [false, true] {
        // Model NyxID's sequential final-platform sends: 9000 ASCII bytes need
        // three Telegram parts, each accepted after 2 s. Legacy CMA sends three
        // requests and also waits 3 s between them; complete CMA sends once.
        let http = FakeHttp::new(|request, count| {
            let body = request.json();
            let parts = body["reply"]["text"].as_str().unwrap().len().div_ceil(4096);
            Answer::json(
                200,
                json!({"message_id":format!("out-{count}"),"platform_message_id":format!("platform-{count}")}),
            )
            .after(Duration::from_secs(2 * parts as u64))
        });
        let mut recording = Recording::start_with_relay(
            http,
            Hooks::answering([]),
            DeliveryConfig::default(),
            None,
            true,
            |relay| relay.with_complete_replies(complete),
        );
        let channel = relay_channel(0);
        let event = event(&channel, &relay_activity(), 1, relay_reply());
        let started = Instant::now();
        let receipt = recording
            .reply(
                &channel,
                &event,
                json!({"text":"x".repeat(9000)}),
                ReplySource::Stream,
            )
            .await;
        assert!(
            matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. }),
            "complete={complete}: {:?}",
            receipt.delivery
        );
        assert_eq!(
            started.elapsed(),
            Duration::from_secs(if complete { 6 } else { 12 })
        );
        let sent = recording.http.sent();
        assert_eq!(sent.len(), if complete { 1 } else { 3 });
        assert_eq!(
            sent[0].timeout,
            Duration::from_secs(if complete { 15 } else { 5 })
        );
        assert_eq!(
            sent.iter()
                .map(|request| request.json()["reply"]["text"].as_str().unwrap().to_owned())
                .collect::<String>(),
            "x".repeat(9000)
        );
        recording.finish().await;
    }
}

#[tokio::test(start_paused = true)]
async fn complete_ordinary_timeout_is_bounded_and_does_not_replay_or_fallback() {
    let http = FakeHttp::relay().script([Answer::json(
        200,
        json!({"message_id":"late-out","platform_message_id":"late-platform"}),
    )
    .after(Duration::from_secs(16))]);
    let mut recording = Recording::start_with_relay(
        http,
        Hooks::answering([]),
        DeliveryConfig::default(),
        None,
        true,
        |relay| relay.with_complete_replies(true),
    );
    let channel = relay_channel(0);
    let event = event(&channel, &relay_activity(), 1, relay_reply());
    let turn = turn(&recording.keys, &channel, relay_activity(), &event);
    let started = Instant::now();
    let receipt = recording
        .reply(
            &channel,
            &event,
            json!({"text":"x".repeat(9000)}),
            ReplySource::Tool,
        )
        .await;
    assert_eq!(started.elapsed(), Duration::from_secs(15));
    assert_eq!(
        receipt.delivery,
        DeliveryOutcome::Unknown {
            reason: "response_lost".into()
        }
    );
    assert!(receipt.message_ref.is_none());
    recording
        .outcome(
            &turn,
            TurnOutcome::Completed {
                final_text: Some("no duplicate".into()),
            },
        )
        .await;
    assert_eq!(
        recording.finish().await["calls"].as_array().unwrap().len(),
        1
    );
}
