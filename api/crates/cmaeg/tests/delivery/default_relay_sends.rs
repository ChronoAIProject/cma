//! Proactive relay sends to a default conversation on the real engine,
//! under the paused clock and the in-process transport: one key per part,
//! the shared route pacer, today's single 429 retry and the refusals, with
//! no secret, chat id, route id or text in any log.
use super::{
    default_fixtures::{
        custom_event, default_channel, initiated, recording, relay_default, run, telegram_default,
        tool, CHAT,
    },
    default_relay::ROUTE,
    fake_http::{Answer, FakeHttp},
    group_fixtures::reviewer,
    recording_fixtures::{BOT_TOKEN, CHANNEL_KEY, REPLY_TOKEN},
};
use cmaeg::{delivery::intents::DeliveryOutcome, turns::outcomes::TurnOutcome};
use cmaeg_core::seal::{EventRefPayload, ReplyAddress};
use serde_json::json;
use std::{
    collections::HashSet,
    io::Write,
    sync::{Arc, Mutex},
    time::Duration,
};

fn event(channel: &cmaeg::channels::resolver::ResolvedChannel) -> EventRefPayload {
    custom_event(channel, Some(reviewer()), ReplyAddress::None)
}

fn key(sent: &super::fake_http::Sent) -> String {
    sent.json()["idempotency_key"].as_str().unwrap().to_owned()
}

#[tokio::test(start_paused = true)]
async fn every_part_gets_its_own_key_and_waits_on_the_route_pacer() {
    let channel = default_channel(relay_default(), |_| {});
    let mut recording = recording(FakeHttp::platforms(), true);
    let http = recording.http.clone();
    let long = "a".repeat(9000);
    let receipt = tool(
        &mut recording,
        &channel,
        &event(&channel),
        json!({ "text": long }),
    )
    .await;
    assert!(matches!(
        receipt.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    recording.finish().await;
    let sent = http.sent();
    let parts = initiated(&sent);
    assert_eq!(parts.len(), 3, "a long reply is never truncated");
    let keys: HashSet<_> = parts.iter().map(|sent| key(sent)).collect();
    assert_eq!(keys.len(), 3);
    for pair in parts.windows(2) {
        assert!(pair[1].at - pair[0].at >= Duration::from_secs(3));
    }
    let text: String = parts
        .iter()
        .map(|sent| sent.json()["message"]["text"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(text, format!("[reviewer] {}", "a".repeat(9000)));
}

#[tokio::test(start_paused = true)]
async fn two_committed_messages_get_distinct_output_keys() {
    let channel = default_channel(relay_default(), |_| {});
    let recording = recording(FakeHttp::platforms(), true);
    let http = recording.http.clone();
    let committed = |text: &str| TurnOutcome::MessageCommitted { text: text.into() };
    run(
        &recording,
        &channel,
        &event(&channel),
        [committed("First."), committed("Second.")],
    )
    .await;
    recording.finish().await;
    let sent = http.sent();
    let outputs = initiated(&sent);
    assert_eq!(outputs.len(), 2);
    let (first, second) = (key(outputs[0]), key(outputs[1]));
    assert_ne!(first, second);
    for key in [first, second] {
        assert_eq!(key.len(), 64);
        assert!(key.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }
}

#[tokio::test(start_paused = true)]
async fn a_burst_of_sends_to_one_route_is_spaced_three_seconds_apart() {
    let channel = default_channel(relay_default(), |_| {});
    let mut recording = recording(FakeHttp::platforms(), true);
    let http = recording.http.clone();
    for text in ["One.", "Two.", "Three."] {
        let receipt = tool(
            &mut recording,
            &channel,
            &event(&channel),
            json!({ "text": text }),
        )
        .await;
        assert!(matches!(
            receipt.delivery,
            DeliveryOutcome::Confirmed { .. }
        ));
    }
    recording.finish().await;
    let sent = http.sent();
    let sends = initiated(&sent);
    assert_eq!(sends.len(), 3);
    for pair in sends.windows(2) {
        assert!(pair[1].at - pair[0].at >= Duration::from_secs(3));
    }
    let keys: HashSet<_> = sends.iter().map(|sent| key(sent)).collect();
    assert_eq!(keys.len(), 3, "every tool call draws new keys");
}

#[tokio::test(start_paused = true)]
async fn a_429_is_retried_once_with_the_same_key_after_its_cooldown() {
    let channel = default_channel(relay_default(), |_| {});
    let http = FakeHttp::platforms().script([Answer::status(429).retry_after("5")]);
    let mut recording = recording(http, true);
    let http = recording.http.clone();
    let receipt = tool(
        &mut recording,
        &channel,
        &event(&channel),
        json!({"text":"Hi."}),
    )
    .await;
    assert!(matches!(
        receipt.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    recording.finish().await;
    let sent = http.sent();
    let attempts = initiated(&sent);
    assert_eq!(attempts.len(), 2);
    assert_eq!(key(attempts[0]), key(attempts[1]));
    assert!(attempts[1].at - attempts[0].at >= Duration::from_secs(5));
}

#[tokio::test(start_paused = true)]
async fn a_second_429_rejects_the_part_without_a_third_attempt() {
    let channel = default_channel(relay_default(), |_| {});
    let limited = Answer::status(429).retry_after("2");
    let http = FakeHttp::platforms().script([limited.clone(), limited]);
    let mut recording = recording(http, true);
    let http = recording.http.clone();
    let receipt = tool(
        &mut recording,
        &channel,
        &event(&channel),
        json!({"text":"Hi."}),
    )
    .await;
    assert_eq!(
        receipt.delivery,
        DeliveryOutcome::Rejected {
            reason: "default_reply_rate_limited".into()
        }
    );
    recording.finish().await;
    assert_eq!(initiated(&http.sent()).len(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_refused_initiated_send_is_reported_in_the_receipt() {
    let channel = default_channel(relay_default(), |_| {});
    for (status, reason) in [
        (403, "default_reply_refused"),
        (501, "default_reply_unsupported"),
    ] {
        let http = FakeHttp::platforms().script([Answer::status(status)]);
        let mut recording = recording(http, true);
        let receipt = tool(
            &mut recording,
            &channel,
            &event(&channel),
            json!({"text":"Hi."}),
        )
        .await;
        assert_eq!(
            receipt.delivery,
            DeliveryOutcome::Rejected {
                reason: reason.into()
            }
        );
        recording.finish().await;
    }
}

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

#[tokio::test(start_paused = true)]
async fn trace_logs_of_default_replies_carry_no_secret_id_or_text() {
    let logs = Logs::default();
    let writer = logs.clone();
    let _capture = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish(),
    );
    let refused = Answer::status(403);
    for (default, script) in [
        (telegram_default(), vec![]),
        (relay_default(), vec![]),
        (relay_default(), vec![refused]),
    ] {
        let channel = default_channel(default, |_| {});
        let mut recording = recording(FakeHttp::platforms().script(script), true);
        let event = event(&channel);
        run(
            &recording,
            &channel,
            &event,
            [TurnOutcome::Completed {
                final_text: Some("Secret answer.".into()),
            }],
        )
        .await;
        tool(
            &mut recording,
            &channel,
            &event,
            json!({"text":"Secret tool."}),
        )
        .await;
        recording.finish().await;
    }
    let text = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert!(text.contains("default reply refused"), "{text}");
    for secret in [
        BOT_TOKEN,
        CHANNEL_KEY,
        REPLY_TOKEN,
        CHAT,
        ROUTE,
        "Secret answer.",
        "Secret tool.",
    ] {
        assert!(!text.contains(secret), "{secret} leaked:\n{text}");
    }
}
