//! The legacy delivery recording and invariant I6's golden: the
//! `DeliveryEngine`, entered through `ShardedScheduler` as a `DeliverySink`,
//! driven through in-process transports, with every outbound call, its token
//! class, each Sync frame, each receipt and the paused-clock offset of each,
//! compared with `fixtures/legacy_sequences.json`, sequences (a)–(h).
//!
//! Unrouted delivery must reproduce this recording byte for byte at every
//! commit, whatever per-consumer state the engine grows, and no message in it
//! may carry a consumer label. Run with `UPDATE_LEGACY_SEQUENCES=1` to rewrite
//! the fixture only after an intended change, and review its diff.
use super::{
    fake_http::{Answer, FakeHttp},
    recording_fixtures::{
        activity, custom_channel, event, keys, relay_activity, relay_channel, relay_reply, turn,
    },
    recording_interactive,
    recording_support::{Hooks, Recording},
};
use cmaeg::{
    delivery::intents::ReplySource, ingress::launch::LaunchRequest, turns::outcomes::TurnOutcome,
    DeliveryConfig,
};
use cmaeg_core::{activity::SourceKind, seal::EventRefPayload, seal::ReplyAddress};
use serde_json::{json, Map, Value};
use std::{sync::Arc, time::Duration};

const GOLDEN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/delivery/fixtures/legacy_sequences.json"
);

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

/// A recording of one relay event on a channel acknowledging after
/// `ack_after_ms`, with the event and its admitted turn.
fn relay_turn(
    ack_after_ms: u32,
    http: FakeHttp,
    config: DeliveryConfig,
) -> (Recording, EventRefPayload, Arc<LaunchRequest>) {
    let channel = relay_channel(ack_after_ms);
    let activity = relay_activity();
    let event = event(&channel, &activity, 1, relay_reply());
    let turn = turn(&keys(), &channel, activity, &event);
    (
        Recording::start(http, Hooks::answering([]), config),
        event,
        turn,
    )
}

/// (a) The ack is sent with the reply token once `ack_after_ms` passes, and
/// the answer later with the channel key, after the route's pacing.
async fn ack_then_answer() -> Value {
    let (recording, _, turn) = relay_turn(1500, FakeHttp::platforms(), Default::default());
    recording.outcome(&turn, accepted()).await;
    recording.at(4000).await;
    recording.outcome(&turn, committed("The answer.")).await;
    recording.outcome(&turn, completed("The answer.")).await;
    recording.finish().await
}

/// (b) An answer before `ack_after_ms` sends no ack.
async fn immediate_answer() -> Value {
    let (recording, _, turn) = relay_turn(1500, FakeHttp::platforms(), Default::default());
    recording.outcome(&turn, accepted()).await;
    recording.outcome(&turn, committed("Quick.")).await;
    recording.outcome(&turn, completed("Quick.")).await;
    recording.at(3000).await;
    recording.finish().await
}

/// (c) After the ack, a quiet turn gets at most five heartbeats.
async fn heartbeats() -> Value {
    let config = DeliveryConfig {
        heartbeat: Duration::from_secs(10),
        ..Default::default()
    };
    let (recording, _, turn) = relay_turn(1000, FakeHttp::platforms(), config);
    recording.outcome(&turn, accepted()).await;
    recording.at(70_000).await;
    recording.outcome(&turn, completed("Done.")).await;
    recording.finish().await
}

/// (d) A failed turn sends the failure text.
async fn failure_text() -> Value {
    let (recording, _, turn) = relay_turn(0, FakeHttp::platforms(), Default::default());
    recording.outcome(&turn, accepted()).await;
    recording
        .outcome(
            &turn,
            TurnOutcome::Failed {
                code: "provider_failed".into(),
                message: "provider detail".into(),
            },
        )
        .await;
    recording.finish().await
}

/// (e) A tool reply suppresses the automatic final answer.
async fn tool_reply_suppresses_auto_final() -> Value {
    let (mut recording, event, turn) = relay_turn(0, FakeHttp::platforms(), Default::default());
    recording.outcome(&turn, accepted()).await;
    recording
        .reply(
            &turn.channel,
            &event,
            json!({"text":"From the tool."}),
            ReplySource::Tool,
        )
        .await;
    recording.outcome(&turn, committed("Automatic.")).await;
    recording.outcome(&turn, completed("Automatic.")).await;
    recording.finish().await
}

