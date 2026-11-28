use super::{execution::*, fixtures::*};
use axum::http::{HeaderValue, StatusCode};
use layer2_core::{
    sandbox::{agent_io::*, store::*, AgentOperationError},
    AgentId,
};
use serde_json::json;
use std::time::Duration;
use tower::ServiceExt;

#[tokio::test]
async fn owner_budget_reset_preserves_the_problem_shape() {
    let reset_at = layer2_core::seconds_from_rfc3339("2026-12-01T00:00:00Z").unwrap();
    let response = crate::conversation::commands::admission_failure(
        CommandLedgerError::OwnerBudgetExhausted { reset_at }.into(),
    );
    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
    assert_eq!(
        response.headers()["content-type"],
        "application/problem+json"
    );
    assert!(response.headers().get("retry-after").is_none());

    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["code"], "owner_budget_exhausted");
    assert_eq!(value.as_object().unwrap().len(), 5);
    assert!(value["detail"]
        .as_str()
        .unwrap()
        .contains("2026-12-01T00:00:00Z"));
}

#[tokio::test]
async fn shared_agui_and_responses_command_refusal_preserves_operator_pause() {
    let response = crate::conversation::commands::admission_failure(CommandAdmissionError::Ledger(
        CommandLedgerError::AdmissionPaused,
    ));
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()["retry-after"], "60");
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["code"], "admission_paused");
    assert_eq!(
        value["detail"],
        layer2_core::admission::ADMISSION_PAUSED_GUIDANCE
    );
}

#[tokio::test]
async fn shared_agui_and_responses_session_capacity_refusal_is_safe_and_retryable() {
    let response = crate::conversation::commands::admission_failure(CommandAdmissionError::Ledger(
        CommandLedgerError::SessionCapacityUnavailable,
    ));
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()["retry-after"], "60");
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["code"], "session_capacity_unavailable");
    assert_eq!(
        value["detail"],
        layer2_core::admission::CapacityKind::ActiveSessions.guidance()
    );
    assert!(!value.to_string().contains("limit"));
    assert!(!value.to_string().contains("count"));
}

