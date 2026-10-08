//! No secret leaves the process: client secret, tokens, codes, verifiers,
//! states and session ids never appear in a console response body, a
//! gateway `Location` or a log line.
use super::support::{csrf_for, Harness};
use std::{
    io::Write,
    sync::{Arc, Mutex},
    time::Duration,
};
use tracing_subscriber::{fmt::MakeWriter, layer::SubscriberExt, Layer};

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Captured {
    type Writer = Captured;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// What a browser sees from the gateway: bodies and same-origin locations.
#[derive(Default)]
struct Seen(Vec<String>);

impl Seen {
    async fn record(&mut self, response: reqwest::Response) -> String {
        if let Some(location) = response.headers().get("location") {
            self.0.push(location.to_str().unwrap().to_owned());
        }
        let body = response.text().await.unwrap();
        self.0.push(body.clone());
        body
    }
}

#[tokio::test]
async fn no_secret_reaches_a_page_a_location_or_a_log() {
    let logs = Captured::default();
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(logs.clone())
            .with_filter(cmaeg::log_layer_filter(Some("trace"))),
    );
    let _guard = tracing::subscriber::set_default(subscriber);
    let harness = Harness::enabled().await;
    let oauth = &harness.nyxid.oauth;
    oauth.grant("org-a", "Org A", "alice", "admin");
    let mut seen = Seen::default();
    let mut secrets = vec![super::fake_nyxid::console::CLIENT_SECRET.to_owned()];

    seen.record(harness.get("/console").await).await;
    // The authorization URL must carry state: that Location is NyxID's
    // protocol, so it alone is not scanned.
    let (authorize, login_cookie) = harness.start_login("/console").await;
    let state = reqwest::Url::parse(&authorize)
        .unwrap()
        .query_pairs()
        .find(|(name, _)| name == "state")
        .unwrap()
        .1
        .into_owned();
    let callback = harness.approve(&authorize).await;
    let code = reqwest::Url::parse(&callback)
        .unwrap()
        .query_pairs()
        .find(|(name, _)| name == "code")
        .unwrap()
        .1
        .into_owned();
    let response = harness.callback(&callback, Some(&login_cookie)).await;
    let sid = super::support::set_cookie(&response, "__Host-cmaeg-console").unwrap();
    seen.record(response).await;
    let page = seen.record(harness.get_as("/console", &sid).await).await;
    let owner_csrf = csrf_for(&page, "/console/owner");
    seen.record(
        harness
            .post(
                "/console/owner",
                Some(&sid),
                None,
                &[("csrf", &owner_csrf), ("owner", "org-a")],
            )
            .await,
    )
    .await;
    // Within 30 s of the access token's expiry, so the page refreshes it.
    harness.clock.advance(Duration::from_secs(871));
    let page = seen.record(harness.get_as("/console", &sid).await).await;
    seen.record(
        harness
            .post(
                "/console/owner",
                Some(&sid),
                Some("https://evil.example"),
                &[],
            )
            .await,
    )
    .await;
    seen.record(
        harness
            .post("/console/owner", Some(&sid), None, &[("csrf", "forged")])
            .await,
    )
    .await;
    let logout_csrf = csrf_for(&page, "/console/logout");
    seen.record(
        harness
            .post(
                "/console/logout",
                Some(&sid),
                None,
                &[("csrf", &logout_csrf)],
            )
            .await,
    )
    .await;
    seen.record(harness.get_as("/console", &sid).await).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    secrets.extend([state, code, sid, login_cookie]);
    secrets.extend(oauth.issued_tokens());
    secrets.extend(oauth.revoked.lock().unwrap().iter().cloned());
    for call in oauth.token_calls.lock().unwrap().iter() {
        secrets.extend(call.code_verifier.clone());
        secrets.extend(call.refresh_token.clone());
    }
    assert!(secrets.len() >= 8);
    let logs = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("console login"), "the log capture is live");
    for secret in &secrets {
        for text in &seen.0 {
            assert!(
                !text.contains(secret.as_str()),
                "a response carries a secret"
            );
        }
        assert!(
            !logs.contains(secret.as_str()),
            "a log line carries a secret"
        );
    }
    harness.close().await;
}
