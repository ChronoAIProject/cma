use super::*;
use layer3_adapter::{agent_sandbox::RuntimeWorkloadClass, AdapterError};

async fn serve_once(
    response: impl Fn() -> Response + Clone + Send + Sync + 'static,
) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = axum::Router::new().route(
        "/reopen",
        axum::routing::post(move || async move { response() }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{address}/reopen"), server)
}

async fn post(url: &str) -> reqwest::Response {
    reqwest::Client::new().post(url).send().await.unwrap()
}

#[test]
fn only_refusals_the_lifecycle_treats_as_terminal_use_the_closed_code() {
    for error in [
        AdapterError::Rejected("existing conversation tool setup cannot be confirmed".into()),
        AdapterError::Unsupported("capability".into()),
        AdapterError::OperationIdentityConflict("open".into()),
    ] {
        assert!(terminal(&error), "{error}");
    }
    for error in [
        AdapterError::Unavailable("lifecycle reopen authority changed".into()),
        AdapterError::InputAuthorityUnavailable("input".into()),
        AdapterError::AtCapacity("slot".into()),
    ] {
        assert!(!terminal(&error), "{error}");
    }
}

#[tokio::test]
async fn runtime_drift_and_desktop_failure_reach_the_caller_as_terminal_validation() {
    for error in [
        AdapterError::AdmittedSandboxSpecDrift,
        AdapterError::NativePoolConfigurationDrift,
        AdapterError::DesktopFailed,
    ] {
        let (url, server) = serve_once(move || failure_response(error.clone())).await;
        let response = post(&url).await;
        assert_eq!(response.status(), reqwest::StatusCode::CONFLICT);
        assert!(matches!(
            rejected_from(response).await,
            CallFailure::Runtime(AdapterError::Rejected(detail))
                if detail == "Agent owner rejected the lifecycle reopen"
        ));
        server.abort();
    }
}

#[tokio::test]
async fn a_remote_terminal_refusal_arrives_terminal_and_without_its_text() {
    let (url, server) = serve_once(|| rejected_response(REJECTED)).await;
    let response = post(&url).await;
    assert_eq!(response.status(), reqwest::StatusCode::CONFLICT);
    let error = rejected_from(response)
        .await
        .into_adapter("owner unavailable");
    assert!(
        matches!(error, AdapterError::Rejected(ref detail)
            if detail == "Agent owner rejected the lifecycle reopen"),
        "{error}"
    );
    server.abort();
}

#[tokio::test]
async fn a_transient_owner_failure_stays_retryable() {
    let (url, server) = serve_once(|| {
        call_failure::response(AdapterError::Unavailable(
            "lifecycle reopen authority changed".into(),
        ))
    })
    .await;
    let response = post(&url).await;
    assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    let error = CallFailure::remote(response, RuntimeWorkloadClass::Continuity)
        .await
        .into_adapter("owner unavailable");
    assert!(
        matches!(error, AdapterError::Unavailable(ref detail) if detail == "owner unavailable")
    );
    server.abort();
}

#[tokio::test]
async fn a_caller_before_3265_keeps_treating_generic_refusals_as_retryable() {
    // A caller from before #3265 sends every non-success except 429 through
    // `CallFailure::remote`, so a mixed-version rollout behaves as today.
    let (url, server) = serve_once(|| rejected_response(REJECTED)).await;
    let error = CallFailure::remote(post(&url).await, RuntimeWorkloadClass::Continuity)
        .await
        .into_adapter("owner unavailable");
    assert!(matches!(error, AdapterError::Unavailable(_)));
    server.abort();
}

#[tokio::test]
async fn an_unknown_conflict_body_stays_retryable() {
    let (url, server) = serve_once(|| {
        use axum::response::IntoResponse;
        (
            axum::http::StatusCode::CONFLICT,
            Json(serde_json::json!({"code": "something_else"})),
        )
            .into_response()
    })
    .await;
    let error = rejected_from(post(&url).await)
        .await
        .into_adapter("owner unavailable");
    assert!(matches!(error, AdapterError::Unavailable(_)));
    server.abort();
}

#[tokio::test]
async fn reopen_refusal_rechecks_the_dispatched_epoch_after_either_return_path() {
    use crate::agent_manager::relay::tests::{acquire, cluster};
    use layer2_core::agent_ownership::{
        observed_transport, transport_observation_scope, OwnershipStore,
    };
    for remote in [false, true] {
        let (store, instances) = cluster(2).await;
        let id = AgentId(format!("agt_reopen_refusal_{remote}"));
        let (_, owner) = acquire(&store, &instances, &id).await;
        let refusal = || async {
            if remote {
                let (url, server) =
                    serve_once(|| failure_response(AdapterError::Rejected(OUTPUT_REFUSED.into())))
                        .await;
                let failure = rejected_from(post(&url).await).await;
                server.abort();
                failure
            } else {
                CallFailure::Runtime(AdapterError::Rejected(OUTPUT_REFUSED.into()))
            }
        };
        let refused = refusal().await;
        transport_observation_scope(async {
            assert!(matches!(
                verify_refused_owner::<()>(store.as_ref(), &owner.token, true, Err(refused)).await,
                Err(CallFailure::Runtime(AdapterError::Rejected(_)))
            ));
            assert_eq!(observed_transport(&id), Some(owner.token.clone()));
        })
        .await;

        let refused = refusal().await;
        store.release(&owner.token).await.unwrap();
        let successor = instances
            .iter()
            .find(|instance| instance.coordinator.instance_id != owner.token.instance_id)
            .unwrap();
        store
            .heartbeat(&owner.token.instance_id, true)
            .await
            .unwrap();
        let token = store
            .acquire(&id, &successor.coordinator.instance_id)
            .await
            .unwrap()
            .unwrap()
            .token;
        assert!(token.epoch > owner.token.epoch);
        transport_observation_scope(async {
            assert!(matches!(
                verify_refused_owner::<()>(store.as_ref(), &owner.token, true, Err(refused)).await,
                Err(CallFailure::Unavailable)
            ));
            assert_eq!(observed_transport(&id), None);
        })
        .await;
    }
}
