use super::{agents_tests, resource_access_tests::context};
use axum::{
    body::Body,
    http::{header, HeaderValue, Method, Request, StatusCode},
    Router,
};
use layer2_core::{resource_access::CallerContext, sandbox::agent_access::*, AgentId, Caller};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

#[derive(Default)]
pub(super) struct Service {
    pub(super) calls: Mutex<Vec<Value>>,
    pub(super) error: Mutex<Option<AgentAccessError>>,
}
fn receipt(status: AgentAccessStatus) -> AgentAccessRequest {
    AgentAccessRequest {
        retry_of: None,
        id: "access-one".into(),
        agent_id: AgentId("agent-one".into()),
        status,
        services: vec!["api-github".into()],
        reason: "Read the selected repository.".into(),
        error: None,
        github_approval: None,
    }
}
impl Service {
    fn result(&self, status: AgentAccessStatus) -> Result<AgentAccessRequest, AgentAccessError> {
        self.error
            .lock()
            .unwrap()
            .clone()
            .map_or_else(|| Ok(receipt(status)), Err)
    }
}
#[async_trait::async_trait]
impl AgentAccessRequests for Service {
    async fn replace_services(
        &self,
        context: &CallerContext,
        credential: &str,
        agent: &AgentId,
        services: Vec<String>,
        expected_services: Vec<String>,
        key: &str,
    ) -> Result<AgentAccessRequest, AgentAccessError> {
        assert_eq!(credential, "request-credential");
        self.calls.lock().unwrap().push(json!({"action":"replace","caller":context.caller.subject.as_str(),"agent":agent.0,"services":services,"expected_services":expected_services,"key":key}));
        self.result(AgentAccessStatus::Connecting)
    }
    async fn list(
        &self,
        caller: &Caller,
        agent: &AgentId,
    ) -> Result<Vec<AgentAccessRequest>, AgentAccessError> {
        self.calls
            .lock()
            .unwrap()
            .push(json!({"action":"list","caller":caller.subject.as_str(),"agent":agent.0}));
        self.result(AgentAccessStatus::Pending)
            .map(|value| vec![value])
    }
    async fn approve(
        &self,
        context: &CallerContext,
        credential: &str,
        agent: &AgentId,
        request: &str,
        services: Vec<String>,
        key: &str,
    ) -> Result<AgentAccessRequest, AgentAccessError> {
        assert_eq!(credential, "request-credential");
        self.calls.lock().unwrap().push(json!({"action":"approve","caller":context.caller.subject.as_str(),"agent":agent.0,"request":request,"services":services,"key":key}));
        self.result(AgentAccessStatus::Connecting)
    }
    async fn approve_github_connection(
        &self,
        context: &CallerContext,
        credential: &str,
        agent: &AgentId,
        request: &str,
        services: Vec<String>,
        choice: GithubConnectionApproval,
        key: &str,
    ) -> Result<AgentAccessRequest, AgentAccessError> {
        assert_eq!(credential, "request-credential");
        self.calls.lock().unwrap().push(json!({
            "action": "approve_github_connection",
            "caller": context.caller.subject.as_str(),
            "agent": agent.0,
            "request": request,
            "services": services,
            "key": key,
            "choice": {
                "sandbox_id": choice.sandbox_id.0,
                "user_service_id": choice.user_service_id,
                "scope_revision": choice.scope_revision,
                "credential_generation": choice.credential_generation,
            },
        }));
        self.result(AgentAccessStatus::Connecting)
    }
    async fn deny(
        &self,
        caller: &Caller,
        agent: &AgentId,
        request: &str,
        key: &str,
    ) -> Result<AgentAccessRequest, AgentAccessError> {
        self.calls.lock().unwrap().push(json!({"action":"deny","caller":caller.subject.as_str(),"agent":agent.0,"request":request,"key":key}));
        self.result(AgentAccessStatus::Denied)
    }
}
pub(super) fn app() -> (Router, Arc<Service>) {
    let service = Arc::new(Service::default());
    (
        super::agent_access_requests_router(service.clone()),
        service,
    )
}
fn request(
    method: Method,
    action: Option<&str>,
    key: Option<&str>,
    body: Option<Value>,
) -> Request<Body> {
    let path = action.map_or(
        "/api/v1/agents/agent-one/access-requests".into(),
        |action| format!("/api/v1/agents/agent-one/access-requests/access-one/{action}"),
    );
    let mut request = agents_tests::request(method, &path, "owner", key, body);
    request.extensions_mut().insert(context("owner"));
    request
        .extensions_mut()
        .insert(crate::DelegatedCredential::new("request-credential".into()));
    request
}
#[tokio::test]
async fn routes_project_durable_requests_and_forward_exact_authority_and_decisions() {
    let (router, service) = app();
    let (status, listed) =
        agents_tests::json_response(&router, request(Method::GET, None, None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["items"][0]["status"], "pending");
    assert!(listed["items"][0].get("connection").is_none());
    let body = json!({"services":["chrono-llm-public","ornn-api","api-github"]});
    for _ in 0..2 {
        let (status, value) = agents_tests::json_response(
            &router,
            request(
                Method::POST,
                Some("approve"),
                Some("exact-key"),
                Some(body.clone()),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(value["status"], "connecting");
        assert_eq!(
            value["connection"],
            json!({"target":{"type":"agent_access_request","id":"access-one"}})
        );
        assert!(!value.to_string().contains("request-credential"));
    }
    let (status, denied) = agents_tests::json_response(
        &router,
        request(
            Method::POST,
            Some("deny"),
            Some("deny-key"),
            Some(json!({})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(denied["status"], "denied");
    let calls = service.calls.lock().unwrap();
    assert_eq!(calls[1], calls[2]);
    assert_eq!(
        calls[1],
        json!({"action":"approve","caller":"owner","agent":"agent-one","request":"access-one","services":body["services"],"key":"exact-key"})
    );
    assert_eq!(calls[3]["key"], "deny-key");
}
#[tokio::test]
async fn github_approval_forwards_one_exact_owner_decision() {
    let (router, service) = app();
    let body = json!({
        "services": ["chrono-llm-public", "api-github"],
        "github_connection": {
            "sandbox_id": "sandbox-one",
            "user_service_id": "repository-connection",
            "scope_revision": 3,
            "credential_generation": 7,
        },
    });
    let submit = || {
        request(
            Method::POST,
            Some("approve"),
            Some("owner-choice"),
            Some(body.clone()),
        )
    };
    let mut without_owner_credential = submit();
    without_owner_credential
        .extensions_mut()
        .remove::<crate::DelegatedCredential>();
    assert_eq!(
        router
            .clone()
            .oneshot(without_owner_credential)
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert!(service.calls.lock().unwrap().is_empty());

    let (status, receipt) = agents_tests::json_response(&router, submit()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(receipt["status"], "connecting");
    assert_eq!(
        *service.calls.lock().unwrap(),
        vec![json!({
            "action": "approve_github_connection",
            "caller": "owner",
            "agent": "agent-one",
            "request": "access-one",
            "services": body["services"],
            "key": "owner-choice",
            "choice": body["github_connection"],
        })]
    );
}

#[tokio::test]
async fn authentication_context_and_delegation_are_verified_before_effects() {
    let (router, service) = app();
    let approve = || {
        request(
            Method::POST,
            Some("approve"),
            Some("key"),
            Some(json!({"services":["api-github"]})),
        )
    };
    let mut missing = approve();
    missing
        .extensions_mut()
        .remove::<crate::AuthenticatedSubject>();
    assert_eq!(
        router.clone().oneshot(missing).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let mut wrong = approve();
    wrong.extensions_mut().insert(context("another-owner"));
    assert_eq!(
        router.clone().oneshot(wrong).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let mut missing_context = approve();
    missing_context.extensions_mut().remove::<CallerContext>();
    assert_eq!(
        router
            .clone()
            .oneshot(missing_context)
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let mut missing_credential = approve();
    missing_credential
        .extensions_mut()
        .remove::<crate::DelegatedCredential>();
    assert_eq!(
        router.oneshot(missing_credential).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    assert!(service.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn decisions_require_one_key_and_bounded_typed_bodies() {
    let (router, service) = app();
    for action in ["approve", "deny"] {
        let body = if action == "approve" {
            json!({"services":["api-github"]})
        } else {
            json!({})
        };
        for key in [None, Some(" ")] {
            assert_eq!(
                router
                    .clone()
                    .oneshot(request(Method::POST, Some(action), key, Some(body.clone())))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::BAD_REQUEST
            );
        }
        let mut duplicate = request(Method::POST, Some(action), Some("first"), Some(body));
        duplicate
            .headers_mut()
            .append("idempotency-key", HeaderValue::from_static("second"));
        assert_eq!(
            router.clone().oneshot(duplicate).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
    }
    for services in [
        json!([]),
        json!([" "]),
        json!(["github/read"]),
        json!(["x".repeat(129)]),
        json!(vec!["service"; 33]),
    ] {
        assert_eq!(
            router
                .clone()
                .oneshot(request(
                    Method::POST,
                    Some("approve"),
                    Some("key"),
                    Some(json!({"services":services}))
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        router
            .clone()
            .oneshot(request(
                Method::POST,
                Some("deny"),
                Some("key"),
                Some(json!({"services":[]}))
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let mut huge = request(Method::POST, Some("approve"), Some("key"), None);
    huge.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    *huge.body_mut() = Body::from(json!({"services":["x".repeat(9000)]}).to_string());
    assert_eq!(
        router.clone().oneshot(huge).await.unwrap().status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert!(service.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn domain_refusals_keep_safe_http_semantics_and_private_cache_headers() {
    let (router, service) = app();
    for (error, status) in [
        (AgentAccessError::Invalid, 400),
        (AgentAccessError::Forbidden, 403),
        (AgentAccessError::OwnerAuthorityRequired, 403),
        (AgentAccessError::ServiceConnectionRequired, 409),
        (AgentAccessError::NotFound, 404),
        (AgentAccessError::Conflict, 409),
        (AgentAccessError::StaleGithubApproval, 409),
        (AgentAccessError::Unavailable, 503),
    ] {
        let stale = error == AgentAccessError::StaleGithubApproval;
        *service.error.lock().unwrap() = Some(error);
        for (method, action, body) in [
            (Method::GET, None, None),
            (
                Method::POST,
                Some("approve"),
                Some(json!({"services":["api-github"]})),
            ),
            (Method::POST, Some("deny"), Some(json!({}))),
        ] {
            let response = router
                .clone()
                .oneshot(request(method, action, Some("key"), body))
                .await
                .unwrap();
            assert_eq!(response.status().as_u16(), status);
            assert_eq!(
                response.headers()[header::CACHE_CONTROL],
                "private, no-store"
            );
            assert_eq!(
                response.headers()[header::CONTENT_TYPE],
                "application/problem+json"
            );
            if stale {
                let bytes = axum::body::to_bytes(response.into_body(), 4096)
                    .await
                    .unwrap();
                let problem: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(problem["code"], "agent_access_stale_github_approval");
            }
        }
    }
}
#[test]
fn access_request_openapi_declares_approval_identity_and_durable_progress() {
    let value = serde_json::to_value(super::openapi::document()).unwrap();
    for (method, path, operation) in [
        (
            "get",
            "/api/v1/agents/{id}/access-requests",
            "get_api_v1_agents_by_id_access_requests",
        ),
        (
            "post",
            "/api/v1/agents/{id}/access-requests/{request_id}/approve",
            "post_api_v1_agents_by_id_access_requests_by_request_id_approve",
        ),
        (
            "post",
            "/api/v1/agents/{id}/access-requests/{request_id}/deny",
            "post_api_v1_agents_by_id_access_requests_by_request_id_deny",
        ),
    ] {
        assert_eq!(value["paths"][path][method]["operationId"], operation);
        if method == "post" {
            assert!(value["paths"][path][method]["parameters"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["name"] == "Idempotency-Key" && p["required"] == true));
        }
    }
    assert!(
        value["components"]["schemas"]["AgentAccessRequestView"]["properties"]["connection"]
            .is_object()
    );
}

#[test]
fn approved_connection_recovery_remains_available_until_the_request_is_terminal() {
    use super::agent_access_requests::AgentAccessRequestView;
    for status in [AgentAccessStatus::Connecting, AgentAccessStatus::Applying] {
        let value = serde_json::to_value(AgentAccessRequestView::from(receipt(status))).unwrap();
        assert_eq!(
            value["connection"],
            json!({"target":{"type":"agent_access_request","id":"access-one"}})
        );
        assert_eq!(value["services"], json!(["api-github"]));
    }
    for status in [
        AgentAccessStatus::Pending,
        AgentAccessStatus::Succeeded,
        AgentAccessStatus::Denied,
        AgentAccessStatus::Failed,
    ] {
        let value = serde_json::to_value(AgentAccessRequestView::from(receipt(status))).unwrap();
        assert!(value.get("connection").is_none());
    }
}

#[cfg(test)]
#[path = "agent_service_access_tests.rs"]
mod scope_tests;
