//! Launch command receipts across concurrency, cancellation and completion failures.
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::http::{Method, StatusCode};
use layer2_core::sandbox::{SandboxCredentialPrerequisite, SandboxCredentialPrerequisiteError};
use layer2_core::trigger_domain::ServiceRequirementState;
use layer2_core::{OperationReservation, SubjectId};
use serde_json::{json, Value};
use tokio::sync::Semaphore;
use tokio::time::timeout;

use super::support_tests::*;

struct Gate {
    setup_required: AtomicBool,
    calls: AtomicUsize,
    block_next: AtomicBool,
    entered: Semaphore,
    release: Semaphore,
}

impl Gate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            setup_required: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
            block_next: AtomicBool::new(false),
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
        })
    }
    fn arm(&self) {
        self.block_next.store(true, Ordering::SeqCst);
    }
    async fn wait_entered(&self) {
        timeout(Duration::from_secs(5), self.entered.acquire())
            .await
            .expect("Start reached its prerequisite")
            .unwrap()
            .forget();
    }
}

#[async_trait::async_trait]
impl SandboxCredentialPrerequisite for Gate {
    async fn check(&self, owner: &SubjectId) -> Result<(), SandboxCredentialPrerequisiteError> {
        assert_eq!(owner, &SubjectId::new(VISITOR));
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.block_next.swap(false, Ordering::SeqCst) {
            self.entered.add_permits(1);
            self.release.acquire().await.unwrap().forget();
        }
        if self.setup_required.load(Ordering::SeqCst) {
            return Err(SandboxCredentialPrerequisiteError::SetupRequired);
        }
        Ok(())
    }
}

