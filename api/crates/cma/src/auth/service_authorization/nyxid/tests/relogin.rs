use super::*;
use axum::{
    body::Body,
    http::{header, Request},
};
use tower::ServiceExt as _;

fn app(f: &Fixture) -> Router {
    crate::auth::flow::router(f.auth.clone())
        .merge(crate::auth::service_authorization::router(f.auth.clone()))
        .merge(crate::connections::router(
            crate::connections::authorization_fixture(),
            f.auth.clone(),
        ))
        .layer(axum::middleware::from_fn_with_state(
            f.auth.clone(),
            crate::auth::attach_subject,
        ))
}

async fn access_b(f: &Fixture) -> StatusCode {
    let token = f
        .auth
        .delegated_credential_for(&SubjectId::new("owner"))
        .await
        .unwrap()
        .unwrap();
    let AuthMode::Provider { base_url, .. } = f.auth.fixture_mode() else {
        unreachable!()
    };
    f.auth
        .fixture_http()
        .get(format!("{base_url}/api/v1/proxy/s/api-github"))
        .bearer_auth(token.expose())
        .send()
        .await
        .unwrap()
        .status()
}

async fn explicit_sign_in(f: &Fixture) {
    let router = app(f);
    let login = router
        .clone()
        .oneshot(
            Request::get("/auth/login?return_to=%2Fagents%2Foriginal")
                .header(header::HOST, "app.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::SEE_OTHER);
    let url = Url::parse(login.headers()[header::LOCATION].to_str().unwrap()).unwrap();
    let query: BTreeMap<_, _> = url.query_pairs().into_owned().collect();
    assert!(!query.contains_key("service_access_mode"));
    assert!(!query.contains_key("prompt"));
    let binding = login
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("cma_login_"))
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    let path = format!(
        "/auth/callback?code=ordinary-login&state={}",
        query["state"]
    );
    let completed = router
        .oneshot(
            Request::get(path)
                .header(header::HOST, "app.example")
                .header(header::COOKIE, binding)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(completed.status(), StatusCode::SEE_OTHER);
    assert_eq!(completed.headers()[header::LOCATION], "/agents/original");
}

#[tokio::test]
async fn expanded_consent_and_refresh_leave_owner_proxy_token_restricted_until_sign_in() {
    let f = Fixture::new().await;
    assert_eq!(access_b(&f).await, StatusCode::FORBIDDEN);
    *f.peer.consents.lock().await = json!({"consents":[grant(&[DRIVE, GITHUB], None)]});
    assert!(f
        .authority()
        .redeem(
            &f.auth,
            &f.intent(),
            "consent-code",
            &SecretValue::new("proof".into())
        )
        .await
        .unwrap()
        .is_some());
    assert!(f
        .authority()
        .missing(&f.auth, &f.intent())
        .await
        .unwrap()
        .unwrap()
        .is_empty());
    assert_eq!(access_b(&f).await, StatusCode::FORBIDDEN);

    let mut subject = f
        .store
        .subject(&SubjectId::new("owner"))
        .await
        .unwrap()
        .unwrap();
    subject.tokens.as_mut().unwrap().expires_at = SystemTime::UNIX_EPOCH;
    f.store.put_subject(subject).await.unwrap();
    assert_eq!(access_b(&f).await, StatusCode::FORBIDDEN);
    assert!(f
        .peer
        .exchange
        .lock()
        .await
        .iter()
        .any(|form| form["grant_type"] == "refresh_token"));

    explicit_sign_in(&f).await;
    assert_eq!(access_b(&f).await, StatusCode::NO_CONTENT);
    let retained = f
        .store
        .subject(&SubjectId::new("owner"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        retained.tokens.unwrap().access_token.expose(),
        "new-sign-in-fixture"
    );
}

#[tokio::test]
async fn revoked_identity_refuses_status_and_start_but_ordinary_login_remains_available() {
    let f = Fixture::new().await;
    let cookie = f
        .auth
        .issue_browser_login(SubjectId::new("owner"))
        .await
        .unwrap();
    *f.peer.retained_identity.lock().await = json!({"active":false});
    let router = app(&f);
    for (method, path, body) in [
        ("GET", "/auth/service-authorization", String::new()),
        (
            "POST",
            "/auth/service-authorization/start",
            json!({"target":{"type":"agent","id":"agent"},"return_to":"/"}).to_string(),
        ),
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(header::HOST, "app.example")
                    .header(header::ORIGIN, "https://app.example")
                    .header(header::COOKIE, format!("cma_session={cookie}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    assert!(f.peer.exchange.lock().await.is_empty());
    explicit_sign_in(&f).await;
    assert_eq!(access_b(&f).await, StatusCode::NO_CONTENT);
}