#[tokio::test]
async fn a_missing_predecessor_refuses_after_an_older_turn_without_creating_turn_zero() {
    let f = fixture(true).await;
    let task = tokio::spawn(f.router.clone().oneshot(request(
        RESPONSES,
        Some("linear"),
        json!({"conversation":ID,"input":"First only","stream":true}),
    )));
    let record = admitted(&f).await.remove(0);
    finish(&f, "Already happened").await;
    let claimed = claim(&f).await;
    assert!(matches!(
        f.state
            .operations
            .execute_claimed_command(&context("owner-a").caller, &claimed)
            .await,
        Err(AgentOperationError::PreviousTurnConflict)
    ));
    f.store
        .reject_command(
            &claimed.work.lease.unwrap().claim,
            CommandFailureCode::TargetTurnChanged,
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        response.headers()["x-cma-command-id"],
        record.receipt.envelope.command_id.as_str()
    );
    let wire = body(response).await;
    assert!(wire.contains("previous_response_id_conflict"));
    assert!(!wire.contains("response.failed"));
    assert!(f.runtime.0.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn observation_timeout_retains_queued_work_and_known_receipt_without_a_fake_outcome() {
    let f = fixture(false).await;
    let task = tokio::spawn(f.router.clone().oneshot(request(
        RESPONSES,
        Some("uncertain"),
        json!({"conversation":ID,"input":"Still queued","stream":true}),
    )));
    let record = admitted(&f).await.remove(0);
    tokio::time::advance(Duration::from_secs(601)).await;
    let response = task.await.unwrap().unwrap();
    assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
    assert_eq!(
        response.headers()["x-cma-command-id"],
        record.receipt.envelope.command_id.as_str()
    );
    assert!(body(response)
        .await
        .contains("agent_command_observation_timeout"));
    let current = f
        .store
        .get_command(&record.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.status, AgentResponseStatus::Queued);
    assert!(current.work.failure.is_none() && current.work.dispatch.is_none());
    assert!(f.runtime.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn committed_prebinding_failures_keep_typed_http_errors_and_receipt_headers() {
    for (failure, expected, code) in [
        (
            CommandFailureCode::QueueTimeout,
            StatusCode::GATEWAY_TIMEOUT,
            "agent_command_queue_timeout",
        ),
        (
            CommandFailureCode::AuthorityRevoked,
            StatusCode::FORBIDDEN,
            "agent_command_authority_revoked",
        ),
        (
            CommandFailureCode::AuthorityUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
            "agent_command_authority_unavailable",
        ),
        (
            CommandFailureCode::CredentialSetupRequired,
            StatusCode::CONFLICT,
            "agent_command_execution_failed",
        ),
        (
            CommandFailureCode::ConnectionsRequired,
            StatusCode::CONFLICT,
            "agent_command_execution_failed",
        ),
        (
            CommandFailureCode::AgentUnavailable,
            StatusCode::CONFLICT,
            "agent_unavailable",
        ),
        (
            CommandFailureCode::RuntimeUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
            "runtime_unavailable",
        ),
        (
            CommandFailureCode::RuntimeLost,
            StatusCode::CONFLICT,
            "runtime_lost",
        ),
        (
            CommandFailureCode::OwnerBudgetExhausted,
            StatusCode::PAYMENT_REQUIRED,
            "owner_budget_exhausted",
        ),
        (
            CommandFailureCode::TurnBudgetExhausted,
            StatusCode::PAYMENT_REQUIRED,
            "turn_budget_exhausted",
        ),
        (
            CommandFailureCode::BudgetUnavailable,
            StatusCode::PAYMENT_REQUIRED,
            "budget_unavailable",
        ),
        (
            CommandFailureCode::SessionCapacityUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
            "session_capacity_unavailable",
        ),
        (
            CommandFailureCode::ExecutionFailed,
            StatusCode::CONFLICT,
            "agent_command_execution_failed",
        ),
    ] {
        let f = fixture(false).await;
        let task = tokio::spawn(f.router.clone().oneshot(request(
            RESPONSES,
            Some("refused"),
            json!({"conversation":ID,"input":"Refused","stream":true}),
        )));
        let record = admitted(&f).await.remove(0);
        let at = if failure == CommandFailureCode::QueueTimeout {
            record.queue_deadline.clone()
        } else {
            layer2_core::now_rfc3339()
        };
        f.store
            .reject_unclaimed_command(
                &record.receipt.envelope.command_id,
                record.revision,
                failure,
                at,
            )
            .await
            .unwrap();
        let response = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), expected);
        assert_eq!(
            response.headers()["x-cma-command-id"],
            record.receipt.envelope.command_id.as_str()
        );
        let wire = body(response).await;
        assert!(wire.contains(code), "{wire}");
        assert!(!wire.contains("response.failed") && !wire.contains("RUN_STARTED"));
        assert!(f.runtime.0.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn malformed_keys_and_missing_resource_context_cannot_fall_back_to_direct_dispatch() {
    let f = fixture(false).await;
    let input = json!({"threadId":ID,"messages":[{"role":"user","content":"Hello"}]});
    for key in ["", "\t", &"x".repeat(257)] {
        let response = f
            .router
            .clone()
            .oneshot(request(AGUI, Some(key), input.clone()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
    let mut duplicate = request(AGUI, Some("one"), input.clone());
    duplicate
        .headers_mut()
        .append("idempotency-key", HeaderValue::from_static("two"));
    assert_eq!(
        f.router.clone().oneshot(duplicate).await.unwrap().status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let mut missing = request(AGUI, Some("valid"), input);
    missing
        .extensions_mut()
        .remove::<layer2_core::resource_access::CallerContext>();
    assert_eq!(
        f.router.clone().oneshot(missing).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert!(f
        .store
        .command_work_page(&PageRequest::default())
        .await
        .unwrap()
        .items
        .is_empty());
    assert!(f.runtime.0.lock().unwrap().is_empty());
    assert_eq!(
        f.store
            .get_agent(&AgentId(ID.into()))
            .await
            .unwrap()
            .unwrap()
            .conversation
            .last_seq,
        2
    );
}

#[tokio::test]
async fn identical_unkeyed_runs_are_distinct_and_run_id_stays_correlation_only() {
    let f = fixture(false).await;
    let input = json!({"threadId":ID,"runId":"shared-correlation","messages":[{"role":"user","content":"Same"}]});
    let one = tokio::spawn(f.router.clone().oneshot(request(AGUI, None, input.clone())));
    admitted(&f).await;
    let two = tokio::spawn(f.router.clone().oneshot(request(AGUI, None, input)));
    let records = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let records = admitted(&f).await;
            if records.len() == 2 {
                break records;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    one.abort();
    two.abort();
    assert_ne!(records[0].key_sha256, records[1].key_sha256);
    assert_ne!(
        records[0].receipt.response_id,
        records[1].receipt.response_id
    );
    assert!(f.runtime.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn platform_exhaustion_is_a_402_problem_with_the_spec_guidance() {
    use axum::response::IntoResponse;
    use layer2_core::budget::BudgetError;
    let reason = BudgetError::PlatformExhausted;
    let admission = crate::rest::instance_problem::InstanceProblem::Budget(reason).into_response();
    assert_eq!(admission.status(), StatusCode::PAYMENT_REQUIRED);
    assert!(admission.headers().get("retry-after").is_none());
    let wire = body(admission).await;
    assert!(wire.contains("platform_budget_exhausted"), "{wire}");
    assert!(wire.contains("send a new message to continue"), "{wire}");

    let f = fixture(false).await;
    let task = tokio::spawn(f.router.clone().oneshot(request(
        RESPONSES,
        Some("platform-stopped"),
        json!({"conversation":ID,"input":"Refused","stream":true}),
    )));
    let record = admitted(&f).await.remove(0);
    f.store
        .reject_unclaimed_command(
            &record.receipt.envelope.command_id,
            record.revision,
            CommandFailureCode::PlatformBudgetExhausted,
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
    assert!(response.headers().get("retry-after").is_none());
    let wire = body(response).await;
    assert!(wire.contains("platform_budget_exhausted"), "{wire}");
    assert!(wire.contains(reason.guidance()), "{wire}");
}
