//! NyxID's proactive relay send: the request, its per-part keys and the
//! mapping of every answer, in process and over a loopback socket.
use super::{
    fake_http::{Answer, FakeHttp, Sent},
    fake_nyxid_reply::{FakeReply, Reply},
};
use cmaeg::{
    delivery::{
        intents::DeliveryOutcome,
        part_keys::PartKeys,
        relay_client::{
            RelayClient, DEFAULT_REPLY_REFUSED, DEFAULT_REPLY_UNSUPPORTED,
            DELIVERY_IN_PROGRESS_OR_UNCERTAIN, IDEMPOTENCY_CONFLICT,
        },
    },
    net::exchange::TransportError,
};
use cmaeg_core::channel::Secret;
use serde_json::json;
use std::{sync::Arc, time::Duration};

pub const ROUTE: &str = "6f1c9a3e-2b4d-4c5e-8f70-112233445566";
const KEY: &str = "default-relay-channel-key";

fn client(fake: &Arc<FakeHttp>) -> RelayClient {
    RelayClient::new("https://nyx.example", Duration::from_secs(5))
        .unwrap()
        .with_transport(fake.clone())
}

/// One initiated send answered by `answer`, and every request it made.
async fn send(answer: Answer) -> (DeliveryOutcome, Vec<Sent>) {
    let fake = Arc::new(FakeHttp::new(move |_, _| answer.clone()));
    let outcome = client(&fake)
        .send_initiated(ROUTE, &Secret::new(KEY), "Hello", "tool_key")
        .await;
    (outcome, fake.sent())
}

fn rejected(reason: &str) -> DeliveryOutcome {
    DeliveryOutcome::Rejected {
        reason: reason.into(),
    }
}

fn unknown(reason: &str) -> DeliveryOutcome {
    DeliveryOutcome::Unknown {
        reason: reason.into(),
    }
}

#[tokio::test(start_paused = true)]
async fn the_request_names_the_conversation_text_and_key_under_the_channel_key() {
    let fake = Arc::new(FakeHttp::relay());
    let key = PartKeys::Tool { seed: [1; 32] }.key(0, "Hello");
    let outcome = client(&fake)
        .send_initiated(ROUTE, &Secret::new(KEY), "Hello", &key)
        .await;
    assert_eq!(
        outcome,
        DeliveryOutcome::Confirmed {
            platform_message_id: Some("platform-1".into())
        }
    );
    let sent = fake.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].url.as_str(),
        "https://nyx.example/api/v1/channel-relay/send"
    );
    assert_eq!(
        sent[0].header("authorization"),
        Some(format!("Bearer {KEY}").as_str())
    );
    assert!(sent[0].headers["authorization"].is_sensitive());
    assert_eq!(sent[0].header("content-type"), Some("application/json"));
    assert_eq!(
        sent[0].json(),
        json!({"conversation_id":ROUTE,"message":{"text":"Hello"},"idempotency_key":key})
    );
    assert!(key.len() <= 128);
}

#[tokio::test(start_paused = true)]
async fn refusals_map_to_typed_rejections_without_a_retry() {
    let cases = [
        (Answer::status(403), rejected(DEFAULT_REPLY_REFUSED)),
        (Answer::status(400), rejected(DEFAULT_REPLY_REFUSED)),
        (
            Answer::json(
                501,
                json!({"error":"channel_platform_send_unsupported","error_code":1003}),
            ),
            rejected(DEFAULT_REPLY_UNSUPPORTED),
        ),
        (
            Answer::json(
                409,
                json!({"error_code":1004,"message":"Idempotency key was used for a different message"}),
            ),
            rejected(IDEMPOTENCY_CONFLICT),
        ),
        // A send in progress, or one whose claim expired after the platform
        // accepted it: it may have been sent, so it is never retried.
        (
            Answer::json(
                409,
                json!({"error_code":1004,"message":"A send with this key is in progress"}),
            ),
            unknown(DELIVERY_IN_PROGRESS_OR_UNCERTAIN),
        ),
        (
            Answer::status(409),
            unknown(DELIVERY_IN_PROGRESS_OR_UNCERTAIN),
        ),
    ];
    for (answer, expected) in cases {
        let (outcome, sent) = send(answer).await;
        assert_eq!(outcome, expected);
        assert_eq!(sent.len(), 1, "{expected:?} is answered by one request");
    }
}

#[tokio::test(start_paused = true)]
async fn a_429_is_returned_for_the_callers_single_retry() {
    let (outcome, sent) = send(Answer::status(429).retry_after("4")).await;
    assert_eq!(outcome, DeliveryOutcome::RateLimited { retry_after: 4 });
    assert_eq!(sent.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn lost_and_unreadable_answers_are_unknown() {
    let lost = Answer {
        reply: Err(TransportError::Unavailable),
        delay: Duration::ZERO,
    };
    let late = Answer::status(200).after(Duration::from_secs(6));
    let cases = [
        (lost, unknown("response_lost")),
        (late, unknown("response_lost")),
        (Answer::status(200), unknown("receipt_invalid")),
        (
            Answer::json(200, json!({"message_id":"","platform_message_id":"1"})),
            unknown("receipt_invalid"),
        ),
        (Answer::status(500), unknown("provider_failure")),
    ];
    for (answer, expected) in cases {
        assert_eq!(send(answer).await.0, expected);
    }
}

#[tokio::test(start_paused = true)]
async fn an_unpresentable_channel_key_sends_nothing() {
    for key in ["", "has space"] {
        let fake = Arc::new(FakeHttp::relay());
        let outcome = client(&fake)
            .send_initiated(ROUTE, &Secret::new(key), "Hello", "tool_key")
            .await;
        assert_eq!(outcome, rejected("no_credential"));
        assert!(fake.sent().is_empty());
    }
}

#[tokio::test]
async fn the_http_transport_reads_a_conflicts_message() {
    let different = Reply {
        body: json!({"error_code":1004,"message":"key reused for a different message"}),
        ..Reply::status(409)
    };
    let uncertain = Reply {
        body: json!({"error_code":1004,"message":"send outcome uncertain"}),
        ..Reply::status(409)
    };
    let loopback = FakeReply::start(vec![different, uncertain]).await;
    let live = RelayClient::new(&loopback.base, Duration::from_secs(5)).unwrap();
    let key = Secret::new(KEY);
    let mut outcomes = Vec::new();
    for idempotency_key in ["tool_one", "tool_two", "tool_three"] {
        outcomes.push(
            live.send_initiated(ROUTE, &key, "Hello", idempotency_key)
                .await,
        );
    }
    assert_eq!(
        outcomes,
        [
            rejected(IDEMPOTENCY_CONFLICT),
            unknown(DELIVERY_IN_PROGRESS_OR_UNCERTAIN),
            DeliveryOutcome::Confirmed {
                platform_message_id: Some("platform-3".into())
            }
        ]
    );
    let requests = loopback.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    for request in requests.iter() {
        assert_eq!(request.path, "/api/v1/channel-relay/send");
        assert_eq!(request.authorization, format!("Bearer {KEY}"));
        assert_eq!(request.body["conversation_id"], ROUTE);
        assert_eq!(request.body["message"]["text"], "Hello");
    }
    assert_eq!(requests[2].body["idempotency_key"], "tool_three");
}
