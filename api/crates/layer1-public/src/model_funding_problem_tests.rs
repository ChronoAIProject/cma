use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use layer2_core::sandbox::{model_credit::FundingError, AgentOperationError};

pub(crate) async fn assert_funding_contract(
    project: impl Fn(AgentOperationError) -> Response,
    responses: bool,
) {
    use FundingError::*;
    let cases = [
        (
            Unsupported,
            503,
            "model_funding_unsupported",
            "not supported",
        ),
        (
            Insufficient,
            402,
            "model_funding_insufficient",
            "manages model funding",
        ),
        (Unavailable, 503, "model_funding_unavailable", "original"),
        (Expired, 409, "model_funding_expired", "expired"),
        (
            BindingChanged,
            409,
            "model_funding_binding_changed",
            "selection",
        ),
        (
            NoReservation,
            503,
            "model_funding_reconciliation_required",
            "original",
        ),
        (
            CheckpointChanged,
            409,
            "model_funding_checkpoint_conflict",
            "changed",
        ),
        (
            ReconciliationRequired,
            503,
            "model_funding_reconciliation_required",
            "original",
        ),
        (
            Forbidden,
            403,
            if responses {
                "response_forbidden"
            } else {
                "forbidden"
            },
            if responses { "permit" } else { "authority" },
        ),
    ];
    let mut failures = Vec::new();
    for (reason, expected_status, expected_code, guidance) in cases {
        let mut inputs = vec![AgentOperationError::ModelFunding(reason)];
        if reason == Unsupported {
            inputs.push(AgentOperationError::ModelFundingUnsupported);
        }
        for input in inputs {
            let response = project(input);
            let status = response.status().as_u16();
            let retry_after = response.headers().contains_key("retry-after");
            let content_type = response.headers()["content-type"]
                .to_str()
                .unwrap()
                .to_owned();
            let bytes = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let (code, message) = if responses {
                assert_eq!(content_type, "application/json");
                assert!(body["error"]["param"].is_null());
                assert_eq!(
                    body["error"]["type"],
                    if status >= 500 {
                        "server_error"
                    } else {
                        "invalid_request_error"
                    }
                );
                (&body["error"]["code"], &body["error"]["message"])
            } else {
                assert_eq!(content_type, "application/problem+json");
                assert_eq!(body["status"], status);
                assert!(body["type"].as_str().unwrap().starts_with("https://"));
                (&body["code"], &body["detail"])
            };
            if status != expected_status
                || code != expected_code
                || retry_after
                || !message.as_str().unwrap().contains(guidance)
            {
                failures.push(format!("{reason:?}: expected {expected_status}/{expected_code}, no Retry-After and guidance containing {guidance:?}; got {status}, Retry-After={retry_after}, {body}"));
            }
            // A reason alone cannot prove a prior request was never admitted.
            assert!(body.get("admission").is_none());
            assert!(body["error"].get("admission").is_none());
            assert!(!message.as_str().unwrap().contains("new Idempotency-Key"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn rest_model_funding_contract() {
    assert_funding_contract(
        |value| crate::rest::instance_problem::InstanceProblem::from(value).into_response(),
        false,
    )
    .await;
}

pub(crate) async fn assert_invalid_contract(
    project: impl Fn(AgentOperationError) -> Response,
    expected_code: &str,
) {
    let response = project(AgentOperationError::InvalidRequest(
        "synthetic-private-input".into(),
    ));
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], expected_code);
    assert!(!body.to_string().contains("synthetic-private-input"));
}
