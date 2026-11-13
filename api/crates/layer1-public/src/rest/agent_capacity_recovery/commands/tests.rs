use super::*;
use crate::rest::{agents_tests, resource_access_tests::context};
use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request},
};
use layer2_core::{
    sandbox::{
        agent_capacity_recovery::state::{RecoveryPhase, RecoveryTarget},
        agent_capacity_recovery::{commands::RecoveryStatus, CapacityRecoveryDisposition},
        AgentWorkReceipt, AgentWorkStep, RuntimeBinding,
    },
    Capability,
};
use serde_json::Value;
use std::sync::Mutex;
use tower::ServiceExt;

const BASE: &str = "/api/v1/agents/recovery-agent/capacity-recovery";
const NOW: &str = "2026-11-11T00:00:00Z";

fn original() -> RecoveryRequest {
    RecoveryRequest {
        agent_id: AgentId("recovery-agent".into()),
        creation_key: "creation".into(),
        creation_fingerprint: "a".repeat(64),
        failure_seq: 10_001,
        requested_at: NOW.into(),
    }
}
fn completed() -> RecoveryWork {
    let request = original();
    RecoveryWork {
        format_version: 1,
        target: RecoveryTarget {
            agent_id: request.agent_id.clone(),
            creation_key: request.creation_key.clone(),
            creation_fingerprint: request.creation_fingerprint.clone(),
            conversation_id: "private-native".into(),
            conversation_generation: 1,
            runtime: RuntimeBinding {
                runtime_id: "private-runtime".into(),
                runtime_version: "test".into(),
                boot_id: Some("b".repeat(32)),
                provision_generation: Some(1),
                bound_at: NOW.into(),
            },
            home_identity: "private-home".into(),
            original_open: AgentWorkReceipt {
                step: AgentWorkStep::OpenConversation,
                operation_id: "agent:recovery-agent:open:v1".into(),
                request_sha256: "c".repeat(64),
                completed_at: NOW.into(),
            },
            active_turn: None,
            input_sha256: "d".repeat(64),
        },
        request,
        phase: RecoveryPhase::Completed,
        revision: 3,
        attempts: 1,
        next_fence: 8,
        lease: None,
        ready_at: NOW.into(),
        completed_at: Some(NOW.into()),
    }
}
struct Commands {
    calls: Mutex<Vec<&'static str>>,
    result: Mutex<Result<CommitOutcome<RecoveryWork>, RecoveryError>>,
}
impl Commands {
    fn new() -> Self {
        Self {
            calls: Mutex::new(vec![]),
            result: Mutex::new(Ok(CommitOutcome::Recorded(completed()))),
        }
    }
    fn authorize(&self, caller: &Caller, method: &'static str) -> Result<(), RecoveryError> {
        if caller.subject.as_str() != "owner-a"
            || !caller.authorization.allows(Capability::UseOwnSandboxes)
        {
            return Err(RecoveryError::Unauthorized);
        }
        self.calls.lock().unwrap().push(method);
        Ok(())
    }
}
#[async_trait::async_trait]
impl CapacityRecoveryCommands for Commands {
    async fn status(
        &self,
        caller: &Caller,
        agent_id: &AgentId,
        request: Option<&RecoveryRequest>,
    ) -> Result<RecoveryStatus, RecoveryError> {
        self.authorize(caller, "status")?;
        if let Some(request) = request {
            assert_eq!(request, &original());
        }
        Ok(RecoveryStatus {
            agent_id: agent_id.clone(),
            disposition: CapacityRecoveryDisposition::EvidenceRequired,
            phase: request.map(|_| RecoveryPhase::Completed),
            attempts: 1,
            retry_at: None,
            operation_key: request.map(|r| r.key().unwrap()),
        })
    }
    async fn admit(
        &self,
        caller: &Caller,
        request: &RecoveryRequest,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
        self.authorize(caller, "admit")?;
        assert_eq!(request, &original());
        Err(RecoveryError::Disabled)
    }
    async fn resume(
        &self,
        caller: &Caller,
        request: &RecoveryRequest,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
        self.authorize(caller, "resume")?;
        assert_eq!(request, &original());
        self.result.lock().unwrap().clone()
    }
}
fn request(action: &str, body: Value) -> Request<Body> {
    agents_tests::request(
        Method::POST,
        &format!("{BASE}/{action}"),
        "owner-a",
        None,
        Some(body),
    )
}
async fn response(router: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = router.clone().oneshot(request).await.unwrap();
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    let status = response.status();
    let body = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
    let value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({"extractor_error": true}));
    (status, value)
}

