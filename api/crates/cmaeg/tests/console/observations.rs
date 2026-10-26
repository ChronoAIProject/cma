//! What ingress reports to the activity observer: authenticated events and
//! their refusals, producers and outcomes, and for anything that fails
//! authentication only a bounded counter call, never an observation.
use std::sync::{Arc, Mutex};

use axum::{
    body::Body,
    extract::{Path, State},
};
use cmaeg::{
    auth::webhook,
    delivery::sync::SyncRegistry,
    http::{callbacks, events, telegram},
    ingress::IngressState,
    turns::observer::{ActivityObserver, LaunchClass, ObservedLauncher, ProducerFacts},
};
use cmaeg_core::{
    activity::Activity,
    channel::Secret,
    ids::{ConsumerId, EventId, RecordId},
};
use serde_json::{json, Value};

use crate::relay_support::{now, Envelope, Fixture};

/// Records every observer call as a short line.
#[derive(Default)]
struct Recording(Mutex<Vec<String>>);

impl Recording {
    fn push(&self, line: String) {
        self.0.lock().unwrap().push(line);
    }

    fn lines(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

impl ActivityObserver for Recording {
    fn admitted(&self, _record: &RecordId, activity: &Activity) {
        self.push(format!("admitted:{}", activity.event_id));
    }

    fn refused(
        &self,
        _record: &RecordId,
        source: &'static str,
        _event_id: Option<&EventId>,
        reason: &str,
    ) {
        self.push(format!("refused:{source}:{reason}"));
    }

    fn launched(
        &self,
        _record: &RecordId,
        _event_id: &EventId,
        consumer: &ConsumerId,
        outcome: LaunchClass,
        _accepted: std::time::Duration,
    ) {
        self.push(format!("launched:{}:{outcome:?}", consumer.as_str()));
    }

    fn produced(&self, _record: &RecordId, producer: ProducerFacts<'_>) {
        self.push(format!(
            "produced:{}:{}:{:?}",
            producer.hint, producer.source, producer.outcome
        ));
    }

    fn unauthenticated(&self, source: &'static str) {
        self.push(format!("unauthenticated:{source}"));
    }
}

/// `state` reporting to a fresh recording, launching through an observed
/// launcher as the gateway composes it with the console on.
fn observed(state: &IngressState) -> (IngressState, Arc<Recording>) {
    let recording = Arc::new(Recording::default());
    let mut state = state.clone().with_activity_observer(recording.clone());
    state.launcher = Arc::new(ObservedLauncher::new(
        state.launcher.clone(),
        recording.clone(),
    ));
    (state, recording)
}

#[tokio::test]
async fn a_bad_relay_token_is_only_counted() {
    let fixture = Fixture::new(|_| {}, Default::default()).await;
    let (state, recording) = observed(&fixture.state);
    let mut envelope = Envelope::new();
    envelope.callback["iss"] = "https://other.example".into();
    let (body, token) = envelope.signed();
    let response = callbacks::callback(
        State(state),
        Path(fixture.id.to_string()),
        fixture.local_request(Body::from(body), &token),
    )
    .await;
    assert_eq!(response.status(), 401);
    assert_eq!(recording.lines(), ["unauthenticated:nyxid_relay"]);
}

#[tokio::test]
async fn an_authenticated_relay_event_is_admitted_and_its_launch_reported() {
    let fixture = Fixture::new(|_| {}, Default::default()).await;
    let (state, recording) = observed(&fixture.state);
    let (body, token) = Envelope::new().signed();
    let response = callbacks::callback(
        State(state),
        Path(fixture.id.to_string()),
        fixture.local_request(Body::from(body), &token),
    )
    .await;
    assert_eq!(response.status(), 202);
    let lines = recording.lines();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[0].starts_with("admitted:"), "{lines:?}");
    assert_eq!(lines[1], "launched:default:Deferred");
}

#[tokio::test]
async fn an_authenticated_relay_refusal_is_reported_with_its_reason() {
    let fixture = Fixture::new(
        |value| {
            value["sources"][0]["admission"] = json!({"type":"allowlist","sender_ids":["99"]});
        },
        Default::default(),
    )
    .await;
    let (state, recording) = observed(&fixture.state);
    let (body, token) = Envelope::new().signed();
    let response = callbacks::callback(
        State(state),
        Path(fixture.id.to_string()),
        fixture.local_request(Body::from(body), &token),
    )
    .await;
    assert_eq!(response.status(), 202);
    assert_eq!(recording.lines(), ["refused:nyxid_relay:sender_denied"]);
}

/// A native Telegram channel and its webhook secret.
async fn native() -> Fixture {
    Fixture::new(
        |value| {
            value["sources"] = json!([{"type":"telegram","bot_id":"77","bot_username":"fixture_bot","admission":{"type":"all"}}]);
            value["secrets"]["telegram_bot_token"] = "77:fixture".into();
            value["secrets"]["telegram_webhook_secret"] = "native-webhook-fixture".into();
        },
        Default::default(),
    )
    .await
}

fn update(fixture: &Fixture, secret: &str) -> axum::extract::Request {
    let raw = json!({"update_id":101,"message":{"message_id":5,"date":now()-5,"chat":{"id":42,"type":"private"},"from":{"id":42,"is_bot":false,"first_name":"Alice"},"text":"hi"}});
    axum::http::Request::post(format!("/v1/telegram/{}", fixture.id))
        .header("content-type", "application/json")
        .header("x-telegram-bot-api-secret-token", secret)
        .body(Body::from(raw.to_string()))
        .unwrap()
}

#[tokio::test]
async fn a_bad_telegram_secret_is_only_counted() {
    let fixture = native().await;
    let (state, recording) = observed(&fixture.state);
    let response = telegram::callback(
        State(state),
        Path(fixture.id.to_string()),
        update(&fixture, "wrong-secret"),
    )
    .await;
    assert_eq!(response.status(), 401);
    assert_eq!(recording.lines(), ["unauthenticated:telegram"]);
}

#[tokio::test]
async fn an_authenticated_telegram_update_is_admitted() {
    let fixture = native().await;
    let (state, recording) = observed(&fixture.state);
    let response = telegram::callback(
        State(state),
        Path(fixture.id.to_string()),
        update(&fixture, "native-webhook-fixture"),
    )
    .await;
    assert_eq!(response.status(), 202);
    let lines = recording.lines();
    assert!(lines[0].starts_with("admitted:"), "{lines:?}");
    assert_eq!(lines[1..], ["launched:default:Deferred"]);
}

/// A custom channel authenticated by HMAC key `ingress`.
pub(crate) async fn custom(secret: &Secret) -> Fixture {
    Fixture::new(
        |value| {
            value["sources"] = json!([{"type":"custom","auth":{"type":"hmac","key_id":"ingress"}}]);
            value["partition"]["key"] = "per_event".into();
            value["reply"]["sinks"] = json!([{"type":"sync"}]);
            value["secrets"]["hmac_secrets"] = json!([["ingress", secret.as_str()]]);
        },
        Default::default(),
    )
    .await
}

pub(crate) fn cloud_event(
    fixture: &Fixture,
    secret: Option<&Secret>,
    event: &Value,
) -> axum::extract::Request {
    let body = event.to_string();
    let mut request = axum::http::Request::post(format!("/v1/channels/{}/events", fixture.id))
        .header("content-type", "application/cloudevents+json");
    if let Some(secret) = secret {
        let timestamp = now().to_string();
        let signature = webhook::signature(secret, "delivery-1", &timestamp, body.as_bytes());
        request = request
            .header("webhook-id", "delivery-1")
            .header("webhook-timestamp", timestamp)
            .header("webhook-signature", signature);
    }
    request.body(Body::from(body)).unwrap()
}

pub(crate) async fn send_custom(
    fixture: &Fixture,
    state: IngressState,
    request: axum::extract::Request,
) -> axum::response::Response {
    events::receive_event(
        State(events::CustomIngressState {
            ingress: state,
            waiters: Arc::new(SyncRegistry::default()),
        }),
        Path(fixture.id.to_string()),
        request,
    )
    .await
}

#[tokio::test]
async fn an_unauthenticated_custom_event_is_only_counted() {
    let secret = Secret::new(uuid::Uuid::new_v4().to_string());
    let fixture = custom(&secret).await;
    let (state, recording) = observed(&fixture.state);
    let event = json!({"specversion":"1.0","id":"evt-1","type":"order.created","source":"https://user:pw@orders.example/a?x=1","data":{"a":1}});
    let forged = Secret::new("not-the-channel-secret");
    for secret in [None, Some(&forged)] {
        let response = send_custom(
            &fixture,
            state.clone(),
            cloud_event(&fixture, secret, &event),
        )
        .await;
        assert_eq!(response.status(), 401);
    }
    assert_eq!(
        recording.lines(),
        ["unauthenticated:custom", "unauthenticated:custom"]
    );
}

#[tokio::test]
async fn an_authenticated_custom_event_names_its_producer() {
    let secret = Secret::new(uuid::Uuid::new_v4().to_string());
    let fixture = custom(&secret).await;
    let (state, recording) = observed(&fixture.state);
    let event = json!({"specversion":"1.0","id":"evt-1","type":"order.created","source":"https://orders.example/a","data":{"a":1}});
    let response = send_custom(
        &fixture,
        state.clone(),
        cloud_event(&fixture, Some(&secret), &event),
    )
    .await;
    assert_eq!(response.status(), 202);
    assert_eq!(
        recording.lines(),
        [
            "admitted:evt-1",
            "launched:default:Deferred",
            "produced:hmac:ingress:https://orders.example/a:Deferred",
        ]
    );
    // An authenticated event that is not a CloudEvent is a refusal, named
    // without its body.
    let bad = json!({"id":"evt-2"});
    let response = send_custom(&fixture, state, cloud_event(&fixture, Some(&secret), &bad)).await;
    assert_eq!(response.status(), 400);
    assert_eq!(
        recording.lines().last().map(String::as_str),
        Some("refused:custom:invalid_cloud_event")
    );
}
