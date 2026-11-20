use super::*;
use axum::{body::Body, http::Request, response::Response};
use layer2_core::sandbox::store::SandboxStore;
use serde_json::json;
use std::sync::atomic::Ordering;
use std::time::Duration;

fn request(method: Method, id: &str, incarnation: &str, subject: &str) -> Request<Body> {
    let path = format!("/api/v1/agents/{id}/browser");
    let (path, body) = if method == Method::PUT {
        (
            path,
            Some(json!({"runtime_incarnation":incarnation,"enabled":true})),
        )
    } else {
        (
            format!("{path}?runtime_incarnation={incarnation}&observer=observer"),
            None,
        )
    };
    crate::rest::agents_tests::request(method, &path, subject, None, body)
}

async fn problem(response: Response, status: StatusCode, code: &str) -> Value {
    assert_eq!(response.status(), status);
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    assert_eq!(
        response.headers()["content-type"],
        "application/problem+json"
    );
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["status"], status.as_u16());
    assert_eq!(body["code"], code);
    body
}

async fn conflict(response: Response) {
    let body = problem(response, StatusCode::CONFLICT, "browser_runtime_conflict").await;
    assert_eq!(
        body,
        json!({
            "type":"https://spec.chrono-ai.fun/cma/1.0/problems/browser_runtime_conflict",
            "title":"browser_runtime_conflict", "status":409, "code":"browser_runtime_conflict",
            "detail":"The browser environment changed. Refresh the Agent environment, then retry with its current runtime incarnation."
        })
    );
}

#[tokio::test]
async fn stale_browser_incarnations_refuse_all_operations_before_runtime_io() {
    let runtime = Arc::new(super::super::runtime::Files::new("browser"));
    let owner = super::super::scenario_with_runtime("filesystem_owner", runtime.clone())
        .await
        .unwrap();
    for method in [Method::GET, Method::PUT, Method::DELETE] {
        for incarnation in [
            "invalid",
            "not-a-real-incarnation",
            "e9ed9ab9-02a9-4c1e-91f5-7e13ca191b24",
            &"0".repeat(64),
        ] {
            let response = owner
                .router
                .clone()
                .oneshot(request(
                    method.clone(),
                    "agt_shared_read",
                    incarnation,
                    "owner-a",
                ))
                .await
                .unwrap();
            conflict(response).await;
        }
    }
    assert_eq!(runtime.browser_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn browser_conflict_does_not_disclose_concealed_or_unauthenticated_agents() {
    for (kind, id, status, code) in [
        (
            "foreign",
            "agt_shared_read",
            StatusCode::NOT_FOUND,
            "resource_not_found",
        ),
        (
            "owner",
            "agt_absent",
            StatusCode::NOT_FOUND,
            "resource_not_found",
        ),
        (
            "anonymous",
            "agt_shared_read",
            StatusCode::UNAUTHORIZED,
            "not_authenticated",
        ),
    ] {
        let runtime = Arc::new(super::super::runtime::Files::new("browser"));
        let fixture =
            super::super::scenario_with_runtime(&format!("filesystem_{kind}"), runtime.clone())
                .await
                .unwrap();
        for method in [Method::GET, Method::PUT, Method::DELETE] {
            let mut request = request(
                method,
                id,
                "invalid",
                fixture.subject.unwrap_or("anonymous"),
            );
            if fixture.subject.is_none() {
                request
                    .extensions_mut()
                    .remove::<crate::AuthenticatedSubject>();
            }
            let response = fixture.router.clone().oneshot(request).await.unwrap();
            let body = problem(response, status, code).await;
            assert!(!body.to_string().contains("incarnation"));
        }
        assert_eq!(runtime.browser_calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn browser_rechecks_environment_after_runtime_reply_without_repeating_mutations() {
    for method in [Method::GET, Method::PUT, Method::DELETE] {
        for change in ["boot", "paused"] {
            let runtime = Arc::new(super::super::runtime::Files::new("browser"));
            let (fixture, store) = super::super::scenario_with_runtime_store(
                "filesystem_owner",
                runtime.clone(),
                &"b".repeat(32),
            )
            .await
            .unwrap();
            let (_, environment) = read(
                &fixture.router,
                "/api/v1/agents/agt_shared_read/environment",
                "owner-a",
            )
            .await;
            let incarnation = environment["runtime_incarnation"].as_str().unwrap();
            runtime.browser_calls.store(0, Ordering::SeqCst);
            *runtime.browser_reply.lock().unwrap() = controlled_frame();
            let barrier = Arc::new(tokio::sync::Barrier::new(2));
            *runtime.browser_barrier.lock().unwrap() = Some(barrier.clone());
            let pending = tokio::spawn(fixture.router.oneshot(request(
                method.clone(),
                "agt_shared_read",
                incarnation,
                "owner-a",
            )));
            tokio::time::timeout(Duration::from_secs(5), barrier.wait())
                .await
                .unwrap();
            let mut sandbox = store
                .get_sandbox(&layer2_core::SandboxId("sbx_route".into()))
                .await
                .unwrap()
                .unwrap();
            let revision = sandbox.revision;
            if change == "boot" {
                let runtime = sandbox.runtime.as_mut().unwrap();
                runtime.runtime_id = "replacement-runtime".into();
                runtime.boot_id = Some("d".repeat(32));
                runtime.provision_generation = Some(2);
                sandbox.lifecycle.runtime_generation_high_water = 2;
            } else {
                sandbox.status = layer2_core::sandbox::SandboxStatus::Paused;
            }
            sandbox.revision += 1;
            store.save_sandbox(revision, sandbox).await.unwrap();
            tokio::time::timeout(Duration::from_secs(5), barrier.wait())
                .await
                .unwrap();
            let response = tokio::time::timeout(Duration::from_secs(5), pending)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            conflict(response).await;
            assert_eq!(runtime.browser_calls.load(Ordering::SeqCst), 1);
        }
    }
}

#[test]
fn browser_openapi_documents_typed_runtime_conflicts_for_all_observation_operations() {
    let document = serde_json::to_value(crate::rest::openapi::document()).unwrap();
    for method in ["get", "put", "delete"] {
        let response =
            &document["paths"]["/api/v1/agents/{id}/browser"][method]["responses"]["409"];
        assert_eq!(
            response["content"]["application/problem+json"]["schema"]["$ref"],
            "#/components/schemas/ProblemDocument"
        );
        assert!(response["description"]
            .as_str()
            .unwrap()
            .contains("browser_runtime_conflict"));
    }
}
