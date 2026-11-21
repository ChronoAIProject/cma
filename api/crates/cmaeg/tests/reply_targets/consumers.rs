//! A reply target belongs to the consumer that created it: it is sealed for
//! that consumer's binding and key, only that key may use it, and removing
//! the consumer or rotating its key revokes it.
use super::{
    routing_fixture,
    support::{now, receipt, App},
};
use cmaeg_core::{
    channel::Secret,
    ids::ConsumerId,
    seal::{EventRef, ReplyTargetRef},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn reviewer() -> ConsumerId {
    ConsumerId::parse(routing_fixture::REVIEWER).unwrap()
}

fn reviewer_key() -> String {
    routing_fixture::agent_key(routing_fixture::REVIEWER)
}

/// Store the channel's definition with `edit` applied as its next version.
async fn store(app: &mut App, edit: impl FnOnce(&mut Value)) {
    let version = app.definition.version;
    let mut value = app.definition.to_blob_json();
    edit(&mut value);
    app.definition = serde_json::from_value(value).unwrap();
    app.definition.version = version;
    app.definition.validate().unwrap();
    app.update().await;
}

/// Add `reviewer`, an additional `cma_codex` consumer with its own binding.
fn add_reviewer(value: &mut Value) {
    value["routing"] = json!({"version": 1, "default_consumer": {"name": "lead"},
        "consumers": [routing_fixture::consumer(routing_fixture::REVIEWER, "reviewer", "cma_codex")],
        "default_targets": ["default"]});
    value["secrets"]["consumer_keys"] = json!([[routing_fixture::REVIEWER, reviewer_key()]]);
}

/// A routed channel and an event issued to its reviewer.
async fn routed() -> (App, EventRef) {
    let mut app = App::new(vec![]).await;
    store(&mut app, add_reviewer).await;
    let credential = Secret::new(reviewer_key());
    let event = app
        .event_for("alice", "reviewed", Some(reviewer()), credential)
        .await;
    (app, event)
}

async fn create(app: &App, event: &EventRef, key: &str) -> reqwest::Response {
    app.client
        .post(format!(
            "{}/v1/events/{}/reply-target",
            app.base,
            event.as_str()
        ))
        .bearer_auth(key)
        .json(&json!({}))
        .send()
        .await
        .unwrap()
}

async fn send(app: &App, target: &str, key: &str, id: &str) -> reqwest::Response {
    app.client
        .post(format!("{}/v1/reply-targets/{target}/messages", app.base))
        .bearer_auth(key)
        .header("idempotency-key", id)
        .json(&json!({"text": "later"}))
        .send()
        .await
        .unwrap()
}

/// The status and detail of a refused request.
async fn refusal(response: reqwest::Response) -> (u16, Value) {
    let status = response.status().as_u16();
    let body: Value = response.json().await.unwrap();
    (status, body["detail"].clone())
}

async fn reviewer_target(app: &App, event: &EventRef) -> String {
    let created = receipt(create(app, event, &reviewer_key()).await).await;
    created["reply_target_ref"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn a_consumers_reply_target_is_sealed_for_it_and_usable_only_with_its_key() {
    let (app, event) = routed().await;
    assert_eq!(
        refusal(create(&app, &event, &app.key).await).await,
        (403, json!("consumer_mismatch"))
    );
    let target = reviewer_target(&app, &event).await;
    let payload = target
        .parse::<ReplyTargetRef>()
        .unwrap()
        .open(&app.state.keys, now())
        .unwrap()
        .payload;
    assert_eq!(payload.binding_id.as_str(), "binding-reviewer");
    assert_eq!(payload.provider.as_str(), "cma_codex");
    let hash: [u8; 32] = Sha256::digest(reviewer_key().as_bytes()).into();
    assert_eq!(payload.agent_key_hash, hash);
    assert_eq!(payload.event.consumer, Some(reviewer()));
    // The default consumer's key is a valid key of the channel, but not the
    // key of the consumer this target belongs to.
    assert_eq!(
        refusal(send(&app, &target, &app.key, "default").await).await,
        (403, json!("reply_target_revoked"))
    );
    let sent = receipt(send(&app, &target, &reviewer_key(), "reviewer").await).await;
    assert_eq!(sent["delivery"]["outcome"], "confirmed");
}

#[tokio::test]
async fn removing_the_consumer_revokes_its_reply_targets() {
    let (mut app, event) = routed().await;
    let target = reviewer_target(&app, &event).await;
    store(&mut app, |value| {
        value.as_object_mut().unwrap().remove("routing");
        value["secrets"]
            .as_object_mut()
            .unwrap()
            .remove("consumer_keys");
    })
    .await;
    let (status, _) = refusal(send(&app, &target, &reviewer_key(), "removed").await).await;
    assert_eq!(status, 403);
    assert!(
        app.relay.requests.lock().unwrap().is_empty(),
        "nothing sent"
    );
}

#[tokio::test]
async fn rotating_the_consumers_key_revokes_its_reply_targets() {
    let (mut app, event) = routed().await;
    let target = reviewer_target(&app, &event).await;
    let rotated = format!("nyx_fixture_rotated_{}", routing_fixture::REVIEWER);
    let hash = routing_fixture::hash(&rotated);
    let key = rotated.clone();
    store(&mut app, move |value| {
        value["routing"]["consumers"][0]["agent_key_hash"] = hash.into();
        value["secrets"]["consumer_keys"] = json!([[routing_fixture::REVIEWER, key]]);
    })
    .await;
    let (status, _) = refusal(send(&app, &target, &reviewer_key(), "old-key").await).await;
    assert_eq!(status, 403);
    assert_eq!(
        refusal(send(&app, &target, &rotated, "new-key").await).await,
        (403, json!("reply_target_revoked"))
    );
    assert!(
        app.relay.requests.lock().unwrap().is_empty(),
        "nothing sent"
    );
}
