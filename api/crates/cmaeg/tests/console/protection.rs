//! Request hardening: security headers on every console response, Origin
//! and CSRF checks on every POST, same-origin redirects, and a logout that
//! really logs out.
use super::support::{config, csrf_for, metrics, sample, Harness};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use sha2::{Digest, Sha256};
use std::sync::atomic::Ordering;

const POST_ROUTES: [&str; 1] = ["/console/logout"];
const CSS: &str = include_str!("../../src/console/console.css");

fn assert_secure(headers: &reqwest::header::HeaderMap, page: &str) {
    let policy = headers["content-security-policy"].to_str().unwrap();
    if let Some(style) = page
        .split("<style>")
        .nth(1)
        .and_then(|rest| rest.split("</style>").next())
    {
        assert_eq!(style, CSS);
    }
    let hash = STANDARD.encode(Sha256::digest(CSS.as_bytes()));
    assert_eq!(
        policy,
        format!("default-src 'none'; style-src 'sha256-{hash}'; img-src 'none'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'")
    );
    assert_eq!(headers["x-frame-options"], "DENY");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert_eq!(headers["referrer-policy"], "same-origin");
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(
        headers["permissions-policy"],
        "camera=(), microphone=(), geolocation=(), payment=(), usb=()"
    );
    if !page.is_empty() {
        assert!(page.contains("<meta name=\"robots\" content=\"noindex\">"));
        assert!(!page.to_ascii_lowercase().contains("<script"));
        assert!(!page.contains("http-equiv=\"refresh\""));
    }
}

async fn checked(response: reqwest::Response) -> (u16, String) {
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let page = response.text().await.unwrap();
    assert_secure(&headers, &page);
    (status, page)
}

#[tokio::test]
async fn every_console_response_carries_the_security_headers() {
    let disabled = Harness::start(config()).await;
    let (status, _) = checked(disabled.get("/console").await).await;
    assert_eq!(status, 503);
    disabled.close().await;
    let harness = Harness::enabled().await;
    let sid = harness.session("alice");
    for path in ["/console", "/console/signed-out", "/console/no/such/page"] {
        checked(harness.get(path).await).await;
        checked(harness.get_as(path, &sid).await).await;
    }
    let (_, page) = checked(harness.get_as("/console", &sid).await).await;
    let csrf = csrf_for(&page, "/console/logout");
    checked(
        harness
            .post("/console/logout", Some(&sid), None, &[("csrf", &csrf)])
            .await,
    )
    .await;
    checked(
        harness
            .post("/console/logout", None, Some("https://evil.example"), &[])
            .await,
    )
    .await;
    harness.close().await;
}

#[tokio::test]
async fn a_get_without_a_session_renders_the_sign_in_page_and_never_leaves() {
    let harness = Harness::enabled().await;
    for (path, link) in [
        ("/console", "/console/login?return_to=%2Fconsole"),
        (
            "/console?page=2",
            "/console/login?return_to=%2Fconsole%3Fpage%3D2",
        ),
    ] {
        let response = harness.get(path).await;
        assert_eq!(response.status(), 200);
        assert!(response.headers().get("location").is_none());
        let page = response.text().await.unwrap();
        assert!(
            page.contains(&format!("href=\"{}\"", link.replace('&', "&amp;"))),
            "{page}"
        );
        assert!(page.contains("Sign in with NyxID"));
    }
    assert!(harness.nyxid.oauth.token_calls.lock().unwrap().is_empty());
    harness.close().await;
}

