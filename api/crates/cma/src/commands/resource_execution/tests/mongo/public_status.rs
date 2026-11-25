use super::super::super::reconciliation::EffectReconciler;
use super::super::reconciliation::fixture::Provider;
use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use layer1_public::{AuthenticatedSubject, DelegatedCredential};
use layer2_core::resource_access::{
    revocations::{ResourceRevocationService, ResourceRevocations, RevocationStatus},
    CallerContext, ResourceSelector, ShareCommand,
};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use tower::ServiceExt;

async fn app(fixture: &Fixture) -> Router {
    app_with_service(fixture, Arc::new(service(fixture).await))
}

async fn service(fixture: &Fixture) -> ResourceRevocationService {
    ResourceRevocationService::new(fixture.access.clone(), fixture.reopened().await)
        .await
        .unwrap()
}

fn app_with_service(fixture: &Fixture, service: Arc<dyn ResourceRevocations>) -> Router {
    layer1_public::rest::resource_access_router_with_revocations(
        Arc::new(fixture.access.clone()),
        service,
    )
}

struct LoseFirstReply {
    service: ResourceRevocationService,
    lose_reply: AtomicBool,
}

#[async_trait::async_trait]
impl ResourceRevocations for LoseFirstReply {
    async fn request(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        command: ShareCommand,
    ) -> ExecutionResult<RevocationStatus> {
        let admitted = self.service.request(context, credential, command).await?;
        // The real service has admitted the operation. Only its reply is lost.
        if self.lose_reply.swap(false, Ordering::SeqCst) {
            return Err(ExecutionError::Unavailable);
        }
        Ok(admitted)
    }

    async fn status(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        selector: &ResourceSelector,
        operation: &str,
    ) -> ExecutionResult<RevocationStatus> {
        self.service
            .status(context, credential, selector, operation)
            .await
    }
}

fn body(fixture: &Fixture) -> Value {
    let layer2_core::resource_access::ResourceSelector::Sandbox { sandbox_id } = &fixture.selector
    else {
        panic!()
    };
    json!({
        "kind": "sandbox", "id": sandbox_id.as_str(), "share_id": fixture.share.id,
        "expected_revision": fixture.share.revision, "change": {"kind":"revoke"}
    })
}

fn path(fixture: &Fixture, operation: &str) -> String {
    let layer2_core::resource_access::ResourceSelector::Sandbox { sandbox_id } = &fixture.selector
    else {
        panic!()
    };
    format!("/api/v1/resource-revocations/{operation}?kind=sandbox&id={sandbox_id}")
}

