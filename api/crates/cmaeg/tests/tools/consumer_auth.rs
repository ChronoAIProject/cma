//! Per-consumer tool authorization on a routed channel: each consumer's agent
//! key acts only on the references issued to that consumer.
use super::{
    consumer_support::{call, handoff, issue, message_ref, reviewer, reviewer_key, routed},
    support::{problem, App},
};
use cmaeg_core::{
    ids::{ConsumerId, RecordId},
    seal::EventRef,
};
use reqwest::Method;
use serde_json::{json, Value};

/// Every event tool, as `(method, path, body)` for the event `reference`.
fn event_tools(app: &App, reference: &str) -> Vec<(Method, String, Option<Value>)> {
    let activity = serde_json::to_value(&app.activity).unwrap();
    vec![
        (Method::GET, format!("/v1/events/{reference}/context"), None),
        (
            Method::POST,
            format!("/v1/events/{reference}/context"),
            Some(activity),
        ),
        (
            Method::POST,
            format!("/v1/events/{reference}/replies"),
            Some(json!({"text": "not yours"})),
        ),
        (
            Method::POST,
            format!("/v1/events/{reference}/typing"),
            Some(json!({})),
        ),
        (
            Method::POST,
            format!("/v1/events/{reference}/reply-target"),
            Some(json!({})),
        ),
        (
            Method::GET,
            format!("/v1/events/{reference}/attachments/0"),
            None,
        ),
    ]
}

async fn mismatched(response: reqwest::Response, what: &str) {
    let body = problem(response, 403, "forbidden").await;
    assert_eq!(body["detail"], "consumer_mismatch", "{what}: {body}");
}

#[tokio::test]
async fn a_consumer_key_cannot_act_on_another_consumers_event() {
    let app = routed().await;
    let theirs = issue(&app, &reviewer(), &reviewer_key(), None).await;
    for (method, path, body) in event_tools(&app, theirs.as_str()) {
        let response = call(&app, method.clone(), &path, &app.key, body).await;
        mismatched(response, &format!("default on reviewer: {method} {path}")).await;
    }
    for (method, path, body) in event_tools(&app, app.event_ref.as_str()) {
        let response = call(&app, method.clone(), &path, &reviewer_key(), body).await;
        mismatched(response, &format!("reviewer on default: {method} {path}")).await;
    }
    assert!(app.requests.lock().unwrap().is_empty(), "nothing was sent");
}

#[tokio::test]
async fn default_origin_lineage_grants_no_publisher_or_cross_record_authority() {
    let app = routed().await;
    let mut lineage = handoff(&app);
    lineage.path = vec![ConsumerId::default_consumer()];
    let reference = issue(&app, &reviewer(), &reviewer_key(), Some(lineage)).await;
    let path = format!("/v1/events/{}/context", reference.as_str());
    let response = call(&app, Method::GET, &path, &reviewer_key(), None).await;
    assert_eq!(response.status(), 200);
    let context: Value = response.json().await.unwrap();
    assert_eq!(context["consumer"]["id"], reviewer().as_str());
    assert_eq!(context["capabilities"]["buttons"], "none");
    assert_eq!(context["capabilities"]["cards"], "none");

    // Being the sealed chain's publisher never grants access to its recipient.
    for (method, path, body) in event_tools(&app, reference.as_str()) {
        let response = call(&app, method, &path, &app.key, body).await;
        mismatched(response, "default publisher on reviewer reference").await;
    }
    let outsider = "nyx_fixture_another_owner";
    problem(
        call(&app, Method::GET, &path, outsider, None).await,
        403,
        "forbidden",
    )
    .await;

    // Even a gateway-sealed reference cannot substitute another record under
    // the original channel and recipient key.
    let mut payload = reference
        .open(&app.state.keys, super::relay_support::now())
        .unwrap()
        .payload;
    payload.record_id = RecordId::new("another-owner-record").unwrap();
    let substituted = EventRef::seal(&payload, &app.state.keys).unwrap();
    let path = format!("/v1/events/{}/context", substituted.as_str());
    problem(
        call(&app, Method::GET, &path, &reviewer_key(), None).await,
        400,
        "event_ref_invalid",
    )
    .await;
    assert!(app.requests.lock().unwrap().is_empty(), "nothing was sent");
}

#[tokio::test]
async fn a_consumer_key_cannot_update_another_consumers_message() {
    let app = routed().await;
    let path = |reference: &str| format!("/v1/messages/{reference}/update");
    let body = Some(json!({"text": "edited"}));
    let theirs = message_ref(&app, Some(reviewer()));
    let response = call(
        &app,
        Method::POST,
        &path(theirs.as_str()),
        &app.key,
        body.clone(),
    )
    .await;
    mismatched(response, "default updating the reviewer's message").await;
    let ours = message_ref(&app, None);
    let response = call(
        &app,
        Method::POST,
        &path(ours.as_str()),
        &reviewer_key(),
        body,
    )
    .await;
    mismatched(response, "reviewer updating the default's message").await;
    // Through MCP the message is checked as well as the event: the default's
    // own event does not authorize the reviewer's message.
    let refused = app
        .rpc(
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
            "name": "updateMessage", "arguments": {"event_ref": app.event_ref.as_str(),
            "message_ref": theirs.as_str(), "message": {"text": "edited"}}}}),
        )
        .await;
    let refused: Value = refused.json().await.unwrap();
    assert_eq!(refused["error"]["data"]["detail"], "consumer_mismatch");
}

#[tokio::test]
async fn each_consumer_key_is_authorized_for_its_own_references() {
    let app = routed().await;
    let default = ConsumerId::default_consumer();
    for (consumer, key) in [(default, app.key.clone()), (reviewer(), reviewer_key())] {
        let reference = issue(&app, &consumer, &key, None).await;
        // This channel has no relay credential, so a reply target is refused
        // for its destination; reply_targets/consumers.rs creates one per
        // consumer on a channel that has one.
        let tools = event_tools(&app, reference.as_str())
            .into_iter()
            .filter(|(_, path, _)| !path.ends_with("/reply-target"));
        for (method, path, body) in tools {
            let response = call(&app, method.clone(), &path, &key, body).await;
            let status = response.status();
            let text = response.text().await.unwrap();
            assert_eq!(status, 200, "{consumer}: {method} {path}: {text}");
        }
        let owned = message_ref(&app, (!consumer.is_default()).then(|| consumer.clone()));
        let path = format!("/v1/messages/{}/update", owned.as_str());
        let body = Some(json!({"text": "edited"}));
        let response = call(&app, Method::POST, &path, &key, body).await;
        assert_eq!(response.status(), 200, "{consumer}: update");
    }
    let unknown = format!("nyxid_ag_{}", uuid::Uuid::new_v4().simple());
    let path = format!("/v1/events/{}/typing", app.event_ref.as_str());
    let refused = problem(
        call(&app, Method::POST, &path, &unknown, Some(json!({}))).await,
        403,
        "forbidden",
    )
    .await;
    assert_eq!(
        refused["detail"],
        "agent key does not authorize this channel"
    );
}