#[tokio::test]
async fn origin_and_csrf_rejections_are_counted_and_refused() {
    let harness = Harness::enabled().await;
    let sid = harness.session("alice");
    let page = harness.get_as("/console", &sid).await.text().await.unwrap();
    let csrf = csrf_for(&page, "/console/logout");
    for origin in [
        "https://evil.example",
        "null",
        &format!("{}/", harness.base),
    ] {
        let response = harness
            .post(
                "/console/logout",
                Some(&sid),
                Some(origin),
                &[("csrf", &csrf)],
            )
            .await;
        assert_eq!(response.status(), 403, "{origin}");
    }
    for fields in [
        vec![],
        vec![("csrf", "forged")],
        vec![("csrf", csrf.as_str()), ("csrf", csrf.as_str())],
    ] {
        let response = harness
            .post("/console/logout", Some(&sid), None, &fields)
            .await;
        assert_eq!(response.status(), 403, "{fields:?}");
    }
    // A token minted for another session is refused too.
    let other = harness.session("bob");
    let response = harness
        .post("/console/logout", Some(&other), None, &[("csrf", &csrf)])
        .await;
    assert_eq!(response.status(), 403);
    assert_eq!(harness.console.sessions().len(), 2);
    let rendered = metrics();
    assert_eq!(
        sample(
            &rendered,
            "cmaeg_console_rejections_total{reason=\"origin\"}"
        ),
        3.0
    );
    assert_eq!(
        sample(&rendered, "cmaeg_console_rejections_total{reason=\"csrf\"}"),
        4.0
    );
    harness.close().await;
}

#[tokio::test]
async fn no_post_redirect_leaves_the_public_origin() {
    let harness = Harness::enabled().await;
    for path in POST_ROUTES {
        let sid = harness.session("alice");
        let page = harness.get_as("/console", &sid).await.text().await.unwrap();
        let csrf = csrf_for(&page, "/console/logout");
        for response in [
            harness.post(path, None, None, &[]).await,
            harness.post(path, Some("expired-session"), None, &[]).await,
            harness
                .post(path, Some(&sid), None, &[("csrf", &csrf)])
                .await,
        ] {
            assert_eq!(response.status(), 303, "{path}");
            let location = response.headers()["location"].to_str().unwrap();
            assert!(location.starts_with("/console"), "{path} -> {location}");
            assert!(!location.starts_with("//"));
            let followed = harness.get(location).await;
            assert!(followed.headers().get("location").is_none());
        }
    }
    harness.close().await;
}

#[tokio::test]
async fn logout_revokes_and_a_later_get_creates_no_session_without_a_click() {
    let harness = Harness::enabled().await;
    let tokens = harness.tokens("alice");
    let refresh = tokens.refresh.as_str().to_owned();
    let (sid, _) = harness
        .console
        .sessions()
        .create("alice".into(), tokens)
        .unwrap();
    let sid = sid.as_str().to_owned();
    let page = harness.get_as("/console", &sid).await.text().await.unwrap();
    let csrf = csrf_for(&page, "/console/logout");
    let response = harness
        .post("/console/logout", Some(&sid), None, &[("csrf", &csrf)])
        .await;
    assert_eq!(response.status(), 303);
    assert_eq!(response.headers()["location"], "/console/signed-out");
    let cleared = response.headers()["set-cookie"].to_str().unwrap();
    assert!(cleared.starts_with("__Host-cmaeg-console=;"));
    assert!(cleared.contains("Max-Age=0"));
    for _ in 0..100 {
        if !harness.nyxid.oauth.revoked.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(*harness.nyxid.oauth.revoked.lock().unwrap(), [refresh]);
    let signed_out = harness
        .get("/console/signed-out")
        .await
        .text()
        .await
        .unwrap();
    assert!(signed_out.contains("Your NyxID session itself is unchanged"));
    assert!(signed_out.contains("href=\"/console/login?return_to=%2Fconsole\""));
    // A reload with the old cookie stays signed out and calls nobody.
    let calls = harness.nyxid.oauth.token_calls.lock().unwrap().len();
    let again = harness.get_as("/console", &sid).await;
    assert_eq!(again.status(), 200);
    assert!(again.headers().get("location").is_none());
    assert!(again.text().await.unwrap().contains("Sign in with NyxID"));
    assert!(harness.console.sessions().is_empty());
    assert_eq!(harness.nyxid.oauth.token_calls.lock().unwrap().len(), calls);
    assert_eq!(harness.nyxid.oauth.users_me_calls.load(Ordering::SeqCst), 0);
    let expired = harness
        .get("/console/signed-out?reason=expired")
        .await
        .text()
        .await
        .unwrap();
    assert!(expired.contains("Your console session has ended"));
    harness.close().await;
}
