use super::*;

async fn refusal_code(response: Response) -> Option<String> {
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), BODY_LIMIT).await.unwrap()).unwrap();
    assert_eq!(body["status"], 401);
    assert!(body.get("items").is_none());
    body.get("code").and_then(Value::as_str).map(str::to_owned)
}

fn without_cookie(f: &Fixture) -> HttpRequest<Body> {
    let mut request = f.request(Method::GET, "/v1/channels");
    request.headers_mut().remove(header::COOKIE);
    request
}

#[tokio::test]
async fn each_browser_credential_refusal_names_its_closed_reason_before_dispatch() {
    let f = Fixture::new().await;
    let mut cases: Vec<(&str, HttpRequest<Body>)> = Vec::new();

    let mut bearer = f.request(Method::GET, "/v1/channels");
    bearer.headers_mut().insert(
        header::AUTHORIZATION,
        HeaderValue::from_static("Bearer another"),
    );
    cases.push(("portal_bearer_presented", bearer));

    cases.push(("portal_cookie_missing", without_cookie(&f)));

    let mut other_cookie = without_cookie(&f);
    other_cookie.headers_mut().insert(
        header::COOKIE,
        HeaderValue::from_static("__Host-cma-event-gateway=old-independent-cookie"),
    );
    cases.push(("portal_cookie_missing", other_cookie));

    let mut duplicate = f.request(Method::GET, "/v1/channels");
    duplicate.headers_mut().append(
        header::COOKIE,
        HeaderValue::from_static("cma_session=another"),
    );
    cases.push(("portal_cookie_ambiguous", duplicate));

    let mut empty = without_cookie(&f);
    empty
        .headers_mut()
        .insert(header::COOKIE, HeaderValue::from_static("cma_session="));
    cases.push(("portal_cookie_invalid", empty));

    let mut oversized = without_cookie(&f);
    oversized.headers_mut().insert(
        header::COOKIE,
        format!("cma_session={}", "x".repeat(513)).parse().unwrap(),
    );
    cases.push(("portal_cookie_invalid", oversized));

    let mut unknown = without_cookie(&f);
    unknown.headers_mut().insert(
        header::COOKIE,
        HeaderValue::from_static("cma_session=not-a-known-login"),
    );
    cases.push(("portal_login_unknown", unknown));

    for (expected, request) in cases {
        let code = refusal_code(f.send(request).await).await;
        assert_eq!(code.as_deref(), Some(expected));
    }
    assert_eq!(f.control.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn revoked_login_and_inactive_provider_evidence_keep_distinct_reasons() {
    let f = Fixture::new().await;
    *f.control.active.write().await = false;
    let inactive = refusal_code(f.send(f.request(Method::GET, "/v1/channels")).await).await;
    assert_eq!(inactive.as_deref(), Some("portal_authorization_missing"));
    *f.control.active.write().await = true;
    f.auth.revoke_browser_login(&f.cookie).await.unwrap();
    let revoked = refusal_code(f.send(f.request(Method::GET, "/v1/channels")).await).await;
    assert_eq!(revoked.as_deref(), Some("portal_login_unknown"));
    assert_eq!(f.control.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn logout_during_the_gateway_read_is_reported_as_a_revoked_login() {
    let f = Fixture::new().await;
    let gate = Arc::new(Gate::default());
    *f.control.gate.write().await = Some(gate.clone());
    let router = f.router.clone();
    let request = f.request(Method::GET, "/v1/channels");
    let pending = tokio::spawn(async move { router.oneshot(request).await.unwrap() });
    tokio::time::timeout(Duration::from_secs(2), gate.reached.notified())
        .await
        .unwrap();
    f.auth.revoke_browser_login(&f.cookie).await.unwrap();
    gate.release.notify_one();
    let code = refusal_code(pending.await.unwrap()).await;
    assert_eq!(code.as_deref(), Some("portal_login_revoked"));
}

#[tokio::test]
async fn a_gateway_refusal_passes_through_without_a_portal_reason() {
    let f = Fixture::new().await;
    *f.control.resource.write().await = Some("https://wrong.example/resource".into());
    let response = f.send(f.request(Method::GET, "/v1/channels")).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = to_bytes(response.into_body(), BODY_LIMIT).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(!text.contains("portal_"), "{text}");
    assert!(!text.contains("test-access"));
    assert_eq!(f.control.calls.load(Ordering::SeqCst), 1);
}
