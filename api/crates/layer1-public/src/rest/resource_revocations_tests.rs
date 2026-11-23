use super::agents_tests::json_response;
use super::resource_access_tests::{app, request};
use axum::{http::StatusCode, Extension};
use layer2_core::resource_access::{execution::*, revocations::*, *};
use serde_json::json;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

pub(super) struct Service {
    pub value: RevocationStatus,
    pub error: Option<ExecutionError>,
    pub commands: Mutex<Vec<(String, Option<u64>)>>,
}
pub(super) fn pending() -> RevocationStatus {
    RevocationStatus {
        operation_id: "a".repeat(64),
        selector: ResourceSelector::Agent {
            agent_id: layer2_core::AgentId("agt_share".into()),
        },
        share_id: "share".into(),
        expected_revision: 1,
        stage: RevocationStage::Pending,
        participants: 2,
        stopped: 1,
        unresolved_effects: 1,
        deadline_unix_ms: 11_000,
        deadline_exceeded: true,
        receipt: None,
    }
}
impl Service {
    pub fn new() -> Self {
        Self {
            value: pending(),
            error: None,
            commands: Mutex::new(vec![]),
        }
    }
    pub fn completed() -> Self {
        let mut service = Self::new();
        let actor = ResourceActor::Human {
            subject: layer2_core::SubjectId::new("owner-a"),
        };
        service.value.stage = RevocationStage::Effective;
        service.value.stopped = service.value.participants;
        service.value.deadline_exceeded = false;
        service.value.receipt = Some(ShareMutationReceipt {
            operation_id: service.value.operation_id.clone(),
            command_fingerprint: "b".repeat(64),
            sandbox_id: layer2_core::SandboxId("sandbox".into()),
            sandbox_created_at: "2026-01-01T00:00:00Z".into(),
            share: ResourceShare {
                id: "share".into(),
                owner: ResourceOwner::Person {
                    subject: layer2_core::SubjectId::new("owner-a"),
                },
                selector: service.value.selector.clone(),
                target_org_id: layer2_core::SubjectId::new("org"),
                link: None,
                mode: ShareMode::View,
                issuer_actor: actor.clone(),
                created_at: "2026-01-01T00:00:00Z".into(),
                revoked_at: Some("2026-01-01T00:00:01Z".into()),
                revision: 2,
            },
            actor,
            at: "2026-01-01T00:00:01Z".into(),
            policy_revision: 2,
        });
        service
    }
    fn observe(
        &self,
        context: &CallerContext,
        selector: &ResourceSelector,
        id: &str,
    ) -> ExecutionResult<RevocationStatus> {
        if context.caller.subject.as_str() != "owner-a"
            || selector != &self.value.selector
            || id != self.value.operation_id
        {
            return Err(ExecutionError::AuthorityChanged);
        }
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        Ok(self.value.clone())
    }
}
#[async_trait::async_trait]
impl ResourceRevocations for Service {
    async fn request(
        &self,
        context: &CallerContext,
        _: Option<&str>,
        command: ShareCommand,
    ) -> ExecutionResult<RevocationStatus> {
        self.commands
            .lock()
            .unwrap()
            .push((command.idempotency_key, command.expected_share_revision));
        self.observe(context, &command.selector, &self.value.operation_id)
    }
    async fn status(
        &self,
        context: &CallerContext,
        _: Option<&str>,
        selector: &ResourceSelector,
        id: &str,
    ) -> ExecutionResult<RevocationStatus> {
        self.observe(context, selector, id)
    }
}
fn body() -> serde_json::Value {
    json!({"kind":"agent","id":"agt_share","share_id":"share","expected_revision":1,"change":{"kind":"revoke"}})
}
fn installed(router: axum::Router, service: Arc<Service>) -> axum::Router {
    router.layer(Extension(service as Arc<dyn ResourceRevocations>))
}
#[tokio::test]
async fn creates_and_reads_pending_with_no_success_receipt_and_no_effect_payload() {
    let (router, _, _) = app().await;
    let service = Arc::new(Service::new());
    let router = installed(router, service.clone());
    let (status, value) = json_response(
        &router,
        request(
            "POST",
            "/api/v1/resource-revocations",
            "owner-a",
            Some("original"),
            Some(body()),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(value["state"], "pending");
    assert!(value["receipt"].is_null());
    assert_eq!(value["unresolved_effects"], 1);
    let path = format!(
        "/api/v1/resource-revocations/{}?kind=agent&id=agt_share",
        "a".repeat(64)
    );
    let (status, read) = json_response(&router, request("GET", &path, "owner-a", None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value, read);
    assert_eq!(
        *service.commands.lock().unwrap(),
        [("original".into(), Some(1))]
    );
    assert_eq!(read.as_object().unwrap().len(), 11);
    assert_eq!(
        json_response(&router, request("GET", &path, "outsider", None, None))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}
#[tokio::test]
async fn predecessor_pending_is_problem_with_owner_status_link_and_reuses_key() {
    let (router, _, _) = app().await;
    let service = Arc::new(Service::new());
    let router = installed(router, service.clone());
    for _ in 0..2 {
        let response = router
            .clone()
            .oneshot(request(
                "DELETE",
                "/api/v1/resource-shares/agent/agt_share/share?expected_revision=1",
                "owner-a",
                Some("original"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            response.headers()["content-type"],
            "application/problem+json"
        );
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(
            response.headers()["link"],
            format!(
                "</api/v1/resource-revocations/{}?kind=agent&id=agt_share>; rel=\"status\"",
                "a".repeat(64)
            )
        );
    }
    assert_eq!(
        *service.commands.lock().unwrap(),
        [("original".into(), Some(1)), ("original".into(), Some(1))]
    );
    let response = router
        .oneshot(request(
            "DELETE",
            "/api/v1/resource-shares/agent/agt_share/share?expected_revision=1",
            "outsider",
            Some("foreign"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(!response.headers().contains_key("link"));
}
#[tokio::test]
async fn absent_policy_refuses_new_api_and_preserves_legacy_mutation() {
    let (router, _, _) = app().await;
    assert_eq!(
        json_response(
            &router,
            request(
                "POST",
                "/api/v1/resource-revocations",
                "owner-a",
                Some("new"),
                Some(body())
            )
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let (_, created) = json_response(
        &router,
        request(
            "POST",
            "/api/v1/resource-shares/agent/agt_share",
            "owner-a",
            Some("create"),
            Some(json!({"target_org_id":"org","mode":"view"})),
        ),
    )
    .await;
    let path = format!(
        "/api/v1/resource-shares/agent/agt_share/{}?expected_revision=1",
        created["share"]["id"].as_str().unwrap()
    );
    let (status, value) = json_response(
        &router,
        request("DELETE", &path, "owner-a", Some("revoke"), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(value["share"]["revoked_at"].is_string());
}
#[tokio::test]
async fn authority_and_body_failures_remain_typed_and_private() {
    let (router, _, _) = app().await;
    let router = installed(router, Arc::new(Service::new()));
    let mut forged = request(
        "POST",
        "/api/v1/resource-revocations",
        "outsider",
        Some("forged"),
        Some(body()),
    );
    forged
        .extensions_mut()
        .insert(super::resource_access_tests::context("owner-a"));
    assert_eq!(
        json_response(&router, forged).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        json_response(
            &router,
            request(
                "POST",
                "/api/v1/resource-revocations",
                "owner-a",
                None,
                Some(body())
            )
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let mut invalid = body();
    invalid["actor"] = json!("owner-a");
    assert_eq!(
        json_response(
            &router,
            request(
                "POST",
                "/api/v1/resource-revocations",
                "owner-a",
                Some("new"),
                Some(invalid)
            )
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn only_effective_receipt_uses_the_predecessor_success_shape() {
    let (router, _, _) = app().await;
    let router = installed(router, Arc::new(Service::completed()));
    let (status, receipt) = json_response(
        &router,
        request(
            "DELETE",
            "/api/v1/resource-shares/agent/agt_share/share?expected_revision=1",
            "owner-a",
            Some("original"),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(receipt["share"]["revoked_at"].is_string());
    assert_eq!(receipt["operation_id"], "a".repeat(64));
    assert_eq!(
        receipt["revocation"],
        json!({
            "operation_id": "a".repeat(64), "kind": "agent", "id": "agt_share",
            "share_id": "share", "expected_revision": 1,
        })
    );
    assert!(receipt.get("state").is_none());
    assert!(receipt.get("sandbox_id").is_none());
}

#[tokio::test]
async fn sandbox_success_and_replay_keep_original_status_identity_with_unresolved_effects() {
    for method in ["DELETE", "PATCH"] {
        let (router, _, _) = app().await;
        let mut service = Service::completed();
        service.value.selector = ResourceSelector::Sandbox {
            sandbox_id: layer2_core::SandboxId("sandbox".into()),
        };
        let receipt = service.value.receipt.as_mut().unwrap();
        receipt.share.selector = service.value.selector.clone();
        if method == "PATCH" {
            receipt.share.revoked_at = None;
        }
        let service = Arc::new(service);
        let router = installed(router, service.clone());
        for _ in 0..2 {
            let body = (method == "PATCH").then(|| json!({"mode":"view", "expected_revision":1}));
            let response = router
                .clone()
                .oneshot(request(
                    method,
                    "/api/v1/resource-shares/sandbox/sandbox/share?expected_revision=1",
                    "owner-a",
                    Some("original"),
                    body,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()["cache-control"], "no-store");
            let body: serde_json::Value = serde_json::from_slice(
                &axum::body::to_bytes(response.into_body(), 16 * 1024)
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(
                body["revocation"],
                json!({
                    "operation_id":"a".repeat(64), "kind":"sandbox", "id":"sandbox",
                    "share_id":"share", "expected_revision":1,
                })
            );
            let (_, status) = json_response(
                &router,
                request(
                    "GET",
                    &format!(
                        "/api/v1/resource-revocations/{}?kind=sandbox&id=sandbox",
                        "a".repeat(64)
                    ),
                    "owner-a",
                    None,
                    None,
                ),
            )
            .await;
            assert_eq!(status["unresolved_effects"], 1);
            let mut legacy = body;
            legacy.as_object_mut().unwrap().remove("revocation");
            assert_eq!(legacy, status["receipt"]);
        }
        assert_eq!(
            *service.commands.lock().unwrap(),
            [("original".into(), Some(1)), ("original".into(), Some(1))]
        );
    }
}

#[tokio::test]
async fn agent_link_success_and_replay_preserve_original_operation_without_changing_legacy_fields()
{
    let (router, store, _) = app().await;
    let (code, created) = json_response(
        &router,
        request(
            "POST",
            "/api/v1/agents/agt_share/share-links",
            "owner-a",
            Some("create-link"),
            Some(json!({"audience":{"kind":"public"}, "mode":"view", "token":"c".repeat(64)})),
        ),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert!(created.get("revocation").is_none());
    let id = created["id"].as_str().unwrap();
    let mut service = Service::completed();
    let mut share = store.resource_share(id).await.unwrap().unwrap();
    share.revoked_at = Some("2026-01-01T00:00:01Z".into());
    share.revision += 1;
    service.value.share_id = id.into();
    service.value.receipt.as_mut().unwrap().share = share;
    let service = Arc::new(service);
    let router = installed(router, service.clone());
    for _ in 0..2 {
        let response = router
            .clone()
            .oneshot(request(
                "DELETE",
                &format!("/api/v1/agents/agt_share/share-links/{id}?expected_revision=1"),
                "owner-a",
                Some("original"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 16 * 1024)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            body["revocation"],
            json!({
                "operation_id":"a".repeat(64), "kind":"agent", "id":"agt_share",
                "share_id":id, "expected_revision":1,
            })
        );
        let mut legacy = body;
        legacy.as_object_mut().unwrap().remove("revocation");
        let mut expected = created.clone();
        expected["revoked_at"] = json!("2026-01-01T00:00:01Z");
        expected["revision"] = json!(2);
        assert_eq!(legacy, expected);
    }
    assert_eq!(service.value.unresolved_effects, 1);
    assert_eq!(
        *service.commands.lock().unwrap(),
        [("original".into(), Some(1)), ("original".into(), Some(1))]
    );
}
