use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
};
use layer2_core::sandbox::store::{AgentLogStore, AgentStore};
use serde_json::Value;
use tower::ServiceExt;

use crate::rest::{agents_tests, resource_access_tests::context, shared_agent_tests};

const PATH: &str = "/api/v1/agents/agt_shared_read/capacity-recovery/dry-run";

fn request(subject: &str) -> Request<Body> {
    let mut request = agents_tests::request(Method::GET, PATH, subject, None, None);
    request.extensions_mut().insert(context(subject));
    request
        .extensions_mut()
        .insert(DelegatedCredential::new("request-credential".into()));
    request
}

async fn response(router: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = router.clone().oneshot(request).await.unwrap();
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 32 * 1024).await.unwrap();
    let value = serde_json::from_slice(&bytes).unwrap();
    (status, value)
}

#[tokio::test]
async fn dry_run_route_returns_only_current_metadata_and_preserves_agent_and_log() {
    let (fixture, access, _, _) =
        shared_agent_tests::fixture(shared_agent_tests::Reply::Member).await;
    // Admission has no feature/configuration path into this metadata service.
    let router =
        agent_capacity_recovery_router(Arc::new(CapacityRecoveryPreflightService::new(access)));
    let id = AgentId("agt_shared_read".into());
    let before = fixture.store.get_agent(&id).await.unwrap().unwrap();
    let logs = fixture.store.agent_log(&id, 0, 200).await.unwrap();
    for _ in 0..2 {
        let (status, value) = response(&router, request("owner-a")).await;
        assert_eq!(status, StatusCode::OK, "{value}");
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
                "agent_id",
                "disposition",
                "evaluated_at",
                "observed_revision",
                "through_seq"
            ]
        );
        assert_eq!(value["agent_id"], id.as_str());
        assert_eq!(value["disposition"], "not_capacity_failure");
        assert_eq!(value["observed_revision"], before.revision);
        assert_eq!(value["through_seq"], before.conversation.last_seq);
        assert!(
            layer2_core::seconds_from_rfc3339(value["evaluated_at"].as_str().unwrap()).is_some()
        );
    }
    assert_eq!(fixture.store.get_agent(&id).await.unwrap(), Some(before));
    assert_eq!(fixture.store.agent_log(&id, 0, 200).await.unwrap(), logs);
}

#[tokio::test]
async fn dry_run_route_authentication_refusal_is_private_and_uncached() {
    let (_, access, _, _) = shared_agent_tests::fixture(shared_agent_tests::Reply::Member).await;
    let router =
        agent_capacity_recovery_router(Arc::new(CapacityRecoveryPreflightService::new(access)));
    let anonymous = Request::builder().uri(PATH).body(Body::empty()).unwrap();
    let (status, _) = response(&router, anonymous).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let outsider = agents_tests::request(Method::GET, PATH, "outsider", None, None);
    let (status, _) = response(&router, outsider).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn dry_run_route_requires_operate_permission_and_current_membership() {
    let (_, access, authority, _) =
        shared_agent_tests::fixture(shared_agent_tests::Reply::Member).await;
    let router =
        agent_capacity_recovery_router(Arc::new(CapacityRecoveryPreflightService::new(access)));
    let (status, _) = response(&router, request("recipient")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    *authority.0.lock().unwrap() = shared_agent_tests::Reply::Outage;
    let (status, _) = response(&router, request("recipient")).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}
