//! Exercise authentication through the public HTTP router and retained channel restore.
use crate::{
    activity::{ActivityStore, ObservedActivity},
    channels::resolver::ChannelResolver,
    ingress::{launch::UnavailableLauncher, routing_plan::fixture::keys, IngressState},
    interactions::native_lark::fixture::{now, Fixture},
    native_lark::test_support as native,
    turns::observer::NoopObserver,
};
use cmaeg_core::channel::{LarkRegion, Secret};
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};
use std::{
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};

struct HttpFixture {
    native: Fixture,
    url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for HttpFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl HttpFixture {
    async fn new() -> Self {
        let native = Fixture::new(false, false, LarkRegion::Feishu).await;
        let records = native.server.records.clone();
        let keys = Arc::new(keys());
        let state = IngressState::new(
            ChannelResolver::new(Arc::new(records.clone()), keys.clone(), Default::default()),
            keys.clone(),
            Default::default(),
            Arc::new(UnavailableLauncher),
        )
        .unwrap()
        .with_native_lark_intake(true)
        .with_activity_observer(Arc::new(ObservedActivity::new(
            Arc::new(ActivityStore::new(records, keys)),
            Arc::new(NoopObserver),
        )));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/v1/native-lark/{}",
            listener.local_addr().unwrap(),
            native.channel.id
        );
        let task = tokio::spawn(async move {
            axum::serve(listener, super::router(state)).await.unwrap();
        });
        Self { native, url, task }
    }
    async fn post(&self, body: Vec<u8>, headers: &[(&str, &str)]) -> reqwest::Response {
        self.post_url(&self.url, body, headers).await
    }
    async fn post_url(
        &self,
        url: &str,
        body: Vec<u8>,
        headers: &[(&str, &str)],
    ) -> reqwest::Response {
        let memory = &self.native.server.memory;
        let writes = memory.writes.load(Ordering::SeqCst);
        let rows = memory.rows.lock().unwrap().len();
        let mut request = Client::new()
            .post(url)
            .header("content-type", "application/json")
            .body(body);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let start = Instant::now();
        let response = request
            .timeout(Duration::from_secs(3))
            .send()
            .await
            .unwrap();
        assert!(start.elapsed() < Duration::from_millis(2500));
        assert_eq!(
            memory.writes.load(Ordering::SeqCst),
            writes,
            "no admission or reply target writes"
        );
        assert_eq!(memory.rows.lock().unwrap().len(), rows);
        response
    }
}

#[tokio::test]
async fn unsigned_event_with_an_invalid_channel_is_authentication_failure() {
    let fixture = HttpFixture::new().await;
    let (body, _) = native::signed(&native::text("om_no_authority", "hello", now()), now());
    let endpoint = fixture
        .url
        .rsplit_once('/')
        .map(|(prefix, _)| format!("{prefix}/not-a-channel"))
        .unwrap();
    let response = fixture.post_url(&endpoint, body, &[]).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
fn challenge() -> Value {
    json!({"type":"url_verification", "token":native::credentials().verification_token.as_str(), "challenge":"exact-挑战-value"})
}
const BAD_HEADERS: &[(&str, &str)] = &[
    ("x-lark-request-timestamp", "not-a-timestamp"),
    ("x-lark-request-nonce", "invalid nonce"),
    ("x-lark-signature", "different-signature"),
];

#[tokio::test]
async fn native_lark_challenge_ignores_absent_invalid_and_duplicate_signature_headers() {
    let fixture = HttpFixture::new().await;
    let (body, _) = native::signed(&challenge(), now());
    for headers in [
        vec![],
        BAD_HEADERS.to_vec(),
        vec![("x-lark-signature", "one"), ("x-lark-signature", "two")],
    ] {
        let response = fixture.post(body.clone(), &headers).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.json::<Value>().await.unwrap(),
            json!({"challenge":"exact-挑战-value"})
        );
    }
}

