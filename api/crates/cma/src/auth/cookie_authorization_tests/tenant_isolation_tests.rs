//! Real auth middleware + real document handlers; only NyxID transport is doubled.
use super::*;
use crate::auth::persistence;
use axum::body::{to_bytes, Body};
use axum::http::Request;
use layer2_core::{document_artifacts::*, Caller};
use tower::ServiceExt;

async fn send(
    app: &Router,
    headers: &HeaderMap,
    method: &str,
    path: &str,
    body: Value,
) -> (u16, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header("idempotency-key", "stale-session-write")
        .body(Body::from(body.to_string()))
        .unwrap();
    request.headers_mut().extend(headers.clone());
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    let text = String::from_utf8_lossy(&bytes);
    for forbidden in [
        "must-not-leak",
        "fixture-token",
        "client-secret",
        "refresh-token",
    ] {
        assert!(!text.contains(forbidden));
    }
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn tenant_isolation_warm_sessions_cannot_read_or_write_after_revocation() {
    for cause in [
        "provider-cookie",
        "provider-bearer",
        "logout",
        "expired",
        "scope",
    ] {
        let fixture = Fixture::new().await;
        fixture.seed().await;
        let rows = Arc::new(memory::MemoryDocuments::default());
        let service = DocumentService::new(
            rows.clone(),
            Arc::new(memory::MemoryDocumentBlobs::default()),
        );
        let owner = Caller::new(
            SubjectId::new("owner"),
            Authorization::new([Capability::UseOwnSandboxes]),
        );
        let document = service
            .create(
                &owner,
                "seed",
                CreateDocument {
                    title: "Private fixture".into(),
                    markdown: "private-isolation-payload".into(),
                    origin_agent_id: None,
                },
            )
            .await
            .unwrap();
        let login = fixture
            .auth
            .issue_browser_login(owner.subject.clone())
            .await
            .unwrap();
        let mut headers = HeaderMap::new();
        if cause == "provider-bearer" {
            headers.insert(
                header::AUTHORIZATION,
                "Bearer independent-fixture-token".parse().unwrap(),
            );
        } else {
            headers.insert(
                header::COOKIE,
                format!("cma_session={login}").parse().unwrap(),
            );
        }
        let app = layer1_public::rest::documents_router(service.clone())
            .merge(crate::auth::router(fixture.auth.clone()))
            .layer(axum::middleware::from_fn_with_state(
                fixture.auth.clone(),
                crate::auth::attach_subject,
            ));
        let base = format!("/api/v1/documents/{}", document.id);
        // Warm both the browser session and resource path before removing authority.
        assert_eq!(
            send(&app, &headers, "GET", "/auth/session", Value::Null)
                .await
                .0,
            200
        );
        for suffix in [
            "",
            "/source",
            "/comments",
            "/revisions",
            "/shares",
            "/share-links",
        ] {
            assert_eq!(
                send(
                    &app,
                    &headers,
                    "GET",
                    &format!("{base}{suffix}"),
                    Value::Null
                )
                .await
                .0,
                200
            );
        }
        let warm_write = json!({"expected_revision":1,"title":"Allowed warm update"});
        assert_eq!(
            send(&app, &headers, "PATCH", &base, warm_write.clone())
                .await
                .0,
            200
        );
        let before = serde_json::to_value(rows.get(&document.id).await.unwrap().unwrap()).unwrap();
        match cause {
            "provider-cookie" | "provider-bearer" => {
                *fixture.control.introspection.write().await = Reply::json(json!({"active":false}));
            }
            "scope" => {
                *fixture.control.introspection.write().await =
                    Reply::json(json!({"active":true,"sub":"owner","permissions":[]}));
            }
            "logout" => fixture.auth.revoke_browser_login(&login).await.unwrap(),
            "expired" => {
                fixture
                    .store
                    .put_browser_login(persistence::BrowserLogin {
                        id: persistence::SecretDigest::identify("browser-login", &login),
                        subject: owner.subject.clone(),
                        expires_at: SystemTime::UNIX_EPOCH,
                    })
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        // DocumentService conceals resources when UseOwnSandboxes is absent.
        let expected = if cause == "scope" { 404 } else { 401 };
        for suffix in [
            "",
            "/source",
            "/comments",
            "/revisions",
            "/shares",
            "/share-links",
        ] {
            let (status, body) = send(
                &app,
                &headers,
                "GET",
                &format!("{base}{suffix}"),
                Value::Null,
            )
            .await;
            assert_eq!(status, expected, "{cause} {suffix}");
            assert_eq!(
                body["code"],
                if cause == "scope" {
                    "document_not_found"
                } else {
                    "not_authenticated"
                }
            );
            assert!(!body.to_string().contains("private-isolation-payload"));
        }
        assert_eq!(
            send(&app, &headers, "GET", "/api/v1/documents", Value::Null)
                .await
                .0,
            expected
        );
        let writes = [
            ("PATCH", base.clone(), warm_write),
            (
                "POST",
                format!("{base}/comments"),
                json!({"body":"unauthorized"}),
            ),
            (
                "PUT",
                format!("{base}/shares/foreign"),
                json!({"expected_revision":1,"permission":"read"}),
            ),
        ];
        for (method, path, body) in writes {
            assert_eq!(
                send(&app, &headers, method, &path, body).await.0,
                expected,
                "{cause} {method}"
            );
        }
        assert_eq!(
            serde_json::to_value(rows.get(&document.id).await.unwrap().unwrap()).unwrap(),
            before
        );
        assert_eq!(
            service
                .source(&owner, &document.id, None, None)
                .await
                .unwrap()
                .markdown,
            "private-isolation-payload"
        );
    }
}
