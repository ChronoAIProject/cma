//! NyxID sign-in: authorization code with PKCE S256 as a confidential
//! client, the login cookie checked before any NyxID call, the login limiter
//! and the exchange semaphore.
use super::support::{config, metrics, sample, set_cookie, Harness};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use cmaeg::ConsoleConfig;
use cmaeg_core::seal::{seal_console_login, ConsoleLogin};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, sync::atomic::Ordering, time::Duration};

fn query(url: &str) -> BTreeMap<String, String> {
    reqwest::Url::parse(url)
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect()
}

#[tokio::test]
async fn the_full_flow_signs_in_with_a_matching_pkce_verifier() {
    let harness = Harness::enabled().await;
    let (authorize, login_cookie) = harness.start_login("/console?page=2").await;
    assert!(authorize.starts_with(&format!("{}/oauth/authorize?", harness.nyxid.base_url)));
    let params = query(&authorize);
    assert_eq!(params["response_type"], "code");
    assert_eq!(params["client_id"], "console-fixture-client");
    assert_eq!(
        params["redirect_uri"],
        format!("{}/console/callback", harness.base)
    );
    assert_eq!(params["scope"], "openid profile");
    assert_eq!(params["code_challenge_method"], "S256");
    assert_eq!(params["state"].len(), 43);
    assert!(!params.contains_key("resource"));
    assert!(!params.contains_key("requested_service_ids"));
    // The login cookie is sealed: neither the state nor the verifier shows.
    assert!(login_cookie.starts_with("cmaeg1.cl."));
    assert!(!login_cookie.contains(&params["state"]));
    let callback = harness.approve(&authorize).await;
    let response = harness.callback(&callback, Some(&login_cookie)).await;
    assert_eq!(response.status(), 303);
    assert_eq!(response.headers()["location"], "/console?page=2");
    let cookies: Vec<&str> = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|value| value.to_str().unwrap())
        .collect();
    assert!(cookies
        .iter()
        .any(|cookie| cookie.starts_with("__Host-cmaeg-console-login=;")
            && cookie.contains("Max-Age=0")));
    let session = cookies
        .iter()
        .find(|cookie| {
            cookie.starts_with("__Host-cmaeg-console=")
                && !cookie.starts_with("__Host-cmaeg-console=;")
        })
        .unwrap();
    assert!(session.ends_with("; Path=/; Secure; HttpOnly; SameSite=Lax"));
    let sid = set_cookie(&response, "__Host-cmaeg-console").unwrap();
    let calls = harness.nyxid.oauth.token_calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].grant_type, "authorization_code");
    assert!(calls[0].client_authenticated);
    assert_eq!(
        calls[0].redirect_uri.as_deref(),
        Some(params["redirect_uri"].as_str())
    );
    let verifier = calls[0].code_verifier.clone().unwrap();
    assert_eq!(verifier.len(), 86);
    assert_eq!(
        URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())),
        params["code_challenge"]
    );
    let page = harness.get_as("/console", &sid).await.text().await.unwrap();
    assert!(page.contains("…lice"), "{page}");
    assert!(!page.contains("alice"));
    assert_eq!(
        sample(&metrics(), "cmaeg_console_logins_total{outcome=\"ok\"}"),
        1.0
    );
    harness.close().await;
}

#[tokio::test]
async fn the_session_subject_is_rests_creator_and_fills_its_cache() {
    let harness = Harness::enabled().await;
    let (sid, _) = harness.sign_in("alice", "/console").await;
    let access = harness.nyxid.oauth.access_tokens_of("alice");
    assert_eq!(access.len(), 1);
    // The callback resolved the identity with authenticate_token, so REST's
    // own look at the same token is a cache hit.
    let before = harness.nyxid.oauth.users_me_calls.load(Ordering::SeqCst);
    let creator = harness
        .creator_auth
        .authenticate_token(&access[0], None)
        .await
        .unwrap();
    assert_eq!(
        harness.nyxid.oauth.users_me_calls.load(Ordering::SeqCst),
        before
    );
    let cell = harness.console.sessions().find(&sid).unwrap();
    assert_eq!(cell.meta().subject, creator.subject);
    harness.close().await;
}