#[tokio::test]
async fn native_lark_challenge_rejects_wrong_token_plaintext_unbounded_and_undecryptable_bodies() {
    let fixture = HttpFixture::new().await;
    let mut wrong = challenge();
    wrong["token"] = "wrong-verification-fixture".into();
    let mut too_long = challenge();
    too_long["challenge"] = "x".repeat(4097).into();
    let mut empty = challenge();
    empty["challenge"] = "".into();
    let mut control = challenge();
    control["challenge"] = "line\nbreak".into();
    for body in [
        native::signed(&wrong, now()).0,
        native::signed(&too_long, now()).0,
        native::signed(&empty, now()).0,
        native::signed(&control, now()).0,
        serde_json::to_vec(&challenge()).unwrap(),
        br#"{"encrypt":"undecryptable"}"#.to_vec(),
    ] {
        assert_eq!(
            fixture.post(body, &[]).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        fixture.post(vec![b' '; 256 * 1024 + 1], &[]).await.status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

#[tokio::test]
async fn native_lark_unsigned_or_badly_signed_nonchallenge_events_never_admit() {
    let fixture = HttpFixture::new().await;
    let sent = now();
    let event = native::text("om_no_authority", "hello", sent);
    let (body, signature) = native::signed(&event, sent);
    let timestamp = sent.to_string();
    for headers in [
        vec![],
        BAD_HEADERS.to_vec(),
        vec![
            ("x-lark-request-timestamp", timestamp.as_str()),
            ("x-lark-request-nonce", "fixture_nonce"),
            (
                "x-lark-signature",
                "0000000000000000000000000000000000000000000000000000000000000000",
            ),
        ],
        vec![("x-lark-signature", signature.as_str())],
        vec![
            ("x-lark-request-timestamp", timestamp.as_str()),
            ("x-lark-request-nonce", "fixture_nonce"),
            ("x-lark-signature", signature.as_str()),
            ("x-lark-signature", signature.as_str()),
        ],
    ] {
        assert_eq!(
            fixture.post(body.clone(), &headers).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }
    let stale = (sent - 301).to_string();
    let (body, signature) = native::signed(&event, sent - 301);
    assert_eq!(
        fixture
            .post(
                body,
                &[
                    ("x-lark-request-timestamp", &stale),
                    ("x-lark-request-nonce", "fixture_nonce"),
                    ("x-lark-signature", &signature)
                ]
            )
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn native_lark_challenge_restores_current_credentials_and_source() {
    for change in [
        "encrypt_key",
        "verification_token",
        "source",
        "other_source",
        "missing",
    ] {
        let mut fixture = HttpFixture::new().await;
        let (body, _) = native::signed(&challenge(), now());
        // Warm the restore path before rotation/removal.
        assert_eq!(
            fixture.post(body.clone(), &[]).await.status(),
            StatusCode::OK
        );
        let definition = &mut Arc::make_mut(&mut fixture.native.channel).definition;
        match change {
            "encrypt_key" => {
                definition.secrets.native_lark.as_mut().unwrap().encrypt_key =
                    Secret::new("rotated-encryption-fixture")
            }
            "verification_token" => {
                definition
                    .secrets
                    .native_lark
                    .as_mut()
                    .unwrap()
                    .verification_token = Secret::new("rotated-verification-fixture")
            }
            "source" => {
                definition.sources = vec![];
                definition.secrets.native_lark = None;
            }
            "other_source" => {
                definition.sources =
                    vec![
                        serde_json::from_value(crate::channels::routing_fixture::telegram())
                            .unwrap(),
                    ];
                definition.secrets.native_lark = None;
                definition.validate().unwrap();
            }
            _ => {}
        }
        fixture.native.save_channel();
        if change == "missing" {
            fixture
                .native
                .server
                .memory
                .channels
                .lock()
                .unwrap()
                .clear();
        }
        assert!(
            !fixture.post(body, &[]).await.status().is_success(),
            "{change}"
        );
    }
}

#[tokio::test]
async fn permanent_source_failures_do_not_masquerade_as_dependency_outages() {
    for failure in ["malformed", "tampered", "missing", "invalid_definition"] {
        let fixture = HttpFixture::new().await;
        let sent = now();
        let (body, signature) =
            native::signed(&native::text("om_no_authority", "hello", sent), sent);
        let timestamp = sent.to_string();
        let mut endpoint = fixture.url.clone();
        match failure {
            "malformed" => {
                endpoint = format!("{}/bad-channel", endpoint.rsplit_once('/').unwrap().0)
            }
            "tampered" => {
                let mut pointer = fixture.native.channel.pointer.clone();
                pointer.record_url = "http://invalid.example/changed-record".into();
                let id = pointer.seal_id(&keys()).unwrap();
                endpoint = format!("{}/{}", endpoint.rsplit_once('/').unwrap().0, id);
            }
            "missing" => fixture
                .native
                .server
                .memory
                .channels
                .lock()
                .unwrap()
                .clear(),
            "invalid_definition" => {
                for row in fixture
                    .native
                    .server
                    .memory
                    .channels
                    .lock()
                    .unwrap()
                    .values_mut()
                {
                    row.blob = b"invalid-fixture".to_vec();
                }
            }
            _ => unreachable!(),
        }
        for headers in [
            vec![],
            BAD_HEADERS.to_vec(),
            vec![("x-lark-signature", "one"), ("x-lark-signature", "two")],
        ] {
            let response = fixture.post_url(&endpoint, body.clone(), &headers).await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{failure}");
        }
        let response = fixture
            .post_url(
                &endpoint,
                body,
                &[
                    ("x-lark-request-timestamp", &timestamp),
                    ("x-lark-request-nonce", "fixture_nonce"),
                    ("x-lark-signature", &signature),
                ],
            )
            .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{failure}");
        assert_eq!(
            response.json::<Value>().await.unwrap()["code"],
            "channel_not_found"
        );
    }
}

#[tokio::test]
async fn source_timeout_remains_retryable_without_admitting_unsigned_work() {
    let fixture = HttpFixture::new().await;
    fixture
        .native
        .server
        .memory
        .delay_ms
        .store(1_500, Ordering::SeqCst);
    let (body, _) = native::signed(&challenge(), now());
    let response = fixture.post(body, &[]).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response.json::<Value>().await.unwrap()["detail"],
        "native_lark_source_unavailable"
    );
}

#[tokio::test]
async fn signed_event_reaches_admission_after_authentication() {
    let fixture = HttpFixture::new().await;
    let sent = now();
    let (body, signature) = native::signed(&native::text("om_signed", "hello", sent), sent);
    let response = fixture
        .post(
            body,
            &[
                ("x-lark-request-timestamp", &sent.to_string()),
                ("x-lark-request-nonce", "fixture_nonce"),
                ("x-lark-signature", &signature),
            ],
        )
        .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response.json::<Value>().await.unwrap()["detail"],
        "native_lark_admission_unavailable"
    );
}

#[test]
fn resolver_statuses_and_failure_stage_are_closed() {
    use super::{resolver_error, FailureStage, ResolveError};
    let headers = axum::http::HeaderMap::new();
    for (error, status) in [
        (ResolveError::Invalid, 401),
        (ResolveError::Tampered, 401),
        (ResolveError::NotFound, 401),
        (ResolveError::InvalidDefinition, 401),
        (ResolveError::CreationPending, 409),
        (ResolveError::RateLimited, 429),
        (ResolveError::Unavailable, 503),
    ] {
        let response = resolver_error(error, &headers);
        assert_eq!(response.status().as_u16(), status);
        assert_eq!(
            response.extensions().get::<FailureStage>(),
            Some(&FailureStage::NativeLarkSource)
        );
    }
}
