//! The actual model-catalogue client, without shortening its timeout in tests.
use super::*;
use crate::dependency_test_support::{assert_deadline, finish, Peer};
use axum::http::{header, Method};
use layer2_core::dependency_diagnostics::capture;
use serde_json::json;

#[tokio::test]
async fn model_catalogue_contract_preserves_scoped_auth_and_typed_503_without_retry() {
    for success in [true, false] {
        let mut peer = Peer::start().await;
        let catalog =
            ChronoAgentModels::new(&peer.origin, Arc::new(super::tests::NoCredentials)).unwrap();
        let task = tokio::spawn(async move {
            capture(catalog.fetch("chrono-llm-public", "contract-scoped-fixture")).await
        });
        let request = peer.next().await;
        assert_eq!(request.method, Method::GET);
        assert_eq!(
            request.uri.path(),
            "/api/v1/proxy/s/chrono-llm-public/models"
        );
        assert!(request.uri.query().is_none());
        assert_eq!(
            request.headers[header::AUTHORIZATION],
            "Bearer contract-scoped-fixture"
        );
        if success {
            request.json(json!({"data":[{"id":"gpt-6","display_name":"Fixture model"}]}));
            let (models, failure) = finish(task).await;
            let models = models.unwrap();
            assert_eq!(models.len(), 1);
            assert_eq!(models[0].id, "gpt-6");
            assert_eq!(models[0].label, "Fixture model");
            assert!(!models[0].available);
            assert_eq!(failure, None);
        } else {
            request.respond(503, "synthetic unavailable");
            let (result, failure) = finish(task).await;
            assert!(matches!(result, Err(AgentModelCatalogError::Unavailable)));
            assert_eq!(
                failure,
                Some(Failure::new(
                    Dependency::ModelGateway,
                    Reason::UpstreamUnavailable
                ))
            );
        }
        assert_eq!(peer.count(), 1);
    }
}

#[tokio::test]
async fn model_catalogue_stalled_headers_and_body_fail_at_fifteen_seconds() {
    for body in [false, true] {
        let mut peer = Peer::start().await;
        let catalog =
            ChronoAgentModels::new(&peer.origin, Arc::new(super::tests::NoCredentials)).unwrap();
        let task = tokio::spawn(async move {
            capture(catalog.fetch("chrono-llm-public", "contract-scoped-fixture")).await
        });
        let request = peer.next().await;
        let held = if body {
            request.stall_body();
            None
        } else {
            Some(request)
        };
        tokio::time::pause();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        tokio::time::advance(Duration::from_secs(15)).await;
        let (result, failure) = finish(task).await;
        assert!(matches!(result, Err(AgentModelCatalogError::Unavailable)));
        // Headers can still be in flight when virtual time advances. Either
        // timeout phase must produce the same typed, bounded failure.
        let expected = Failure::new(Dependency::ModelGateway, Reason::Timeout);
        let body_timeout = Failure::new(Dependency::ModelGateway, Reason::BodyTimeout);
        assert!(failure == Some(expected) || (body && failure == Some(body_timeout)));
        assert_deadline(deadline);
        tokio::time::advance(Duration::from_secs(60)).await;
        tokio::task::yield_now().await;
        assert_eq!(peer.count(), 1);
        drop(held);
        tokio::time::resume();
    }
}
