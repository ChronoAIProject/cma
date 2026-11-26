use super::*;
use crate::auth::persistence::LoginAttempt;

fn cookie_value(response: &Response, name: &str) -> String {
    cookies(response)
        .into_iter()
        .find(|value| value.starts_with(&format!("{name}=")))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

async fn legacy_attempt(f: &Fixture) -> (String, String) {
    f.store
        .put_login_attempt(LoginAttempt {
            id: SecretDigest::identify(ATTEMPT_DOMAIN, "predecessor-state"),
            browser_binding: SecretDigest::identify(BINDING_DOMAIN, "predecessor-binding"),
            verifier: SecretValue::new("predecessor-pkce".into()),
            return_to: Some("/agent-manager".into()),
            actor_link_intent: None,
            service_authorization: None,
            silent: false,
            consent_requested: false,
            custody_reserved: false,
            started_at: None,
            expires_at: SystemTime::now() + ATTEMPT_LIFETIME,
        })
        .await
        .unwrap();
    (
        "/auth/callback?state=predecessor-state&code=test-code".into(),
        "cma_login_predecessor-stat=predecessor-binding".into(),
    )
}

#[tokio::test]
async fn predecessor_start_completes_with_fresh_session_and_logout_still_fences_it() {
    let f = Fixture::new(StatusCode::OK, false).await;
    let (path, binding) = legacy_attempt(&f).await;
    let response = call(&f.second, &path, &binding).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/agent-manager");
    let session = cookie_value(&response, AUTH_COOKIE);
    assert_ne!(session.split_once('=').unwrap().1, "predecessor-binding");
    assert!(f
        .store
        .browser_login(&SecretDigest::identify(
            AUTH_COOKIE_DOMAIN,
            "predecessor-binding"
        ))
        .await
        .unwrap()
        .is_none());
    call(&f.app, "/auth/logout", &binding).await;
    assert!(f
        .store
        .browser_login(&SecretDigest::identify(
            AUTH_COOKIE_DOMAIN,
            session.split_once('=').unwrap().1
        ))
        .await
        .unwrap()
        .is_none());

    let f = Fixture::new(StatusCode::OK, true).await;
    let (path, binding) = legacy_attempt(&f).await;
    let task = {
        let app = f.app.clone();
        let binding = binding.clone();
        tokio::spawn(async move { call(&app, &path, &binding).await })
    };
    tokio::time::timeout(Duration::from_secs(3), f.reached.notified())
        .await
        .unwrap();
    call(&f.second, "/auth/logout", &binding).await;
    f.release.notify_one();
    assert_eq!(task.await.unwrap().status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn logout_before_legacy_adoption_cannot_be_resurrected() {
    let f = Fixture::new(StatusCode::OK, false).await;
    let (path, binding) = legacy_attempt(&f).await;
    call(&f.second, "/auth/logout", &binding).await;
    assert_eq!(
        call(&f.app, &path, &binding).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn explicit_success_reenables_silent_entry_but_delayed_response_cannot_undo_logout() {
    let f = Fixture::new(StatusCode::OK, false).await;
    let (path, binding) = f.start(false, "/agent-manager").await;
    let response = call(&f.app, &path, &format!("{binding}; cma_signed_out=1")).await;
    let signed_in = cookie_value(&response, "cma_signed_in");
    let reenabling = call(
        &f.app,
        "/auth/login?prompt=none",
        &format!("cma_signed_out=1; {signed_in}"),
    )
    .await;
    assert!(reenabling.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .contains("prompt=none"));
    // The old success was produced first but its cookies arrive after logout.
    let session = cookie_value(&response, AUTH_COOKIE);
    let logout = call(&f.second, "/auth/logout", &binding).await;
    let signed_out = cookie_value(&logout, "cma_signed_out");
    let late = call(
        &f.app,
        "/auth/login?prompt=none",
        &format!("{signed_out}; {signed_in}; {session}"),
    )
    .await;
    assert_eq!(late.headers()[header::LOCATION], "/");
    assert!(cookies(&late).is_empty());
    assert!(f
        .store
        .browser_login(&SecretDigest::identify(
            AUTH_COOKIE_DOMAIN,
            session.split_once('=').unwrap().1
        ))
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn only_same_origin_logout_sets_preference_but_every_logout_revokes() {
    for (header_name, header_value, expected) in [
        ("sec-fetch-site", "same-origin", true),
        ("sec-fetch-site", "none", true),
        ("origin", "https://cma.example", true),
        ("sec-fetch-site", "cross-site", false),
        ("sec-fetch-site", "same-site", false),
        ("origin", "https://foreign.example", false),
    ] {
        let f = Fixture::new(StatusCode::OK, false).await;
        let (path, binding) = f.start(false, "/agent-manager").await;
        let response = call(&f.app, &path, &binding).await;
        let session = cookie_value(&response, AUTH_COOKIE);
        let logout = f
            .second
            .clone()
            .oneshot(
                Request::get("/auth/logout")
                    .header(header_name, header_value)
                    .header(header::COOKIE, &session)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(logout.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            cookies(&logout)
                .iter()
                .any(|value| value.starts_with("cma_signed_out=")),
            expected
        );
        assert!(f
            .store
            .browser_login(&SecretDigest::identify(
                AUTH_COOKIE_DOMAIN,
                session.split_once('=').unwrap().1
            ))
            .await
            .unwrap()
            .is_none());
    }
}

#[tokio::test]
async fn consent_renews_only_live_pending_custody() {
    let f = Fixture::new(StatusCode::OK, false).await;
    let binding = SecretDigest::identify(AUTH_COOKIE_DOMAIN, "slow-consent");
    let soon = SystemTime::now() + Duration::from_secs(2);
    assert!(f
        .store
        .reserve_browser_login(binding.clone(), soon)
        .await
        .unwrap());
    let response = begin_login_with_intent(
        f.auth.clone(),
        Some("/agent-manager".into()),
        None,
        None,
        true,
        Some("slow-consent"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    tokio::time::sleep(Duration::from_millis(2100)).await;
    let url = reqwest::Url::parse(response.headers()[header::LOCATION].to_str().unwrap()).unwrap();
    let state = url
        .query_pairs()
        .find(|(key, _)| key == "state")
        .unwrap()
        .1
        .into_owned();
    let binding_cookie = cookies(&response)[0].split(';').next().unwrap().to_owned();
    let complete = call(
        &f.app,
        &format!("/auth/callback?state={state}&code=test-code"),
        &binding_cookie,
    )
    .await;
    assert!(cookies(&complete)
        .iter()
        .any(|value| value.starts_with("cma_session=")));
    assert!(cookies(&response)[0].contains(&format!(
        "Max-Age={}",
        crate::auth::BROWSER_BINDING_LIFETIME.as_secs()
    )));
    assert_eq!(
        crate::auth::BROWSER_BINDING_LIFETIME,
        ATTEMPT_LIFETIME + Duration::from_secs(60)
    );
}

#[tokio::test]
async fn hint_respects_preference_order_without_a_round_trip() {
    let auth = AuthState::in_memory(
        AuthMode::Provider {
            base_url: "http://unused.invalid".into(),
            client_id: "test".into(),
            client_secret: "fixture".into(),
            public_url: "https://cma.example".into(),
        },
        None,
    )
    .with_silent_sign_in(true);
    let app = router(auth);
    for (cookie, offered) in [
        ("", true),
        ("cma_signed_out=1", false),
        ("cma_signed_out=1; cma_signed_in=2", false),
        ("cma_signed_out=3; cma_signed_in=3", false),
        ("cma_signed_out=invalid; cma_signed_in=4", false),
    ] {
        let response = call(&app, "/auth/session", cookie).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(
            body,
            json!({"authenticated":false,"silent_sign_in":offered})
        );
    }
}

#[tokio::test]
async fn complete_encoded_login_url_obeys_the_shared_hop_budget() {
    let spec: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../layer1-public/tests/fixtures/browser-auth-limits.json"
    )))
    .unwrap();
    assert_eq!(
        MAX_LOGIN_URL_BYTES,
        spec["max_login_url_bytes"].as_u64().unwrap() as usize
    );
    let f = Fixture::new(StatusCode::OK, false).await;
    for prompt in [false, true] {
        let mut url = reqwest::Url::parse("https://cma.example/auth/login").unwrap();
        url.query_pairs_mut()
            .append_pair("return_to", "/bots/new?setup=");
        if prompt {
            url.query_pairs_mut().append_pair("prompt", "none");
        }
        let overhead = url.as_str().len();
        for length in [
            MAX_LOGIN_URL_BYTES - 1,
            MAX_LOGIN_URL_BYTES,
            MAX_LOGIN_URL_BYTES + 1,
        ] {
            let mut request = reqwest::Url::parse("https://cma.example/auth/login").unwrap();
            request.query_pairs_mut().append_pair(
                "return_to",
                &format!("/bots/new?setup={}", "a".repeat(length - overhead)),
            );
            if prompt {
                request.query_pairs_mut().append_pair("prompt", "none");
            }
            assert_eq!(request.as_str().len(), length);
            let response = call(
                &f.app,
                &format!("{}?{}", request.path(), request.query().unwrap()),
                "",
            )
            .await;
            assert_eq!(
                response.status(),
                if length <= MAX_LOGIN_URL_BYTES {
                    StatusCode::SEE_OTHER
                } else {
                    StatusCode::BAD_REQUEST
                }
            );
            if length > MAX_LOGIN_URL_BYTES {
                assert!(cookies(&response).is_empty());
            }
        }
    }
    for value in ["/bots/new?setup=界", "/sandboxes/s?tab=é"] {
        let response = call(
            &f.app,
            &format!("/auth/login?return_to={}", urlencode(value)),
            "",
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(cookies(&response).is_empty());
    }
}

#[tokio::test]
async fn durable_preference_survives_reordered_logout_responses_and_clock_skew() {
    let f = Fixture::new(StatusCode::OK, false).await;
    let first_logout = call(&f.app, "/auth/logout", "").await;
    let stale_out = cookie_value(&first_logout, "cma_signed_out");
    let (path, binding) = f.start(false, "/agent-manager").await;
    // Simulate a fast clock on the start replica without changing expiry.
    let state = reqwest::Url::parse(&format!("https://cma.example{path}"))
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == "state")
        .unwrap()
        .1
        .into_owned();
    let raw = binding.split_once('=').unwrap().1;
    let mut attempt = f
        .store
        .take_login_attempt(
            &SecretDigest::identify(ATTEMPT_DOMAIN, &state),
            &SecretDigest::identify(BINDING_DOMAIN, raw),
        )
        .await
        .unwrap()
        .unwrap();
    attempt.started_at = Some(SystemTime::now() + Duration::from_secs(60));
    f.store.put_login_attempt(attempt).await.unwrap();
    let complete = call(&f.app, &path, &format!("{binding}; {stale_out}")).await;
    let proof = cookie_value(&complete, "cma_signed_in");
    let session = cookie_value(&complete, AUTH_COOKIE);
    let allowed = call(
        &f.app,
        "/auth/login?prompt=none",
        &format!("{proof}; {stale_out}"),
    )
    .await;
    assert!(allowed.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .contains("prompt=none"));
    call(
        &f.second,
        "/auth/logout",
        &format!("{session}; {proof}; {stale_out}"),
    )
    .await;
    // L1's old cookie overwrites L2's newer time; the durable fence still wins.
    let declined = call(
        &f.app,
        "/auth/login?prompt=none",
        &format!("{proof}; {stale_out}"),
    )
    .await;
    assert_eq!(declined.headers()[header::LOCATION], "/");
    assert!(cookies(&declined).is_empty());
}

#[tokio::test]
async fn cross_site_logout_does_not_reactivate_an_older_preference() {
    let f = Fixture::new(StatusCode::OK, false).await;
    let (path, binding) = f.start(false, "/agent-manager").await;
    let response = call(&f.app, &path, "cma_signed_out=1").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = call(&f.app, &path, &format!("{binding}; cma_signed_out=1")).await;
    let proof = cookie_value(&response, "cma_signed_in");
    let session = cookie_value(&response, AUTH_COOKIE);
    let foreign = f
        .second
        .clone()
        .oneshot(
            Request::get("/auth/logout")
                .header("sec-fetch-site", "cross-site")
                .header(
                    header::COOKIE,
                    format!("{proof}; {session}; cma_signed_out=1"),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(cookies(&foreign)
        .iter()
        .all(|value| !value.starts_with("cma_signed_out=")));
    let allowed = call(
        &f.app,
        "/auth/login?prompt=none",
        &format!("{proof}; cma_signed_out=1"),
    )
    .await;
    assert!(allowed.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .contains("prompt=none"));
}

#[tokio::test]
async fn binding_only_logout_revokes_an_issued_session_before_callback_response_delivery() {
    for legacy in [false, true] {
        let f = Fixture::new(StatusCode::OK, false).await;
        let (path, binding) = if legacy {
            legacy_attempt(&f).await
        } else {
            f.start(false, "/agent-manager").await
        };
        // The server finished the callback, but the browser has not received
        // its response and therefore still sends only its original binding.
        let in_flight_response = call(&f.app, &path, &binding).await;
        assert_eq!(
            in_flight_response.headers()[header::LOCATION],
            "/agent-manager"
        );
        let session = cookie_value(&in_flight_response, AUTH_COOKIE);
        let proof = cookie_value(&in_flight_response, "cma_signed_in");
        let id = SecretDigest::identify(AUTH_COOKIE_DOMAIN, session.split_once('=').unwrap().1);
        assert!(f.store.browser_login(&id).await.unwrap().is_some());
        assert!(!binding.contains("cma_session="));
        let logout = call(&f.second, "/auth/logout", &binding).await;
        assert_eq!(logout.status(), StatusCode::SEE_OTHER);
        assert!(f.store.browser_login(&id).await.unwrap().is_none());
        let signed_out = cookie_value(&logout, "cma_signed_out");
        let late = call(
            &f.app,
            "/auth/login?prompt=none",
            &format!("{signed_out}; {proof}; {session}"),
        )
        .await;
        assert_eq!(late.headers()[header::LOCATION], "/");
        assert!(cookies(&late).is_empty());
    }
}

#[tokio::test]
async fn binding_cookie_has_only_the_attempt_window_and_in_flight_margin() {
    let f = Fixture::new(StatusCode::OK, false).await;
    for prompt in ["", "?prompt=none"] {
        let response = call(&f.app, &format!("/auth/login{prompt}"), "").await;
        let values = cookies(&response);
        assert_eq!(values.len(), 1);
        assert!(values[0].ends_with("Max-Age=660"));
        assert_eq!(
            crate::auth::BROWSER_BINDING_LIFETIME,
            ATTEMPT_LIFETIME + Duration::from_secs(60)
        );
    }
}

#[tokio::test]
async fn preference_never_overrides_a_logout_after_its_start_even_with_a_live_proof() {
    let f = Fixture::new(StatusCode::OK, false).await;
    let (path, binding) = f.start(false, "/agent-manager").await;
    let complete = call(&f.app, &path, &binding).await;
    let proof = cookie_value(&complete, "cma_signed_in");
    let started: u64 = proof
        .split_once('=')
        .unwrap()
        .1
        .split_once('.')
        .unwrap()
        .0
        .parse()
        .unwrap();
    for signed_out in [started, started + 1] {
        let declined = call(
            &f.app,
            "/auth/login?prompt=none",
            &format!("{proof}; cma_signed_out={signed_out}"),
        )
        .await;
        assert_eq!(declined.headers()[header::LOCATION], "/");
        assert!(cookies(&declined).is_empty());
    }
}
