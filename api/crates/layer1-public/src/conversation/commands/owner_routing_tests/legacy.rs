use super::*;
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
    Router as HttpRouter,
};
use serde_json::{json, Value};
use tower::ServiceExt;

fn request(path: &str) -> Request<Body> {
    let body: Value = if path == "/api/v1/agui/run" {
        json!({"threadId":ID,"runId":"owner-route","messages":[{"role":"user","content":"Next turn"}]})
    } else {
        json!({"conversation":ID,"input":"Next turn","stream":true})
    };
    let mut request =
        fixtures::request(Method::POST, path, "owner-a", Some("wire-key"), Some(body));
    request.extensions_mut().insert(context("owner-a"));
    request
        .extensions_mut()
        .insert(crate::DelegatedCredential::new("fixture-credential".into()));
    request
}
fn routes(f: &Fixture) -> HttpRouter {
    crate::agui::router(f.state.clone())
        .merge(crate::responses::router(f.state.clone()))
        .merge(crate::responses::v2::router(
            crate::responses::v2::ResponsesV2State::new(
                crate::agent_page::AgentPageOrigin::test(),
                f.service.clone(),
                f.state.reads.clone(),
            ),
        ))
}

#[tokio::test]
async fn legacy_spawn_and_v2_request_preserve_original_evidence_without_local_fallback() {
    for path in ["/api/v1/agui/run", "/api/v1/responses", "/api/v2/responses"] {
        let f = fixture(Some("unavailable")).await;
        let evidence = OwnerCommandRoutingEvidence::new(Arc::from(&b"request-origin-fixture"[..]));
        let response = tokio::time::timeout(
            Duration::from_secs(2),
            evidence.scope(routes(&f).oneshot(request(path))),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE, "{path}");
        assert_eq!(
            *f.router.evidence.lock().unwrap(),
            vec![Some(b"request-origin-fixture".to_vec())],
            "{path}"
        );
        assert_eq!(f.router.mutations.lock().unwrap().len(), 1);
        assert_eq!(f.ingress.0.load(Ordering::SeqCst), 0);
        assert!(records(&f).await.is_empty());
        assert!(OwnerCommandRoutingEvidence::current().is_none());
    }
}

#[tokio::test]
async fn owner_admission_leaves_legacy_stream_observation_at_ingress() {
    for path in ["/api/v1/agui/run", "/api/v1/responses"] {
        let f = fixture(None).await;
        // The existing protocol fixture drives shared ledger work so the
        // unchanged ingress observer can bind its stream to the admitted turn.
        let router = command_test_support::router(f.state.clone(), f.store.clone());
        let evidence = OwnerCommandRoutingEvidence::new(Arc::from(&b"stream-origin-fixture"[..]));
        let response = tokio::time::timeout(
            Duration::from_secs(3),
            evidence.scope(router.oneshot(request(path))),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert!(response
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("text/event-stream"));
        assert_eq!(f.router.mutations.lock().unwrap().len(), 1);
        assert_eq!(
            *f.router.evidence.lock().unwrap(),
            vec![Some(b"stream-origin-fixture".to_vec())]
        );
        assert_eq!(f.ingress.0.load(Ordering::SeqCst), 0);
        assert_eq!(records(&f).await.len(), 1);
    }
}
