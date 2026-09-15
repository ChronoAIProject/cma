use super::{agents_tests, resource_access_tests::context, shared_agent_tests};
use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use layer2_core::resource_access::*;
use layer2_core::sandbox::{agent_artifacts::AgentArtifactService, store::AgentLogStore};
use layer2_core::AgentId;
use serde_json::{json, Value};
use tower::ServiceExt;

fn request(
    method: Method,
    path: &str,
    subject: &str,
    key: Option<&str>,
    body: Option<Value>,
) -> Request<Body> {
    let mut request = agents_tests::request(method, path, subject, key, body);
    request.extensions_mut().insert(context(subject));
    request
        .extensions_mut()
        .insert(crate::DelegatedCredential::new("request-credential".into()));
    request
}
fn body() -> Value {
    json!({"name":"report.txt", "media_type":"text/plain", "content_base64":STANDARD.encode("approved bytes\n")})
}
const ROOT: &str = "/api/v1/agents/agt_shared_read/artifacts";

#[test]
fn artifact_openapi_exposes_all_routes_and_safe_metadata_only() {
    let document = serde_json::to_value(super::openapi::document()).unwrap();
    for (path, method) in [
        ("/api/v1/agents/{id}/artifacts", "post"),
        ("/api/v1/agents/{id}/artifacts", "get"),
        ("/api/v1/agents/{id}/artifacts/{artifact_id}", "get"),
        (
            "/api/v1/agents/{id}/artifacts/{artifact_id}/download",
            "get",
        ),
    ] {
        assert!(document["paths"][path][method].is_object());
    }
    let properties = document["components"]["schemas"]["AgentArtifactView"]["properties"]
        .as_object()
        .unwrap();
    assert_eq!(properties.len(), 7);
    assert!(!properties.contains_key("authorization"));
    assert!(!properties.contains_key("content_base64"));
}

