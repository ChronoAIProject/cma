use super::*;
use axum::body::{to_bytes, Body};
use axum::http::Request;
use tower::ServiceExt as _;

fn application(fixture: &Fixture) -> Router {
    crate::auth::router(fixture.auth.clone()).layer(axum::middleware::from_fn_with_state(
        fixture.auth.clone(),
        crate::auth::attach_subject,
    ))
}

async fn credential(fixture: &Fixture, cookie: bool) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if cookie {
        let login = fixture
            .auth
            .issue_browser_login(SubjectId::new("owner"))
            .await
            .unwrap();
        headers.insert(
            header::COOKIE,
            format!("cma_session={login}").parse().unwrap(),
        );
    } else {
        headers.insert(
            header::AUTHORIZATION,
            "Bearer independent-fixture-token".parse().unwrap(),
        );
    }
    headers
}

async fn session(app: &Router, headers: &HeaderMap) -> (StatusCode, Value) {
    let mut request = Request::get("/auth/session").body(Body::empty()).unwrap();
    *request.headers_mut() = headers.clone();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("fixture-token"));
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn session_verifies_once_per_request_and_observes_changed_permissions_and_revocation() {
    for cookie in [false, true] {
        let fixture = Fixture::new().await;
        let retained = fixture.seed().await;
        fixture.auth.metadata().await.unwrap();
        fixture.control.requests.store(0, Ordering::SeqCst);
        let app = application(&fixture);
        let headers = credential(&fixture, cookie).await;

        let (status, body) = session(&app, &headers).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            json!({
                "authenticated": true, "subject": "owner", "visitor": false,
                "usage_reporting": {"measured_visible": false},
                "capabilities": ["use_own_sandboxes"], "display_name": "Live Owner",
            })
        );
        assert_eq!(fixture.control.requests.load(Ordering::SeqCst), 2);

        *fixture.control.userinfo.write().await =
            Reply::json(json!({"sub": "owner", "name": "Current Owner"}));
        *fixture.control.introspection.write().await =
            Reply::json(json!({"active": true, "sub": "owner", "permissions": []}));
        let (status, body) = session(&app, &headers).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["capabilities"], json!([]));
        assert_eq!(body["display_name"], "Current Owner");
        assert_eq!(fixture.control.requests.load(Ordering::SeqCst), 4);

        *fixture.control.introspection.write().await = Reply::json(json!({"active": false}));
        let (status, body) = session(&app, &headers).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body, json!({"authenticated": false, "silent_sign_in": true}));
        assert_eq!(fixture.control.requests.load(Ordering::SeqCst), 6);
        fixture.assert_unchanged(&retained).await;
    }
}

#[tokio::test]
async fn session_provider_failure_remains_unavailable_and_next_request_reverifies() {
    for cookie in [false, true] {
        for stage in ["userinfo", "introspection"] {
            let fixture = Fixture::new().await;
            let retained = fixture.seed().await;
            fixture.auth.metadata().await.unwrap();
            fixture.control.requests.store(0, Ordering::SeqCst);
            let app = application(&fixture);
            let headers = credential(&fixture, cookie).await;
            let reply = if stage == "userinfo" {
                &fixture.control.userinfo
            } else {
                &fixture.control.introspection
            };
            reply.write().await.status = StatusCode::TOO_MANY_REQUESTS;
            let userinfo = Arc::new(Gate::default());
            let introspection = Arc::new(Gate::default());
            fixture.control.userinfo.write().await.gate = Some(userinfo.clone());
            fixture.control.introspection.write().await.gate = Some(introspection.clone());
            let pending = {
                let app = app.clone();
                let headers = headers.clone();
                tokio::spawn(async move { session(&app, &headers).await })
            };
            // Both provider requests now start together. Reach both before
            // releasing the failure so cancellation cannot race request counts.
            tokio::time::timeout(Duration::from_secs(2), async {
                tokio::join!(
                    userinfo.reached.notified(),
                    introspection.reached.notified()
                );
            })
            .await
            .unwrap();
            fixture.control.userinfo.write().await.gate = None;
            fixture.control.introspection.write().await.gate = None;
            userinfo.release.notify_one();
            introspection.release.notify_one();
            let (status, body) = pending.await.unwrap();
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(body["code"], "store_failed");
            assert_eq!(fixture.control.requests.load(Ordering::SeqCst), 2);

            reply.write().await.status = StatusCode::OK;
            let (status, body) = session(&app, &headers).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body["capabilities"], json!(["use_own_sandboxes"]));
            assert_eq!(fixture.control.requests.load(Ordering::SeqCst), 4);
            fixture.assert_unchanged(&retained).await;
        }
    }
}

#[tokio::test]
async fn session_without_credentials_or_after_browser_logout_stays_unauthenticated() {
    let fixture = Fixture::new().await;
    fixture.seed().await;
    let app = application(&fixture);
    assert_eq!(
        session(&app, &HeaderMap::new()).await,
        (
            StatusCode::UNAUTHORIZED,
            json!({"authenticated": false, "silent_sign_in": true})
        )
    );
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst), 0);

    let headers = credential(&fixture, true).await;
    assert_eq!(session(&app, &headers).await.0, StatusCode::OK);
    let login =
        crate::auth::session_routes::request_cookie(&headers, crate::auth::AUTH_COOKIE).unwrap();
    fixture.auth.revoke_browser_login(login).await.unwrap();
    let before = fixture.control.requests.load(Ordering::SeqCst);
    assert_eq!(
        session(&app, &headers).await,
        (
            StatusCode::UNAUTHORIZED,
            json!({"authenticated": false, "silent_sign_in": true})
        )
    );
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst), before);
}
