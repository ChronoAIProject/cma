use super::*;
use axum::{body::Body, http::Request};
use layer1_public::AuthenticatedSubject;
use layer2_core::{Authorization, SubjectId};
use tower::ServiceExt;

#[tokio::test]
async fn suggestion_control_route_is_absent_when_disabled() {
    let response = routes(Router::new(), None)
        .oneshot(Request::get(PATH).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore = "requires an isolated Mongo replica set"]
async fn suggestion_control_route_requires_operator_authority() {
    use crate::config::suggestion_control::Target;
    use crate::store::MongoSandboxStore;

    let client = mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let name = format!(
        "cma_suggestion_control_http_{}",
        uuid::Uuid::new_v4().simple()
    );
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let control = SuggestionControl::new(
        &store,
        Target {
            sandbox_id: "sbx_fixture".into(),
            agent_id: "agt_fixture".into(),
        },
    );
    let router = routes(Router::new(), Some(control));
    for (method, path, body) in [
        ("GET", PATH, ""),
        ("POST", PATH, r#"{"mode":"late_ready"}"#),
        ("POST", RELEASE_PATH, r#"{"claim_started_at_ms":1}"#),
    ] {
        for (caps, expected) in [
            (None, StatusCode::UNAUTHORIZED),
            (Some(Capability::UseOwnSandboxes), StatusCode::FORBIDDEN),
        ] {
            let mut request = Request::builder()
                .method(method)
                .uri(path)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap();
            if let Some(cap) = caps {
                request
                    .extensions_mut()
                    .insert(AuthenticatedSubject(SubjectId("owner".into())));
                request.extensions_mut().insert(Authorization::new([cap]));
            }
            assert_eq!(
                router.clone().oneshot(request).await.unwrap().status(),
                expected
            );
        }
    }
    let mut request = Request::get(PATH).body(Body::empty()).unwrap();
    request
        .extensions_mut()
        .insert(AuthenticatedSubject(SubjectId("operator".into())));
    request
        .extensions_mut()
        .insert(Authorization::new([Capability::ConfigurePlatform]));
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        axum::body::to_bytes(response.into_body(), 32)
            .await
            .unwrap(),
        "null"
    );
    client.database(&name).drop().await.unwrap();
}