#[tokio::test]
async fn accepted_resume_bypasses_disabled_admission_and_redacts_native_facts() {
    let commands = Arc::new(Commands::new());
    let router = router(commands.clone());
    let (status, value) = response(&router, request("admit", json!(original()))).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(value["code"], "capacity_recovery_disabled");
    for _ in 0..2 {
        let (status, value) = response(&router, request("resume", json!(original()))).await;
        assert_eq!(status, StatusCode::OK, "{value}");
        assert_eq!(value["outcome"], "recorded");
        assert_eq!(value["operation_key"], original().key().unwrap());
        let mut keys: Vec<_> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "attempts",
                "completed_at",
                "operation_key",
                "outcome",
                "phase",
                "retry_at"
            ]
        );
        assert!(!value.to_string().contains("private-"));
    }
    assert_eq!(
        *commands.calls.lock().unwrap(),
        ["admit", "resume", "resume"]
    );
}

#[tokio::test]
async fn status_passes_only_the_original_request_and_current_caller() {
    let commands = Arc::new(Commands::new());
    let router = router(commands.clone());
    let get = agents_tests::request(Method::GET, BASE, "owner-a", None, None);
    assert_eq!(response(&router, get).await.0, StatusCode::OK);
    let (status, value) = response(&router, request("status", json!(original()))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["operation_key"], original().key().unwrap());
    assert_eq!(*commands.calls.lock().unwrap(), ["status", "status"]);
}

#[tokio::test]
async fn invalid_or_scoped_authentication_never_calls_recovery_commands() {
    let commands = Arc::new(Commands::new());
    let router = router(commands.clone());
    let anonymous = Request::builder().uri(BASE).body(Body::empty()).unwrap();
    assert_eq!(
        response(&router, anonymous).await.0,
        StatusCode::UNAUTHORIZED
    );
    let foreign = agents_tests::request(
        Method::POST,
        &format!("{BASE}/resume"),
        "outsider",
        None,
        Some(json!(original())),
    );
    assert_eq!(response(&router, foreign).await.0, StatusCode::NOT_FOUND);
    let mut binding = request("resume", json!(original()));
    binding.extensions_mut().insert(AuthMethod::BindingKey {
        binding_id: "binding".into(),
    });
    assert_eq!(response(&router, binding).await.0, StatusCode::NOT_FOUND);
    for source in [
        CredentialSource::Binding,
        CredentialSource::RequestBoundRead {
            expires_at: std::time::SystemTime::now() + std::time::Duration::from_secs(60),
        },
        CredentialSource::Independent {
            expires_at: Some(std::time::SystemTime::UNIX_EPOCH),
        },
    ] {
        let mut req = request("resume", json!(original()));
        let mut context = context("owner-a");
        context.credential_source = source;
        req.extensions_mut().insert(context);
        assert!(!response(&router, req).await.0.is_success());
    }
    assert!(commands.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn path_mismatch_input_payload_invalid_clock_and_oversized_body_are_refused() {
    let commands = Arc::new(Commands::new());
    let router = router(commands.clone());
    for (field, value) in [
        ("agent_id", json!("different")),
        ("requested_at", json!("2026-02-29T00:00:00Z")),
        ("input", json!("must never replay")),
        ("creation_key", json!("x".repeat(5000))),
    ] {
        let mut body = json!(original());
        body[field] = value;
        assert!(!response(&router, request("resume", body))
            .await
            .0
            .is_success());
    }
    assert!(commands.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn ambiguous_abort_and_failed_activation_have_distinct_unsuccessful_responses() {
    let commands = Arc::new(Commands::new());
    let router = router(commands.clone());
    for (result, expected, field, value) in [
        (
            Ok(CommitOutcome::Unknown),
            StatusCode::ACCEPTED,
            "outcome",
            "unknown",
        ),
        (
            Ok(CommitOutcome::KnownAbort),
            StatusCode::CONFLICT,
            "outcome",
            "known_abort",
        ),
        (
            Err(RecoveryError::Unavailable),
            StatusCode::SERVICE_UNAVAILABLE,
            "code",
            "capacity_recovery_unavailable",
        ),
        (
            Err(RecoveryError::Conflict),
            StatusCode::CONFLICT,
            "code",
            "capacity_recovery_conflict",
        ),
    ] {
        *commands.result.lock().unwrap() = result;
        let (status, body) = response(&router, request("resume", json!(original()))).await;
        assert_eq!(status, expected);
        assert_eq!(body[field], value);
    }
    assert_eq!(*commands.calls.lock().unwrap(), ["resume"; 4]);
}
