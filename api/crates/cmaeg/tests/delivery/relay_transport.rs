//! The relay client's replaceable transport sends exactly what HTTP sends and
//! maps every answer as a live response was mapped.
use super::{
    fake_http::{Answer, FakeHttp},
    fake_nyxid_reply::FakeReply,
};
use cmaeg::{
    delivery::{
        intents::DeliveryOutcome,
        relay_client::{Credential, RelayClient},
    },
    interactions::Destination,
    net::exchange::{BodyError, HttpReply, TransportError},
    DeliveryConfig,
};
use cmaeg_core::{channel::Platform, channel::Secret, seal::ReplyAddress};
use reqwest::StatusCode;
use serde_json::json;
use std::{sync::Arc, time::Duration};

fn address() -> ReplyAddress {
    ReplyAddress::NyxidRelay {
        message_id: "inbound-1".into(),
        route_id: "route-1".into(),
        platform: Platform::Telegram,
        reply_token: Secret::new("reply-token-fixture"),
        reply_token_expires_at: i64::MAX,
    }
}

fn faked(fake: &Arc<FakeHttp>, timeout: Duration) -> RelayClient {
    RelayClient::new("https://nyx.example", timeout)
        .unwrap()
        .with_transport(fake.clone())
}

#[tokio::test]
async fn a_fake_transport_receives_what_the_loopback_relay_receives() {
    let loopback = FakeReply::start(vec![]).await;
    let live = RelayClient::new(&loopback.base, Duration::from_secs(5)).unwrap();
    let key = Secret::new("route-key-fixture");
    let markup = json!({"inline_keyboard":[[{"text":"One","callback_data":"one"}]]});
    let live_outcome = live
        .send_rich(
            &address(),
            Credential::ChannelKey(&key),
            "hello",
            Some(&markup),
        )
        .await;

    let fake = Arc::new(FakeHttp::relay());
    let client = RelayClient::new(&loopback.base, Duration::from_secs(5))
        .unwrap()
        .with_transport(fake.clone());
    let faked_outcome = client
        .send_rich(
            &address(),
            Credential::ChannelKey(&key),
            "hello",
            Some(&markup),
        )
        .await;

    assert_eq!(live_outcome, faked_outcome);
    let recorded = &loopback.requests.lock().unwrap()[0];
    let sent = &fake.sent()[0];
    assert_eq!(
        sent.url.as_str(),
        format!("{}/api/v1/channel-relay/reply", loopback.base)
    );
    assert_eq!(
        sent.header("authorization"),
        Some(recorded.authorization.as_str())
    );
    assert!(sent.headers["authorization"].is_sensitive());
    assert_eq!(sent.header("content-type"), Some("application/json"));
    assert_eq!(sent.headers.len(), 2);
    assert_eq!(sent.json(), recorded.body);
    assert_eq!(sent.timeout, Duration::from_secs(5));
}

#[tokio::test(start_paused = true)]
async fn a_fake_sleeping_past_the_timeout_loses_the_response() {
    let fake =
        Arc::new(FakeHttp::relay().script([Answer::status(200).after(Duration::from_secs(60))]));
    let client = faked(&fake, Duration::from_millis(1500));
    let started = tokio::time::Instant::now();
    let outcome = client
        .send(
            &address(),
            Credential::ReplyToken(&Secret::new("token")),
            "hi",
        )
        .await;
    assert_eq!(started.elapsed(), Duration::from_millis(1500));
    assert_eq!(
        outcome,
        DeliveryOutcome::Unknown {
            reason: "response_lost".into()
        }
    );
}

#[tokio::test(start_paused = true)]
async fn reply_answers_map_onto_todays_outcomes() {
    let unknown = |reason: &str| DeliveryOutcome::Unknown {
        reason: reason.into(),
    };
    let with_body = |body| Answer {
        reply: Ok(HttpReply {
            status: StatusCode::OK,
            headers: Default::default(),
            body: Err(body),
        }),
        delay: Duration::ZERO,
    };
    for (answer, expected) in [
        (
            Answer::status(429).retry_after("7"),
            DeliveryOutcome::RateLimited { retry_after: 7 },
        ),
        (
            Answer::status(429).retry_after("999"),
            DeliveryOutcome::RateLimited { retry_after: 10 },
        ),
        (Answer::status(500), unknown("provider_failure")),
        (Answer::status(201), unknown("provider_failure")),
        (Answer::status(200), unknown("receipt_invalid")),
        (
            Answer::json(200, json!({"message_id":"inbound-1"})),
            unknown("receipt_invalid"),
        ),
        (with_body(BodyError::Oversized), unknown("receipt_invalid")),
        (with_body(BodyError::Lost), unknown("response_lost")),
        (
            Answer {
                reply: Err(TransportError::Unavailable),
                delay: Duration::ZERO,
            },
            unknown("response_lost"),
        ),
        (
            Answer::json(
                200,
                json!({"message_id":"out-1","platform_message_id":"p-1"}),
            ),
            DeliveryOutcome::Confirmed {
                platform_message_id: Some("p-1".into()),
            },
        ),
    ] {
        let fake = Arc::new(FakeHttp::relay().script([answer]));
        let outcome = faked(&fake, Duration::from_secs(5))
            .send(
                &address(),
                Credential::ReplyToken(&Secret::new("token")),
                "hi",
            )
            .await;
        assert_eq!(outcome, expected);
    }
    // A credential that is not a printable header value is refused before IO.
    let fake = Arc::new(FakeHttp::relay());
    let outcome = faked(&fake, Duration::from_secs(5))
        .send(
            &address(),
            Credential::ReplyToken(&Secret::new("a b")),
            "hi",
        )
        .await;
    assert_eq!(
        outcome,
        DeliveryOutcome::Rejected {
            reason: "no_credential".into()
        }
    );
    assert!(fake.sent().is_empty());
}