async fn draft(gate: Arc<Gate>) -> (TestApp, String) {
    let app = TestApp::with_credential_prerequisite(gate).await;
    app.broker
        .set_requirement(ServiceRequirementState::Satisfied);
    let trigger = create_trigger(&app, "always_new", "fence-trigger").await;
    let published = publish_trigger(&app, trigger["id"].as_str().unwrap(), "fence-publish").await;
    let (status, launch) = json_response(
        &app.router,
        visitor_request(
            Method::POST,
            &format!(
                "/api/v1/triggers/public/{}/launches",
                published["public_id"].as_str().unwrap()
            ),
            Some(json!({"start_if_ready":false})),
            Some("fence-draft"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{launch}");
    assert_eq!(launch["state"], "draft");
    (app, launch["id"].as_str().unwrap().to_owned())
}

async fn read(app: &TestApp, id: &str) -> Value {
    let (status, value) = json_response(
        &app.router,
        visitor_request(Method::GET, &format!("/api/v1/launches/{id}"), None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    value
}

async fn refused(app: &TestApp, path: &str, key: &str, status: StatusCode, code: &str) {
    let (actual, problem) = timeout(
        Duration::from_secs(5),
        json_response(
            &app.router,
            visitor_request(Method::POST, path, Some(json!({})), Some(key)),
        ),
    )
    .await
    .expect("duplicate request did not block on the original request");
    assert_eq!(actual, status, "{problem}");
    assert_eq!(problem["code"], code);
}

#[tokio::test]
async fn concurrent_same_key_is_fenced_then_replays_the_original_result() {
    let gate = Gate::new();
    let (app, id) = draft(gate.clone()).await;
    let path = format!("/api/v1/launches/{id}/start");
    let before = read(&app, &id).await;
    gate.arm();
    let router = app.router.clone();
    let first_path = path.clone();
    let first = tokio::spawn(async move {
        raw_response(
            &router,
            visitor_request(
                Method::POST,
                &first_path,
                Some(json!({})),
                Some("concurrent"),
            ),
        )
        .await
    });
    gate.wait_entered().await;
    let calls = gate.calls.load(Ordering::SeqCst);
    refused(
        &app,
        &path,
        "concurrent",
        StatusCode::CONFLICT,
        "idempotency_in_flight",
    )
    .await;
    let (status, problem) = json_response(
        &app.router,
        visitor_request(
            Method::POST,
            &path,
            Some(json!({"acknowledgements":["changed"]})),
            Some("concurrent"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(problem["code"], "idempotency_key_reused");
    assert_eq!(gate.calls.load(Ordering::SeqCst), calls);
    assert_eq!(read(&app, &id).await, before);
    gate.release.add_permits(1);
    let accepted = timeout(Duration::from_secs(5), first)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(accepted.0, StatusCode::ACCEPTED);
    let replay = raw_response(
        &app.router,
        visitor_request(Method::POST, &path, Some(json!({})), Some("concurrent")),
    )
    .await;
    assert_eq!(replay, accepted);
    assert_eq!(gate.calls.load(Ordering::SeqCst), calls);
}

#[tokio::test]
async fn cancelled_execution_stays_pending_and_recovery_required_never_reexecutes() {
    let gate = Gate::new();
    let (app, id) = draft(gate.clone()).await;
    let path = format!("/api/v1/launches/{id}/start");
    let before = read(&app, &id).await;
    gate.arm();
    let router = app.router.clone();
    let first_path = path.clone();
    let first = tokio::spawn(async move {
        raw_response(
            &router,
            visitor_request(
                Method::POST,
                &first_path,
                Some(json!({})),
                Some("cancelled"),
            ),
        )
        .await
    });
    gate.wait_entered().await;
    let calls = gate.calls.load(Ordering::SeqCst);
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    refused(
        &app,
        &path,
        "cancelled",
        StatusCode::CONFLICT,
        "idempotency_in_flight",
    )
    .await;
    assert_eq!(read(&app, &id).await, before);
    // The memory store has no lease clock: inject the real store's result to test only the route.
    app.fail_operations(Ok(OperationReservation::RecoveryRequired));
    refused(
        &app,
        &path,
        "cancelled",
        StatusCode::CONFLICT,
        "idempotency_recovery_required",
    )
    .await;
    assert_eq!(read(&app, &id).await, before);
    assert_eq!(gate.calls.load(Ordering::SeqCst), calls);
}

#[tokio::test]
async fn failure_before_receipt_write_can_follow_a_successful_launch_transition() {
    let gate = Gate::new();
    let (app, id) = draft(gate.clone()).await;
    let path = format!("/api/v1/launches/{id}/start");
    let before = read(&app, &id).await;
    app.fail_completion(Some(CompletionFault::BeforeWrite));
    refused(
        &app,
        &path,
        "write-failed",
        StatusCode::SERVICE_UNAVAILABLE,
        "store_failed",
    )
    .await;
    let advanced = read(&app, &id).await;
    assert_eq!(advanced["state"], "resolving");
    assert_ne!(advanced["revision"], before["revision"]);
    let calls = gate.calls.load(Ordering::SeqCst);
    app.fail_completion(None);
    refused(
        &app,
        &path,
        "write-failed",
        StatusCode::CONFLICT,
        "idempotency_in_flight",
    )
    .await;
    refused(
        &app,
        &path,
        "replacement",
        StatusCode::CONFLICT,
        "launch_not_startable",
    )
    .await;
    assert_eq!(read(&app, &id).await, advanced);
    assert_eq!(gate.calls.load(Ordering::SeqCst), calls);
    app.fail_operations(Ok(OperationReservation::RecoveryRequired));
    refused(
        &app,
        &path,
        "write-failed",
        StatusCode::CONFLICT,
        "idempotency_recovery_required",
    )
    .await;
    assert_eq!(read(&app, &id).await, advanced);
}

#[tokio::test]
async fn lost_receipt_write_ack_returns_503_but_same_key_recovers_the_saved_202() {
    let gate = Gate::new();
    let (app, id) = draft(gate.clone()).await;
    let path = format!("/api/v1/launches/{id}/start");
    app.fail_completion(Some(CompletionFault::AfterWrite));
    refused(
        &app,
        &path,
        "ack-lost",
        StatusCode::SERVICE_UNAVAILABLE,
        "store_failed",
    )
    .await;
    let advanced = read(&app, &id).await;
    assert_eq!(advanced["state"], "resolving");
    let calls = gate.calls.load(Ordering::SeqCst);
    app.fail_completion(None);
    let saved = raw_response(
        &app.router,
        visitor_request(Method::POST, &path, Some(json!({})), Some("ack-lost")),
    )
    .await;
    assert_eq!(saved.0, StatusCode::ACCEPTED);
    let body: Value = serde_json::from_slice(&saved.2).unwrap();
    assert_eq!(body["state"], advanced["state"]);
    assert_eq!(body["revision"], advanced["revision"]);
    assert_eq!(body["event_head"], advanced["event_head"]);
    let replay = raw_response(
        &app.router,
        visitor_request(Method::POST, &path, Some(json!({})), Some("ack-lost")),
    )
    .await;
    assert_eq!(replay, saved);
    assert_eq!(read(&app, &id).await, advanced);
    assert_eq!(gate.calls.load(Ordering::SeqCst), calls);
}

#[tokio::test]
async fn credential_refusal_replays_after_setup_and_a_new_command_can_start() {
    let gate = Gate::new();
    let (app, id) = draft(gate.clone()).await;
    let path = format!("/api/v1/launches/{id}/start");
    let before = read(&app, &id).await;
    gate.setup_required.store(true, Ordering::SeqCst);
    let refusal = raw_response(
        &app.router,
        visitor_request(Method::POST, &path, Some(json!({})), Some("before-setup")),
    )
    .await;
    assert_eq!(refusal.0, StatusCode::CONFLICT);
    let problem: Value = serde_json::from_slice(&refusal.2).unwrap();
    assert_eq!(problem["code"], "sandbox_credential_setup_required");
    assert_eq!(read(&app, &id).await, before);
    let calls = gate.calls.load(Ordering::SeqCst);

    gate.setup_required.store(false, Ordering::SeqCst);
    let replay = raw_response(
        &app.router,
        visitor_request(Method::POST, &path, Some(json!({})), Some("before-setup")),
    )
    .await;
    assert_eq!(replay, refusal);
    assert_eq!(gate.calls.load(Ordering::SeqCst), calls);
    assert_eq!(read(&app, &id).await, before);

    let (status, accepted) = json_response(
        &app.router,
        visitor_request(Method::POST, &path, Some(json!({})), Some("after-setup")),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    assert_eq!(accepted["state"], "resolving");
    assert_eq!(gate.calls.load(Ordering::SeqCst), calls + 1);
}