#[tokio::test]
async fn actual_artifact_routes_publish_list_read_and_download_only_approved_bytes() {
    let (fixture, access, authority, share) =
        shared_agent_tests::fixture(shared_agent_tests::Reply::Member).await;
    let router = super::agent_artifacts_router(AgentArtifactService::new(
        fixture.store.clone(),
        access.clone(),
    ));
    let id = AgentId("agt_shared_read".into());
    let logs = fixture.store.agent_log(&id, 0, 200).await.unwrap();
    let target = ResourceTarget::Agent {
        agent_id: id.clone(),
    };
    let lineage = fixture
        .store
        .resource_access_snapshot(&target)
        .await
        .unwrap()
        .lineage;
    let (status, value) = agents_tests::json_response(
        &router,
        request(Method::POST, ROOT, "owner-a", Some("one"), Some(body())),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{value}");
    assert_eq!(value.as_object().unwrap().len(), 7);
    for field in [
        "content_base64",
        "binding",
        "authorization",
        "owner",
        "actor",
        "sandbox_id",
    ] {
        assert!(value.get(field).is_none());
    }
    let artifact_id = value["id"].as_str().unwrap();
    let read_path = format!("{ROOT}/{artifact_id}");
    let download_path = format!("{read_path}/download");
    assert_eq!(
        agents_tests::json_response(
            &router,
            request(Method::POST, ROOT, "owner-a", Some("one"), Some(body()))
        )
        .await,
        (StatusCode::CREATED, value.clone())
    );
    for subject in ["owner-a", "recipient"] {
        let (status, page) =
            agents_tests::json_response(&router, request(Method::GET, ROOT, subject, None, None))
                .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(page, json!({"items":[value]}));
        let response = agents_tests::json_response(
            &router,
            request(Method::GET, &read_path, subject, None, None),
        )
        .await;
        assert_eq!(response, (StatusCode::OK, value.clone()));
        let downloaded = router
            .clone()
            .oneshot(request(Method::GET, &download_path, subject, None, None))
            .await
            .unwrap();
        assert_eq!(downloaded.status(), StatusCode::OK);
        assert_eq!(
            downloaded.headers()[header::CONTENT_TYPE],
            "application/octet-stream"
        );
        assert_eq!(
            downloaded.headers()[header::CACHE_CONTROL],
            "private, no-store"
        );
        assert_eq!(
            downloaded.headers()[header::X_CONTENT_TYPE_OPTIONS],
            "nosniff"
        );
        assert!(downloaded.headers()[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .starts_with("attachment; filename=\"artifact-"));
        assert_eq!(
            to_bytes(downloaded.into_body(), 1024).await.unwrap(),
            "approved bytes\n"
        );
    }
    let mut legacy = request(Method::GET, &download_path, "owner-a", None, None);
    legacy.extensions_mut().remove::<CallerContext>();
    assert_eq!(
        router.clone().oneshot(legacy).await.unwrap().status(),
        StatusCode::OK
    );
    assert_eq!(
        agents_tests::json_response(
            &router,
            request(
                Method::POST,
                ROOT,
                "recipient",
                Some("cannot-publish"),
                Some(body())
            )
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .store
            .resource_access_snapshot(&target)
            .await
            .unwrap()
            .lineage,
        lineage
    );
    assert_eq!(fixture.store.agent_log(&id, 0, 200).await.unwrap(), logs);
    *authority.0.lock().unwrap() = shared_agent_tests::Reply::Missing;
    for path in [ROOT.to_owned(), read_path.clone(), download_path.clone()] {
        assert_eq!(
            agents_tests::json_response(
                &router,
                request(Method::GET, &path, "recipient", None, None)
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
    access
        .change_share(
            &context("owner-a"),
            None,
            ShareCommand {
                selector: share.selector.clone(),
                change: ShareChange::Revoke {
                    share_id: share.id.clone(),
                },
                expected_share_revision: Some(share.revision),
                idempotency_key: "revoke-artifacts".into(),
            },
        )
        .await
        .unwrap();
    *authority.0.lock().unwrap() = shared_agent_tests::Reply::Member;
    assert_eq!(
        agents_tests::json_response(
            &router,
            request(Method::GET, &download_path, "recipient", None, None)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        router
            .oneshot(request(Method::GET, &download_path, "owner-a", None, None))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn artifact_routes_conceal_foreign_agents_and_reject_injected_authority_or_invalid_content() {
    let (fixture, access, _, _) =
        shared_agent_tests::fixture(shared_agent_tests::Reply::Member).await;
    let router = super::agent_artifacts_router(AgentArtifactService::new(fixture.store, access));
    let (_, first) = agents_tests::json_response(
        &router,
        request(Method::POST, ROOT, "owner-a", Some("one"), Some(body())),
    )
    .await;
    let id = first["id"].as_str().unwrap();
    for path in [
        "/api/v1/agents/sibling/artifacts".to_owned(),
        format!("/api/v1/agents/sibling/artifacts/{id}"),
        format!("/api/v1/agents/sibling/artifacts/{id}/download"),
    ] {
        assert_eq!(
            router
                .clone()
                .oneshot(request(Method::GET, &path, "recipient", None, None))
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    for change in [
        json!({"owner":"recipient"}),
        json!({"url":"http://internal"}),
        json!({"path":"/home/cma/.credentials"}),
    ] {
        let mut altered = body();
        altered
            .as_object_mut()
            .unwrap()
            .extend(change.as_object().unwrap().clone());
        assert_eq!(
            router
                .clone()
                .oneshot(request(
                    Method::POST,
                    ROOT,
                    "owner-a",
                    Some("invalid"),
                    Some(altered)
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    for (field, value) in [
        ("name", "bad\r\nheader.txt"),
        ("name", "../private"),
        ("media_type", "text/html"),
        ("content_base64", "http://internal"),
    ] {
        let mut altered = body();
        altered[field] = value.into();
        let (status, problem) = agents_tests::json_response(
            &router,
            request(
                Method::POST,
                ROOT,
                "owner-a",
                Some("invalid"),
                Some(altered),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(problem["code"], "invalid_artifact_request");
    }
    for query in ["limit=0", "limit=51", "cursor=../secret", "owner=recipient"] {
        assert_eq!(
            agents_tests::json_response(
                &router,
                request(
                    Method::GET,
                    &format!("{ROOT}?{query}"),
                    "owner-a",
                    None,
                    None
                )
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        router
            .clone()
            .oneshot(request(Method::POST, ROOT, "owner-a", None, Some(body())))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    let mut changed = body();
    changed["name"] = "other.txt".into();
    let (status, problem) = agents_tests::json_response(
        &router,
        request(Method::POST, ROOT, "owner-a", Some("one"), Some(changed)),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(problem["code"], "idempotency_key_reused");
    let anonymous = Request::get(ROOT).body(Body::empty()).unwrap();
    assert_eq!(
        router.oneshot(anonymous).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
}