#[tokio::test(start_paused = true)]
async fn edits_post_to_the_update_endpoint_and_map_by_status() {
    let destination = Destination {
        outbound_id: "out-1".into(),
        platform_id: "p-1".into(),
    };
    let markup = json!({"inline_keyboard":[]});
    for (answer, expected) in [
        (
            Answer::status(200),
            DeliveryOutcome::Confirmed {
                platform_message_id: Some("p-1".into()),
            },
        ),
        (
            Answer {
                reply: Ok(HttpReply {
                    status: StatusCode::OK,
                    headers: Default::default(),
                    body: Err(BodyError::Lost),
                }),
                delay: Duration::ZERO,
            },
            DeliveryOutcome::Confirmed {
                platform_message_id: Some("p-1".into()),
            },
        ),
        (
            Answer::status(429).retry_after("3"),
            DeliveryOutcome::RateLimited { retry_after: 3 },
        ),
        (
            Answer::status(500),
            DeliveryOutcome::Unknown {
                reason: "edit_unconfirmed".into(),
            },
        ),
    ] {
        let fake = Arc::new(FakeHttp::relay().script([answer]));
        let outcome = faked(&fake, Duration::from_secs(5))
            .edit(&Secret::new("route-key"), &destination, "edited", &markup)
            .await;
        assert_eq!(outcome, expected);
        let sent = &fake.sent()[0];
        assert_eq!(
            sent.url.as_str(),
            "https://nyx.example/api/v1/channel-relay/reply/update"
        );
        assert_eq!(sent.header("authorization"), Some("Bearer route-key"));
        assert_eq!(
            sent.json(),
            json!({"message_id":"p-1","outbound_message_id":"out-1","reply":{"text":"edited","metadata":{"reply_markup":markup}}})
        );
    }
    // A credential that cannot be a header value is never sent.
    let fake = Arc::new(FakeHttp::relay());
    let outcome = faked(&fake, Duration::from_secs(5))
        .edit(&Secret::new("bad\nkey"), &destination, "edited", &markup)
        .await;
    assert_eq!(
        outcome,
        DeliveryOutcome::Unknown {
            reason: "edit_unconfirmed".into()
        }
    );
    assert!(fake.sent().is_empty());
}

#[test]
fn original_delivery_config_supports_exhaustive_literals_and_destructuring() {
    let config = DeliveryConfig {
        reply_timeout: Duration::from_secs(5),
        ack_text: "Working on it…".into(),
        heartbeat: Duration::from_secs(60),
        failure_text: "Sorry, I couldn’t finish that request. Please try again.".into(),
    };
    let DeliveryConfig {
        reply_timeout,
        ack_text,
        heartbeat,
        failure_text,
    } = config;
    let defaults = DeliveryConfig::default();
    assert_eq!(reply_timeout, defaults.reply_timeout);
    assert_eq!(ack_text, defaults.ack_text);
    assert_eq!(heartbeat, defaults.heartbeat);
    assert_eq!(failure_text, defaults.failure_text);
}

#[tokio::test(start_paused = true)]
async fn low_level_sends_preserve_large_payloads_wire_shape_and_error_precedence() {
    use cmaeg::delivery::relay_client::REQUEST_MAX_BYTES;
    let mut address = address();
    if let ReplyAddress::NyxidRelay { message_id, .. } = &mut address {
        *message_id = "id\\\"中文".repeat(30);
    }
    let ReplyAddress::NyxidRelay { message_id, .. } = &address else {
        unreachable!()
    };
    let markup = json!({"inline_keyboard":[],"note":"\\\""});
    // A raw backslash occupies two bytes in the serialized JSON string.
    let text = "\\".repeat(REQUEST_MAX_BYTES / 2 + 1);
    let unsupported = ReplyAddress::Telegram {
        bot_id: "bot-1".into(),
        chat_id: "chat-1".into(),
        message_id: "message-1".into(),
        thread_id: None,
    };
    let invalid = Secret::new("a b");
    let key = Secret::new("large-request-fixture");
    for complete in [false, true] {
        let fake = Arc::new(FakeHttp::relay());
        let client = faked(&fake, Duration::from_secs(5)).with_complete_replies(complete);
        assert_eq!(
            client
                .send(&unsupported, Credential::ChannelKey(&invalid), &text)
                .await,
            DeliveryOutcome::Unsupported
        );
        assert_eq!(
            client
                .send_rich(
                    &address,
                    Credential::ChannelKey(&invalid),
                    &text,
                    Some(&markup)
                )
                .await,
            (
                DeliveryOutcome::Rejected {
                    reason: "no_credential".into()
                },
                None
            )
        );
        assert!(fake.sent().is_empty());
        let receipt = client
            .send_rich(&address, Credential::ChannelKey(&key), &text, Some(&markup))
            .await;
        assert!(matches!(receipt.0, DeliveryOutcome::Confirmed { .. }));
        assert!(receipt.1.is_some());
        assert!(matches!(
            client
                .send(&address, Credential::ReplyToken(&key), &text)
                .await,
            DeliveryOutcome::Confirmed { .. }
        ));
        let sent = fake.sent();
        assert_eq!(sent.len(), 2);
        for (request, metadata) in sent
            .iter()
            .zip([json!({"reply_markup":markup}), serde_json::Value::Null])
        {
            assert!(request.body.len() > REQUEST_MAX_BYTES);
            assert_eq!(
                request.json(),
                json!({"message_id":message_id,"reply":{"text":text,"metadata":metadata}})
            );
            assert_eq!(
                request.header("authorization"),
                Some("Bearer large-request-fixture")
            );
        }
    }
}
