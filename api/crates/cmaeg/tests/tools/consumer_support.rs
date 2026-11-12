//! A routed channel for the consumer authorization tests: `default`, served
//! with the app's own key and presented as `lead`, and `reviewer`, an
//! additional `cma_codex` consumer with its own binding and agent key, plus
//! references issued to either of them.
use super::{
    media_support::{relayed, MediaSetup},
    routing_fixture,
    support::App,
};
use cmaeg::providers::{EventContextSnapshot, TurnRequest};
use cmaeg_core::{
    activity::AttachmentKind,
    channel::Secret,
    ids::{ConsumerId, ConversationId, IdempotencyKey, MessageRef},
    seal::{EventRef, Lineage, MessageRefPayload, RootId},
};
use serde_json::{json, Value};

pub const REVIEWER_BINDING: &str = "binding-reviewer";

pub fn reviewer() -> ConsumerId {
    ConsumerId::parse(routing_fixture::REVIEWER).unwrap()
}

/// The reviewer's agent key, a low-entropy fixture spelling.
pub fn reviewer_key() -> String {
    routing_fixture::agent_key(routing_fixture::REVIEWER)
}

/// Add `reviewer` to the stored definition and present `default` as `lead`.
pub fn add_reviewer(value: &mut Value) {
    value["routing"] = json!({"version": 1, "default_consumer": {"name": "lead"},
        "consumers": [routing_fixture::consumer(routing_fixture::REVIEWER, "reviewer", "cma_codex")],
        "default_targets": ["default"]});
    value["secrets"]["consumer_keys"] = json!([[routing_fixture::REVIEWER, reviewer_key()]]);
}

/// The setup of a routed app whose event carries one image.
pub fn routed_setup() -> MediaSetup {
    MediaSetup::new(vec![relayed(0, AttachmentKind::Image)]).edit(add_reviewer)
}

pub async fn routed() -> App {
    App::with_media(routed_setup()).await
}

/// A one-hop hand-off lineage published by the reviewer.
pub fn handoff(app: &App) -> Lineage {
    let payload = app
        .event_ref
        .open(&app.state.keys, super::relay_support::now())
        .unwrap()
        .payload;
    Lineage {
        root: RootId::derive(&payload.record_id, &payload.event_id)
            .as_str()
            .into(),
        hop: 1,
        path: vec![reviewer()],
        reply: Default::default(),
    }
}

/// The app's event issued to `consumer` (and, with `lineage`, as a hand-off),
/// with its context stored by the consumer's provider under the consumer's
/// own credential and conversation, as the consumer's turn would store it.
pub async fn issue(
    app: &App,
    consumer: &ConsumerId,
    credential: &str,
    lineage: Option<Lineage>,
) -> EventRef {
    let keys = &app.state.keys;
    let mut payload = app
        .event_ref
        .open(keys, super::relay_support::now())
        .unwrap()
        .payload;
    if !consumer.is_default() {
        payload.conversation_id = ConversationId::derive_for_consumer(
            keys,
            &payload.channel_id,
            1,
            consumer,
            &payload.partition_key,
        );
        payload.consumer = Some(consumer.clone());
    }
    payload.lineage = lineage;
    let reference = EventRef::seal(&payload, keys).unwrap();
    let mut stream = app
        .state
        .providers
        .start_turn(
            &"cma_codex".parse().unwrap(),
            TurnRequest {
                timing: None,
                conversation_id: payload.conversation_id.clone(),
                input: vec![],
                idempotency_key: IdempotencyKey::for_consumer(&payload.event_id, consumer),
                metadata: Default::default(),
                agent_key: Secret::new(credential),
                event_context: Some(EventContextSnapshot {
                    schema_version: 1,
                    channel_id: payload.channel_id.clone(),
                    conversation_id: payload.conversation_id.clone(),
                    activity: app.activity.clone(),
                    event_ref: reference.clone(),
                }),
                turn_trace: None,
            },
        )
        .await
        .unwrap();
    (&mut stream.accepted).await.unwrap();
    while stream.events.recv().await.is_some() {}
    reference
}

/// A message of the app's event delivered for `consumer` (`None` = default).
pub fn message_ref(app: &App, consumer: Option<ConsumerId>) -> MessageRef {
    let message = app.message_ref.open(&app.state.keys).unwrap().payload;
    MessageRef::seal(
        &MessageRefPayload {
            consumer,
            ..message
        },
        &app.state.keys,
    )
    .unwrap()
}

/// Send `method` to `path` on the app with `key`, and a JSON `body` if any.
pub async fn call(
    app: &App,
    method: reqwest::Method,
    path: &str,
    key: &str,
    body: Option<Value>,
) -> reqwest::Response {
    let request = app
        .fixture
        .client
        .request(method, format!("{}{path}", app.base))
        .bearer_auth(key);
    match body {
        Some(body) => request.json(&body),
        None => request,
    }
    .send()
    .await
    .unwrap()
}