/// The console adds no alternate identity path: the service and owner-key
/// composition hooks still use the reviewed shared authentication entrypoints.
#[test]
fn creator_authentication_keeps_the_reviewed_surface() {
    fn public_functions(source: &str) -> Vec<&str> {
        source
            .lines()
            .map(str::trim_start)
            .filter_map(|line| {
                line.strip_prefix("pub fn ")
                    .or_else(|| line.strip_prefix("pub async fn "))
                    .or_else(|| line.strip_prefix("pub(crate) fn "))
                    .or_else(|| line.strip_prefix("pub(crate) async fn "))
            })
            .map(|rest| rest.split(['(', '<']).next().unwrap())
            .collect()
    }

    assert_eq!(
        public_functions(include_str!("../../src/auth/creator.rs")),
        [
            "new",
            "with_service",
            "with_owner_keys",
            "authenticate_token"
        ]
    );
    assert_eq!(
        public_functions(include_str!("../../src/auth/creator_request.rs")),
        ["authenticate"]
    );
}

#[tokio::test]
async fn a_state_mismatch_is_refused_without_an_exchange() {
    let harness = Harness::enabled().await;
    let (authorize, login_cookie) = harness.start_login("/console").await;
    let callback = harness.approve(&authorize).await;
    let mut forged = reqwest::Url::parse(&callback).unwrap();
    let code = query(&callback)["code"].clone();
    forged
        .query_pairs_mut()
        .clear()
        .append_pair("code", &code)
        .append_pair("state", "forged-state");
    let response = harness.callback(forged.as_str(), Some(&login_cookie)).await;
    assert_eq!(response.status(), 400);
    let page = response.text().await.unwrap();
    assert!(page.contains("Sign-in expired; please try again."));
    assert!(harness.nyxid.oauth.token_calls.lock().unwrap().is_empty());
    harness.close().await;
}

#[tokio::test]
async fn a_callback_without_a_valid_login_cookie_makes_no_nyxid_call() {
    let harness = Harness::enabled().await;
    let (authorize, _) = harness.start_login("/console").await;
    let callback = harness.approve(&authorize).await;
    let state = query(&callback)["state"].clone();
    let expired = seal_console_login(
        &harness.keys,
        &ConsoleLogin {
            state,
            verifier: "v".repeat(86),
            return_to: "/console".into(),
            exp: 1_000,
        },
    )
    .unwrap();
    for cookie in [None, Some("garbage"), Some(expired.as_str())] {
        let response = harness.callback(&callback, cookie).await;
        assert_eq!(response.status(), 400, "{cookie:?}");
        assert!(response.headers().get("location").is_none());
        assert!(response
            .text()
            .await
            .unwrap()
            .contains("Sign-in expired; please try again."));
    }
    assert!(harness.nyxid.oauth.token_calls.lock().unwrap().is_empty());
    assert_eq!(harness.nyxid.oauth.users_me_calls.load(Ordering::SeqCst), 0);
    harness.close().await;
}

#[tokio::test]
async fn unsafe_return_paths_fall_back_to_the_console_home() {
    let harness = Harness::enabled().await;
    let long = format!("/console/{}", "a".repeat(600));
    for return_to in [
        long.as_str(),
        "//evil.example/console",
        "https://evil.example/console",
        "/console//evil.example",
        "/console\\evil",
        "/elsewhere",
    ] {
        let (_, location) = harness.sign_in("alice", return_to).await;
        assert_eq!(location, "/console", "{return_to}");
    }
    harness.close().await;
}

