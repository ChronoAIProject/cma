use std::sync::atomic::{AtomicUsize, Ordering};

use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use layer2_core::bridge::ingress::*;
use layer2_core::bridge::*;
use serde_json::{json, Value};
use tokio::sync::{Mutex, Notify};
use tower::ServiceExt;

use super::*;
use crate::bridge::ingress::capabilities::tests::fixtures::Fixture;

mod boundaries;
mod controls;
mod fixture;
mod reply_addressing;

use fixture::*;

#[tokio::test]
async fn genuine_signed_person_and_org_callbacks_ack_only_after_commit_and_hold_execution() {
    for organization in [false, true] {
        let fixture = HttpFixture::new(organization);
        let provider = fixture.provider.clone();
        let request = fixture.request();
        let app = fixture.router();
        let pending = tokio::spawn(async move { app.oneshot(request).await.unwrap() });
        provider.entered.notified().await;
        assert!(!pending.is_finished());
        assert!(provider.rows.lock().await.is_empty());
        provider.commits.add_permits(1);
        let response = pending.await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let value = value(response).await;
        assert_eq!(value["status"], "inbox_persisted");
        assert_eq!(value["receipt"]["admission"], "accepted");
        assert_eq!(value["receipt"]["execution_hold"], "worker_unavailable");
        let rows = provider.rows.lock().await;
        let row = rows.values().next().unwrap();
        assert_eq!(row.owner, fixture.protocol.bridge.owner);
        assert_eq!(row.envelope.conversation_type, "group");
        assert!(row.capabilities.is_some());
        assert_eq!(
            row.execution_hold,
            Some(InboxExecutionHold::WorkerUnavailable)
        );
    }
}

#[tokio::test]
async fn duplicate_returns_retained_receipt_and_changed_signed_content_conflicts() {
    let fixture = HttpFixture::new(false);
    fixture.provider.commits.add_permits(3);
    let first = value(fixture.router().oneshot(fixture.request()).await.unwrap()).await;
    let again = value(fixture.router().oneshot(fixture.request()).await.unwrap()).await;
    assert_eq!(first, again);
    assert_eq!(fixture.provider.rows.lock().await.len(), 1);
    let mut changed = fixture.protocol.clone();
    changed.body["content"]["text"] = json!("changed signed text");
    changed.raw_mut()["message"]["text"] = json!("changed signed text");
    let response = fixture.router().oneshot(request(&changed)).await.unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(value(response).await["code"], "callback_replay_conflict");
    assert_eq!(fixture.provider.rows.lock().await.len(), 1);
}

#[tokio::test]
async fn durable_policy_denial_has_no_execution_hold_or_reply_custody() {
    let mut fixture = HttpFixture::new(false);
    fixture.protocol.raw_mut()["message"]["from"]["is_bot"] = json!(true);
    fixture.protocol.body["thread_id"] = json!("interaction:app:discard-fixture-secret");
    fixture.provider.commits.add_permits(1);
    let response = fixture.router().oneshot(fixture.request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let result = value(response).await;
    assert_eq!(
        result["receipt"]["admission"],
        json!({"partition_denied":{"reason":"bot_sender"}})
    );
    assert!(result["receipt"].get("execution_hold").is_none());
    assert!(!result.to_string().contains("discard-fixture-secret"));
    let rows = fixture.provider.rows.lock().await;
    let row = rows.values().next().unwrap();
    assert!(row.reply_expires_at.is_none());
    assert!(!row.envelope.protected_thread_context_present);
}

#[tokio::test]
async fn unknown_persistence_result_is_503_and_retry_reconciles_committed_receipt() {
    let fixture = HttpFixture::new(false);
    fixture.provider.failure.store(2, Ordering::SeqCst);
    fixture.provider.commits.add_permits(2);
    let response = fixture.router().oneshot(fixture.request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        value(response).await,
        json!({"code":"callback_acceptance_unknown","acceptance":"unknown"})
    );
    assert_eq!(fixture.provider.rows.lock().await.len(), 1);
    fixture.provider.failure.store(0, Ordering::SeqCst);
    let response = fixture.router().oneshot(fixture.request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(fixture.provider.rows.lock().await.len(), 1);
}

async fn value(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap()
}
