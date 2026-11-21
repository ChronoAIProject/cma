//! OAuth discovery may move without changing REST's retained identity authority.
use super::{
    fake_nyxid::FakeNyxid,
    support::{config, Harness},
};
use cmaeg::{console::DiscoveryError, ConsoleConfig};
use serde_json::json;
use std::sync::atomic::Ordering;

#[tokio::test]
async fn canonical_oauth_origin_keeps_retained_authority_and_rejects_foreign_endpoints() {
    let public = FakeNyxid::start().await;
    let harness = Harness::start(ConsoleConfig {
        nyxid_public_url: Some(public.base_url.clone()),
        ..config()
    })
    .await;
    let document = json!({
        "issuer": harness.nyxid.base_url,
        "authorization_endpoint": format!("{}/oauth/authorize", public.base_url),
        "token_endpoint": format!("{}/oauth/token", public.base_url),
        "revocation_endpoint": format!("{}/oauth/revoke", public.base_url),
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["client_secret_post"]
    });
    *public.oauth.discovery.lock().unwrap() = Some(document.clone());
    // The legacy origin cannot supply discovery; only the explicit public one can.
    harness
        .nyxid
        .oauth
        .discovery_status
        .store(503, Ordering::SeqCst);
    harness.console.discover_once().await.unwrap();
    assert_eq!(harness.get("/console").await.status(), 200);
    let (authorize, _) = harness.start_login("/console").await;
    assert!(authorize.starts_with(&format!("{}/oauth/authorize?", public.base_url)));
    // Existing REST credentials still authenticate against the original authority.
    let sid = harness.session("alice");
    let page = harness.get_as("/console", &sid).await;
    assert_eq!(page.status(), 200);
    assert!(page.text().await.unwrap().contains("…lice"));
    assert!(harness.nyxid.oauth.users_me_calls.load(Ordering::SeqCst) > 0);
    assert_eq!(public.oauth.users_me_calls.load(Ordering::SeqCst), 0);
    // Moving OAuth does not relax the same-origin check for any sensitive endpoint.
    for field in [
        "authorization_endpoint",
        "token_endpoint",
        "revocation_endpoint",
    ] {
        let mut foreign = document.clone();
        foreign[field] = json!(format!("{}/oauth/untrusted", harness.nyxid.base_url));
        *public.oauth.discovery.lock().unwrap() = Some(foreign);
        assert_eq!(
            harness.console.discover_once().await,
            Err(DiscoveryError::Mismatch(field))
        );
        assert_eq!(harness.get("/console/login").await.status(), 503);
    }
    harness.close().await;
    public.close().await;
}
