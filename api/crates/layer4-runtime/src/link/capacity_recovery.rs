use super::*;

pub(super) async fn capacity_recovery(
    Path(agent_id): Path<String>,
    State(supervisor): State<Arc<Supervisor>>,
    Json(request): Json<CapacityRecoveryRequest>,
) -> Response {
    if request.target.agent_id != agent_id {
        return protocol_failure(
            StatusCode::CONFLICT,
            "capacity_recovery_identity",
            "capacity recovery request names another Agent",
            false,
        );
    }
    match supervisor.capacity_recovery(request).await {
        Ok(proof) => Json(proof).into_response(),
        Err(error) if error.code == "capacity_recovery_unsupported" => {
            protocol_failure(StatusCode::NOT_IMPLEMENTED, error.code, error.detail, false)
        }
        Err(error) => runtime_failure(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt as _;

    #[tokio::test]
    async fn disabled_recovery_endpoint_returns_typed_unsupported() {
        let home = std::env::temp_dir().join(format!("cma-recovery-off-{}", uuid::Uuid::new_v4()));
        let config = crate::config::RuntimeConfig::for_test(home.clone());
        let supervisor =
            Supervisor::new(config, crate::writer_fence::SupervisorBoundary::default()).unwrap();
        let body = serde_json::json!({
            "operation_key":"recovery", "writer_fence":1, "challenge":"a".repeat(32),
            "target":{
                "sandbox_id":"sandbox", "agent_id":"agent", "runtime":"runtime", "boot_id":"b".repeat(32),
                "provision_generation":1, "home_identity":"a".repeat(43), "conversation_id":"conversation",
                "conversation_generation":1, "original_open_operation":"agent:agent:open:v1",
                "original_open_sha256":"c".repeat(64), "active_turn":null
            }
        });
        let response = router(supervisor)
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/runtime/agents/agent/capacity-recovery")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        assert!(std::str::from_utf8(&bytes)
            .unwrap()
            .contains("capacity_recovery_unsupported"));
        for root in [
            &home,
            &home.with_extension("checkpoints"),
            &home.with_extension("runtime-state"),
        ] {
            if root.exists() {
                std::fs::remove_dir_all(root).unwrap();
            }
        }
    }
}