/// (f) A custom event streams Sync frames and posts result webhooks; the
/// terminal webhook needs all four attempts.
async fn custom_webhook_and_sync() -> Value {
    let channel = custom_channel();
    let activity = activity(SourceKind::Custom, "msg-1");
    let mut recording = Recording::start(
        FakeHttp::platforms(),
        Hooks::answering([200, 500, 500, 500, 200]),
        Default::default(),
    );
    let waiter = recording.waiter();
    let event = event(&channel, &activity, 1, ReplyAddress::Sync { waiter });
    let turn = turn(&recording.keys, &channel, activity, &event);
    recording.outcome(&turn, accepted()).await;
    recording
        .outcome(&turn, TurnOutcome::TextDelta { text: "Hel".into() })
        .await;
    recording
        .reply(
            &channel,
            &event,
            json!({"text":"Interim."}),
            ReplySource::Tool,
        )
        .await;
    recording.outcome(&turn, committed("Hello.")).await;
    recording.outcome(&turn, completed("Hello.")).await;
    recording.finish().await
}

/// (g) The reply token is presented once: a 429 is retried with the channel
/// key, which every later reply uses.
async fn token_once_then_channel_key() -> Value {
    let http = FakeHttp::platforms().script([Answer::status(429).retry_after("2")]);
    let (mut recording, event, turn) = relay_turn(0, http, Default::default());
    recording.outcome(&turn, accepted()).await;
    for text in ["First.", "Second."] {
        recording
            .reply(
                &turn.channel,
                &event,
                json!({"text":text}),
                ReplySource::Tool,
            )
            .await;
    }
    recording.outcome(&turn, completed("Unused.")).await;
    recording.finish().await
}

/// Every legacy sequence, normalized. (a)–(g) run under the paused clock;
/// (h) runs in real time, as `recording_interactive` explains.
fn record() -> Value {
    let paused = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .unwrap();
    let mut sequences = paused.block_on(async {
        let mut sequences = Map::new();
        sequences.insert("a_ack_then_answer".into(), ack_then_answer().await);
        sequences.insert("b_immediate_answer".into(), immediate_answer().await);
        sequences.insert("c_heartbeats".into(), heartbeats().await);
        sequences.insert("d_failure_text".into(), failure_text().await);
        sequences.insert(
            "e_tool_reply_suppresses_auto_final".into(),
            tool_reply_suppresses_auto_final().await,
        );
        sequences.insert(
            "f_custom_webhook_and_sync".into(),
            custom_webhook_and_sync().await,
        );
        sequences.insert(
            "g_token_once_then_channel_key".into(),
            token_once_then_channel_key().await,
        );
        sequences
    });
    let real_time = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    sequences.insert(
        "h_relay_update_and_native_controls".into(),
        real_time.block_on(recording_interactive::record()),
    );
    Value::Object(sequences)
}

#[test]
fn legacy_delivery_matches_the_recorded_sequences() {
    let recorded = record();
    if std::env::var_os("UPDATE_LEGACY_SEQUENCES").is_some() {
        let pretty = serde_json::to_string_pretty(&recorded).unwrap();
        std::fs::write(GOLDEN, pretty + "\n").unwrap();
    }
    let golden: Value = serde_json::from_str(&std::fs::read_to_string(GOLDEN).unwrap()).unwrap();
    for (name, expected) in golden.as_object().unwrap() {
        assert_eq!(&recorded[name], expected, "sequence {name} changed");
    }
    assert_eq!(recorded, golden);
    assert_unlabeled(&recorded);
}

/// No chat message of an unrouted channel carries a `[<label>] ` prefix. A
/// label opens with a literal `[`, which message text can only produce
/// escaped, as `\[`.
fn assert_unlabeled(recorded: &Value) {
    let mut texts = 0;
    for (name, sequence) in recorded.as_object().unwrap() {
        for call in sequence["calls"].as_array().unwrap() {
            let text = match call["transport"].as_str() {
                Some("relay") => &call["body"]["reply"]["text"],
                Some("telegram") => &call["body"]["text"],
                _ => continue,
            };
            let text = text.as_str().unwrap();
            assert!(!text.starts_with('['), "{name} labeled a message: {text}");
            texts += 1;
        }
    }
    assert!(texts > 0, "the recording sent no chat message");
}

#[test]
fn two_recordings_normalize_identically() {
    assert_eq!(record(), record());
}
