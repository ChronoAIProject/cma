//! Readers of extended source shapes, end to end through ingress. Writes of
//! these shapes are still refused, so each channel is stored directly, as a
//! writer behind the dynamic-sources flag would store it.
use super::support::*;
use cmaeg::{
    auth::webhook,
    delivery::sync::SyncRegistry,
    http::{self, events::CustomIngressState},
    turns::{
        render_input,
        render_media::{Audience, RenderMedia},
    },
};
use cmaeg_core::{
    activity::{ActivityKind, SourceKind},
    channel::{Platform, Secret, Source},
    ids::ConversationId,
    seal::ReplyAddress,
};
use serde_json::{json, Value};
use std::sync::Arc;

/// Copy the fixture's relay, which signs against its JWKS, onto `platform`
/// admitting `route`.
fn relay(value: &Value, platform: &str, route: &str) -> Value {
    let mut source = value["sources"][0].clone();
    source["platform"] = platform.into();
    source["route_ids"] = json!([route]);
    source.as_object_mut().unwrap().remove("bot_username");
    source
}

/// A callback signed for `platform` on `route`, sent with its own header.
async fn send(fixture: &Fixture, platform: &str, route: &str) -> reqwest::Response {
    let mut envelope = Envelope::on(platform);
    envelope.payload["conversation"]["id"] = route.into();
    envelope.reply["conversation_id"] = route.into();
    send_envelope(fixture, platform, &envelope).await
}

async fn send_envelope(
    fixture: &Fixture,
    platform: &str,
    envelope: &Envelope,
) -> reqwest::Response {
    let (body, token) = envelope.signed();
    let mut request = fixture.request(body, &token).build().unwrap();
    request
        .headers_mut()
        .insert("x-nyxid-platform", platform.parse().unwrap());
    fixture.client.execute(request).await.unwrap()
}

async fn accepted(response: reqwest::Response) {
    assert_eq!(response.status(), 202);
    let value: Value = response.json().await.unwrap();
    assert_eq!(value["status"], "accepted", "{value}");
}

#[tokio::test]
async fn lark_and_feishu_callbacks_select_their_own_source_and_routes() {
    let fixture = Fixture::on(
        "lark",
        |value| {
            value["sources"] = json!([
                relay(value, "lark", "route-1"),
                relay(value, "feishu", "route-2")
            ]);
        },
        Default::default(),
    )
    .await;
    fixture
        .recorder
        .outcome
        .store(1, std::sync::atomic::Ordering::SeqCst);
    accepted(send(&fixture, "lark", "route-1").await).await;
    accepted(send(&fixture, "feishu", "route-2").await).await;
    // Each source checks only its own routes.
    problem(
        send(&fixture, "feishu", "route-1").await,
        401,
        "route_denied",
    )
    .await;
    problem(send(&fixture, "lark", "route-2").await, 401, "route_denied").await;
    let requests = fixture.recorder.requests.lock().unwrap();
    let sources: Vec<_> = requests
        .iter()
        .map(|request| request.activity.source.clone())
        .collect();
    assert_eq!(
        sources,
        [
            SourceKind::NyxidRelay {
                platform: Platform::Lark,
                route_id: "route-1".into()
            },
            SourceKind::NyxidRelay {
                platform: Platform::Feishu,
                route_id: "route-2".into()
            },
        ]
    );
}

/// A Telegram relay group message addressing `@support_bot`.
fn addressed() -> Envelope {
    let text = "@support_bot hello";
    let mut envelope = Envelope::on("telegram");
    envelope.group();
    envelope.payload["content"]["text"] = text.into();
    let message = &mut envelope.payload["raw_platform_data"]["message"];
    message["text"] = text.into();
    message["entities"] = json!([{"type":"mention","offset":0,"length":12}]);
    envelope
}

#[tokio::test]
async fn a_telegram_relay_behind_a_lark_relay_keeps_its_cards_and_its_own_bot() {
    let fixture = Fixture::on(
        "telegram",
        |value| {
            let mut lark = relay(value, "lark", "route-lark");
            lark["bot_username"] = "lark_bot".into();
            let telegram = value["sources"][0].clone();
            value["sources"] = json!([lark, telegram]);
        },
        Default::default(),
    )
    .await;
    fixture
        .recorder
        .outcome
        .store(1, std::sync::atomic::Ordering::SeqCst);
    accepted(send_envelope(&fixture, "telegram", &addressed()).await).await;
    let requests = fixture.recorder.requests.lock().unwrap();
    let request = &requests[0];
    let ActivityKind::Message { mentions_bot, .. } = &request.activity.kind else {
        panic!("expected a message");
    };
    assert!(mentions_bot, "the Telegram relay's own bot is addressed");
    // Its replies, and so its cards, are sealed on the Telegram relay's route,
    // which selects that relay rather than the channel's first chat source.
    let opened = request
        .event_ref
        .open(&fixture.state.master_keys, now())
        .unwrap()
        .payload;
    let ReplyAddress::NyxidRelay {
        route_id, platform, ..
    } = &opened.reply
    else {
        panic!("wrong reply transport");
    };
    assert_eq!(
        (route_id.as_str(), *platform),
        ("route-1", Platform::Telegram)
    );
    let definition = &request.channel.definition;
    assert!(matches!(
        definition.relay_source_for_route(route_id),
        Some(Source::NyxidRelay {
            platform: Platform::Telegram,
            ..
        })
    ));
    // The mention of its own bot is stripped, not the first relay's bot.
    let input = render_input::render(
        &request.activity,
        definition,
        &RenderMedia::empty(None),
        Audience {
            tools: false,
            downloads: false,
        },
        None,
    );
    assert_eq!(input[0]["content"][0]["text"], "hello");
}