async fn call(
    router: &Router,
    method: &str,
    path: &str,
    subject: Option<&str>,
    body: Option<Value>,
    key: &str,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header("Idempotency-Key", key)
        .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))
        .unwrap();
    if let Some(subject) = subject {
        let context = fixture::context(subject);
        request
            .extensions_mut()
            .insert(AuthenticatedSubject(context.caller.subject.clone()));
        request
            .extensions_mut()
            .insert(context.caller.authorization.clone());
        request.extensions_mut().insert(context);
        request
            .extensions_mut()
            .insert(DelegatedCredential::new("synthetic".into()));
    }
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    if status.is_success() {
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
    let bytes = to_bytes(response.into_body(), 16_384).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn create(router: &Router, fixture: &Fixture, key: &str) -> Value {
    let (status, value) = call(
        router,
        "POST",
        "/api/v1/resource-revocations",
        Some("owner"),
        Some(body(fixture)),
        key,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{value}");
    value
}

async fn read(router: &Router, fixture: &Fixture, operation: &str) -> Value {
    let (status, value) = call(
        router,
        "GET",
        &path(fixture, operation),
        Some("owner"),
        None,
        "read",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    // Public status contains aggregate progress and the owner receipt only.
    let mut keys: Vec<_> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "deadline_exceeded",
            "deadline_unix_ms",
            "expected_revision",
            "operation_id",
            "participants",
            "receipt",
            "selector",
            "share_id",
            "state",
            "stopped",
            "unresolved_effects"
        ]
    );
    value
}

async fn operation(fixture: &Fixture, value: &Value) -> RevocationOperationV1 {
    fixture
        .store
        .revocation(&wire::Digest::new(value["operation_id"].as_str().unwrap()).unwrap())
        .await
        .unwrap()
        .unwrap()
}

// Authentication context, provider replies and stop receipts are synthetic.
// The public router, owner service, worker and Mongo persistence are real.
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn public_effective_receipt_survives_late_effect_resolution_and_service_reopen() {
    for outcome in [
        wire::EffectOutcome::CommitConfirmed,
        wire::EffectOutcome::Rejected,
    ] {
        let fixture = Fixture::new().await;
        let effect = effects::prepare(&fixture).await;
        let router = app(&fixture).await;
        let pending = create(&router, &fixture, "original-public-command").await;
        assert_eq!(pending["state"], "pending");
        assert_eq!(pending["participants"], 1);
        assert_eq!(pending["stopped"], 0);
        assert_eq!(pending["unresolved_effects"], 1);
        assert!(pending["receipt"].is_null());
        let original = operation(&fixture, &pending).await;
        let id = original.key.operation_id.as_str();
        effects::complete(&fixture, &effect, &original).await;
        let reopened = app(&fixture).await;
        let effective = read(&reopened, &fixture, id).await;
        assert_eq!(effective["state"], "effective");
        assert_eq!(effective["stopped"], 1);
        assert_eq!(effective["unresolved_effects"], 1);
        assert_eq!(effective["operation_id"], pending["operation_id"]);
        assert_eq!(effective["deadline_unix_ms"], pending["deadline_unix_ms"]);
        let provider = Provider::new(&effect, outcome);
        let reconciler = EffectReconciler::new(fixture.reopened().await, provider.clone());
        let after_reads = fixture
            .store
            .effects_for(&effect.permission.intent.registration, 0, 32)
            .await
            .unwrap();
        assert_eq!(after_reads.as_slice(), std::slice::from_ref(&effect));
        assert!(provider.calls.lock().unwrap().is_empty());
        assert_eq!(
            reconciler
                .reconcile(&effect.permission.intent.registration)
                .await
                .unwrap()
                .terminal,
            1
        );
        let resolved = read(&app(&fixture).await, &fixture, id).await;
        assert_eq!(resolved["state"], "effective");
        assert_eq!(resolved["unresolved_effects"], 0);
        assert_eq!(resolved["receipt"], effective["receipt"]);
        let replay = create(&app(&fixture).await, &fixture, "original-public-command").await;
        assert_eq!(replay["operation_id"], pending["operation_id"]);
        assert_eq!(replay["receipt"], effective["receipt"]);
        assert_eq!(replay["unresolved_effects"], 0);
        assert_eq!(provider.calls.lock().unwrap().len(), 1);
        fixture.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn public_uncertain_create_retries_original_identity_across_service_replacement() {
    let fixture = Fixture::new().await;
    let effect = effects::prepare(&fixture).await;
    // Inject service reply loss after real admission, not Mongo commit uncertainty.
    let router = app_with_service(
        &fixture,
        Arc::new(LoseFirstReply {
            service: service(&fixture).await,
            lose_reply: AtomicBool::new(true),
        }),
    );
    let (status, error) = call(
        &router,
        "POST",
        "/api/v1/resource-revocations",
        Some("owner"),
        Some(body(&fixture)),
        "uncertain-public-command",
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{error}");
    assert_eq!(error["code"], "resource_revocation_unavailable");
    assert!(error.get("operation_id").is_none());
    let retained = fixture.store.pending_revocations(None, 32).await.unwrap();
    assert_eq!(retained.len(), 1);
    let replacement = app(&fixture).await;
    let (a, b) = tokio::join!(
        create(&replacement, &fixture, "uncertain-public-command"),
        create(&router, &fixture, "uncertain-public-command")
    );
    assert_eq!(a["operation_id"], retained[0].key.operation_id.as_str());
    assert_eq!(b["operation_id"], a["operation_id"]);
    assert_eq!(a["deadline_unix_ms"], retained[0].deadline_unix_ms);
    assert_eq!(b["deadline_unix_ms"], a["deadline_unix_ms"]);
    let (conflict, _) = call(
        &replacement,
        "POST",
        "/api/v1/resource-revocations",
        Some("owner"),
        Some(body(&fixture)),
        "replacement-key-is-not-recovery",
    )
    .await;
    assert_eq!(conflict, StatusCode::CONFLICT);
    assert_eq!(
        fixture
            .store
            .pending_revocations(None, 32)
            .await
            .unwrap()
            .len(),
        1
    );
    effects::complete(&fixture, &effect, &retained[0]).await;
    let effective = read(
        &replacement,
        &fixture,
        retained[0].key.operation_id.as_str(),
    )
    .await;
    let replay = create(&replacement, &fixture, "uncertain-public-command").await;
    assert_eq!(replay["state"], "effective");
    assert_eq!(replay["operation_id"], a["operation_id"]);
    assert_eq!(replay["deadline_unix_ms"], a["deadline_unix_ms"]);
    assert_eq!(replay["receipt"], effective["receipt"]);
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn public_pending_and_final_status_refuse_other_actors_and_resource_selectors() {
    let fixture = Fixture::new().await;
    let effect = effects::prepare(&fixture).await;
    let router = app(&fixture).await;
    let pending = create(&router, &fixture, "private-public-command").await;
    let retained = operation(&fixture, &pending).await;
    let id = retained.key.operation_id.as_str();
    for final_stage in [false, true] {
        if final_stage {
            effects::complete(&fixture, &effect, &retained).await;
        }
        let router = app(&fixture).await;
        let owner = read(&router, &fixture, id).await;
        assert_eq!(owner["unresolved_effects"], 1);
        for subject in ["recipient", "another-owner"] {
            let found = call(
                &router,
                "GET",
                &path(&fixture, id),
                Some(subject),
                None,
                "read",
            )
            .await;
            let absent = call(
                &router,
                "GET",
                &path(&fixture, digest('e').as_str()),
                Some(subject),
                None,
                "read",
            )
            .await;
            assert_eq!(found.0, StatusCode::NOT_FOUND);
            assert_eq!(found, absent);
            assert_eq!(found.1["code"], "resource_not_found");
        }
        let no_auth = call(&router, "GET", &path(&fixture, id), None, None, "read").await;
        assert_eq!(no_auth.0, StatusCode::UNAUTHORIZED);
        let wrong_target = format!(
            "/api/v1/resource-revocations/{id}?kind=agent&id={}",
            effect.permission.intent.execution.agent_id.as_str()
        );
        let wrong = call(&router, "GET", &wrong_target, Some("owner"), None, "read").await;
        assert_eq!(wrong.0, StatusCode::NOT_FOUND);
        assert_eq!(wrong.1["code"], "resource_not_found");
        assert!(wrong.1.get("receipt").is_none());
        assert!(wrong.1.get("unresolved_effects").is_none());
        assert_eq!(
            fixture
                .store
                .effects_for(&effect.permission.intent.registration, 0, 32)
                .await
                .unwrap()
                .as_slice(),
            std::slice::from_ref(&effect)
        );
    }
    fixture.cleanup().await;
}