#[tokio::test]
async fn nyxid_refusals_and_outages_render_pages_and_create_no_session() {
    let harness = Harness::enabled().await;
    // The user cancelled at NyxID.
    let (authorize, login_cookie) = harness.start_login("/console").await;
    let state = query(&authorize)["state"].clone();
    let cancelled = format!(
        "{}/console/callback?error=access_denied&state={state}",
        harness.base
    );
    let page = harness.callback(&cancelled, Some(&login_cookie)).await;
    assert_eq!(page.status(), 400);
    assert!(page.text().await.unwrap().contains("cancelled or refused"));
    // A refused exchange, then an unavailable token endpoint, then an
    // unavailable identity.
    let oauth = &harness.nyxid.oauth;
    for (status, expected) in [(400, 400), (503, 503)] {
        oauth.token_status.store(status, Ordering::SeqCst);
        let (authorize, login_cookie) = harness.start_login("/console").await;
        let callback = harness.approve(&authorize).await;
        let response = harness.callback(&callback, Some(&login_cookie)).await;
        assert_eq!(response.status(), expected);
    }
    oauth.token_status.store(200, Ordering::SeqCst);
    oauth.down.store(true, Ordering::SeqCst);
    let (authorize, login_cookie) = harness.start_login("/console").await;
    let callback = harness.approve(&authorize).await;
    let response = harness.callback(&callback, Some(&login_cookie)).await;
    assert_eq!(response.status(), 503);
    assert!(harness.console.sessions().is_empty());
    // The tokens of the failed identity check are revoked.
    for _ in 0..100 {
        if !oauth.revoked.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(oauth.revoked.lock().unwrap().len(), 1);
    harness.close().await;
}

#[tokio::test]
async fn the_login_limiter_renders_the_429_sign_in_page_for_login_and_callback() {
    let limited = ConsoleConfig {
        login_rate_per_minute: 2,
        ..config()
    };
    let harness = Harness::start(limited).await;
    harness.console.discover_once().await.unwrap();
    let (authorize, login_cookie) = harness.start_login("/console").await;
    let callback = harness.approve(&authorize).await;
    // The second step of the minute is the callback; the third is refused.
    let admitted = harness.callback(&callback, Some(&login_cookie)).await;
    assert_eq!(admitted.status(), 303);
    for response in [
        harness.get("/console/login?return_to=%2Fconsole").await,
        harness.callback(&callback, Some(&login_cookie)).await,
    ] {
        assert_eq!(response.status(), 429);
        assert!(response.headers().get("location").is_none());
        assert!(response
            .text()
            .await
            .unwrap()
            .contains("Too many sign-in attempts; try again in a minute."));
    }
    assert_eq!(
        sample(
            &metrics(),
            "cmaeg_console_logins_total{outcome=\"rate_limited\"}"
        ),
        2.0
    );
    harness.close().await;
}

#[tokio::test]
async fn at_most_four_code_exchanges_run_at_once() {
    let harness = Harness::enabled().await;
    harness
        .nyxid
        .oauth
        .token_delay_ms
        .store(300, Ordering::SeqCst);
    let mut callbacks = Vec::new();
    for _ in 0..6 {
        let (authorize, cookie) = harness.start_login("/console").await;
        callbacks.push((harness.approve(&authorize).await, cookie));
    }
    let responses = futures_util::future::join_all(
        callbacks
            .iter()
            .map(|(url, cookie)| harness.callback(url, Some(cookie))),
    )
    .await;
    assert!(responses.iter().all(|response| response.status() == 303));
    assert_eq!(harness.nyxid.oauth.token_peak.load(Ordering::SeqCst), 4);
    assert_eq!(harness.console.sessions().len(), 5);
    harness.close().await;
}

#[tokio::test]
async fn token_endpoint_requires_form_credentials_without_a_basic_header() {
    use crate::fake_nyxid::console::{CLIENT_ID, CLIENT_SECRET};
    let harness = Harness::enabled().await;
    let endpoint = format!("{}/oauth/token", harness.nyxid.base_url);
    let client = reqwest::Client::new();
    for (id, secret, basic) in [
        (None, None, true),
        (Some(CLIENT_ID), None, false),
        (Some(CLIENT_ID), Some("wrong-client-secret"), false),
        (Some(CLIENT_ID), Some(CLIENT_SECRET), true),
    ] {
        let mut form = vec![("grant_type", "authorization_code"), ("code", "unused")];
        if let Some(id) = id {
            form.push(("client_id", id));
        }
        if let Some(secret) = secret {
            form.push(("client_secret", secret));
        }
        let mut request = client.post(&endpoint).form(&form);
        if basic {
            request = request.basic_auth(CLIENT_ID, Some(CLIENT_SECRET));
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), 401);
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["error"], "invalid_client");
    }
    // A correct client still signs in after rejected authentication attempts.
    let (sid, _) = harness.sign_in("alice", "/console").await;
    assert!(harness.console.sessions().find(&sid).is_some());
    harness.close().await;
}