async fn serve(router: axum::Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    (
        base,
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() }),
    )
}

/// Custom ingress over the fixture's channel, launching into its recorder.
async fn custom_ingress(fixture: &Fixture) -> (String, tokio::task::JoinHandle<()>) {
    serve(http::events::router(CustomIngressState {
        ingress: fixture.state.clone(),
        waiters: Arc::new(SyncRegistry::default()),
    }))
    .await
}

/// A structured CloudEvent for the fixture's channel, signed with `secret`.
fn signed_event(fixture: &Fixture, base: &str, secret: &Secret) -> reqwest::RequestBuilder {
    let body = json!({"specversion":"1.0","id":"event-1","type":"order.created",
        "source":"https://orders.example","subject":"customer-1"})
    .to_string();
    let timestamp = now().to_string();
    let signature = webhook::signature(secret, "delivery-1", &timestamp, body.as_bytes());
    fixture
        .client
        .post(format!("{base}/v1/channels/{}/events", fixture.id))
        .header("content-type", "application/cloudevents+json")
        .header("webhook-id", "delivery-1")
        .header("webhook-timestamp", timestamp)
        .header("webhook-signature", signature)
        .body(body)
}

#[tokio::test]
async fn a_channel_without_sources_refuses_every_ingress() {
    let fixture = Fixture::on(
        "telegram",
        |value| {
            value["sources"] = json!([]);
            value["reply"]["sinks"] = json!([{"type":"sync"}]);
            value["partition"]["key"] = "thread".into();
        },
        Default::default(),
    )
    .await;
    problem(
        fixture.send(&Envelope::new()).await,
        400,
        "no matching relay source",
    )
    .await;
    let (native, native_task) = serve(http::telegram::router(fixture.state.clone())).await;
    let response = fixture
        .client
        .post(format!("{native}/v1/telegram/{}", fixture.id))
        .header("x-telegram-bot-api-secret-token", "native-webhook-fixture")
        .json(&json!({"update_id":1,"message":{"message_id":5,"date":now(),"chat":{"id":42,"type":"private"},"from":{"id":42,"is_bot":false,"first_name":"Alice"},"text":"hi"}}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["detail"], "telegram_source_missing", "{body}");
    let (custom, custom_task) = custom_ingress(&fixture).await;
    let response = signed_event(&fixture, &custom, &Secret::new("any-secret"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["detail"], "channel has no custom source", "{body}");
    assert!(fixture.recorder.requests.lock().unwrap().is_empty());
    native_task.abort();
    custom_task.abort();
}

#[tokio::test]
async fn custom_events_are_admitted_on_a_multi_relay_channel_with_a_custom_key() {
    let secret = Secret::new("multi-relay-ingress-fixture");
    let fixture = Fixture::on(
        "lark",
        |value| {
            value["sources"] = json!([
                relay(value, "lark", "route-1"),
                relay(value, "feishu", "route-2"),
                {"type":"custom","auth":{"type":"hmac","key_id":"ingress"}},
            ]);
            value["partition"] =
                json!({"version":1,"key":"conversation","custom_key":"event_subject"});
            value["secrets"]["hmac_secrets"] = json!([["ingress", secret.as_str()]]);
        },
        Default::default(),
    )
    .await;
    fixture
        .recorder
        .outcome
        .store(1, std::sync::atomic::Ordering::SeqCst);
    let (custom, task) = custom_ingress(&fixture).await;
    let response = signed_event(&fixture, &custom, &secret)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202, "{}", response.text().await.unwrap());
    let requests = fixture.recorder.requests.lock().unwrap();
    let request = &requests[0];
    assert_eq!(request.activity.source, SourceKind::Custom);
    assert_eq!(request.activity.conversation.id, "customer-1");
    assert_eq!(
        request.conversation_id,
        ConversationId::derive(&fixture.state.master_keys, &fixture.id, 1, "customer-1")
    );
    task.abort();
}
