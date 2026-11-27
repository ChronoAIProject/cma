use super::*;

async fn start_catalog(f: &Fixture, cookie: &str, target: serde_json::Value) -> Response {
    f.a.clone()
        .oneshot(
            Request::post("/auth/service-authorization/start")
                .header(header::COOKIE, format!("cma_session={cookie}"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ORIGIN, "https://app.example")
                .body(Body::from(
                    serde_json::json!({
                        "purpose": "skill_catalog",
                        "target": target,
                        "return_to": "/agents/agent",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn catalog_consent_requests_only_ornn_for_the_verified_agent_owner() {
    let f = Fixture::with_services(&["ornn-api"], &["ornn-api"]).await;
    let target = serde_json::json!({"type": "agent", "id": "agent"});
    for (target, cookie, status) in [
        (
            serde_json::json!({"type":"sandbox","id":"sandbox"}),
            &f.cookie,
            StatusCode::FORBIDDEN,
        ),
        (
            serde_json::json!({"type":"agent","id":"other"}),
            &f.cookie,
            StatusCode::NOT_FOUND,
        ),
        (target.clone(), &f.other_cookie, StatusCode::FORBIDDEN),
    ] {
        assert_eq!(start_catalog(&f, cookie, target).await.status(), status);
    }
    let response = start_catalog(&f, &f.cookie, target).await;
    assert_eq!(response.status(), StatusCode::OK);
    let response: serde_json::Value = serde_json::from_str(&body(response).await).unwrap();
    let url = Url::parse(response["hosted_url"].as_str().unwrap()).unwrap();
    let params: std::collections::BTreeMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(params["scope"], "ornn-api");
    let callback = get(
        &f.b,
        &f.cookie,
        &format!("{CALLBACK_PATH}?state={}&code=allow", params["state"]),
    )
    .await;
    assert_eq!(callback.status(), StatusCode::OK);
    assert!(!callback.headers().contains_key(header::SET_COOKIE));
    let page = body(callback).await;
    assert!(page.contains("Sign in again"));
    assert!(page.contains("/agents/agent"));
}
