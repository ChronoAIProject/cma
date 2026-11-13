#[path = "support/database.rs"]
pub mod database;
#[path = "fake_nyxid.rs"]
pub mod fake_nyxid;

use std::{path::Path, sync::Arc};

use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine as _};
use cmaeg::{
    auth::CreatorAuthenticator,
    github,
    providers::{ProviderClient, ProviderRegistry},
    records::{mongo::MongoRecords, RecordClient},
    ManagementConfig,
};
use cmaeg_core::{channel::Secret, seal::MasterKeyRing};
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};
use tokio::sync::oneshot;

struct Fixture {
    database: database::DatabaseFixture,
    nyxid: fake_nyxid::FakeNyxid,
    client: Client,
    base: String,
    app_ref: String,
    shutdown: Option<oneshot::Sender<()>>,
    clock: Arc<std::sync::atomic::AtomicI64>,
}

impl Fixture {
    async fn new() -> Self {
        let database = database::DatabaseFixture::new(Path::new(env!("CARGO_BIN_EXE_cmaeg"))).await;
        let admin = database.admin.database(&database.name);
        github::migrate(&admin).await.unwrap();
        admin
            .run_command(github::runtime_role(&database.name))
            .await
            .unwrap();
        admin.run_command(mongodb::bson::doc! {"grantRolesToUser":"runtime","roles":[{"role":"cma_eg_automation_runtime","db":&database.name}]}).await.unwrap();
        let nyxid = fake_nyxid::FakeNyxid::start().await;
        let mut fixture = Self {
            database,
            nyxid,
            client: Client::new(),
            base: String::new(),
            app_ref: String::new(),
            shutdown: None,
            clock: Arc::new(std::sync::atomic::AtomicI64::new(
                time::OffsetDateTime::now_utc().unix_timestamp(),
            )),
        };
        fixture.start().await;
        let response = fixture
            .client
            .post(format!("{}/v1/github/apps", fixture.base))
            .bearer_auth("token-alice")
            .header("Idempotency-Key", "legacy-routing-fixture")
            .json(&json!({"app_id":42,"display_name":"Routing fixture","webhook_secret":"fixture-webhook-secret"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        fixture.app_ref = response.json::<Value>().await.unwrap()["app_ref"]
            .as_str()
            .unwrap()
            .to_owned();
        fixture.shutdown.take().unwrap().send(()).unwrap();
        fixture.start().await;
        fixture
    }

    async fn start(&mut self) {
        let database = mongodb::Client::with_uri_str(&self.database.uri)
            .await
            .unwrap()
            .database(&self.database.name);
        let keys = Arc::new(
            MasterKeyRing::parse(&format!("1:{}", STANDARD_NO_PAD.encode([3_u8; 32]))).unwrap(),
        );
        let records = Arc::new(
            RecordClient::new("http://127.0.0.1:1", "unused-service")
                .unwrap()
                .with_database(MongoRecords::open(database.clone()).await.unwrap()),
        );
        let providers = Arc::new(
            ProviderClient::new(
                ProviderRegistry::parse("[]").unwrap(),
                std::time::Duration::from_secs(1),
                std::time::Duration::from_secs(1),
                1,
            )
            .unwrap(),
        );
        let clock = self.clock.clone();
        let gateway = github::Gateway::open(database, keys.clone())
            .await
            .unwrap()
            .with_receipt_clock(Arc::new(move || {
                clock.load(std::sync::atomic::Ordering::SeqCst)
            }));
        let state = cmaeg::http::ManagementState::new(
            providers,
            records,
            Arc::new(
                CreatorAuthenticator::new(&self.nyxid.base_url)
                    .unwrap()
                    .with_service(
                        cma_service_auth::NyxIdAuth::new(
                            &self.nyxid.base_url,
                            cma_service_auth::Service::EventGateway,
                            "gateway-management",
                            "gateway-management-fixture".into(),
                        )
                        .unwrap(),
                    ),
            ),
            ManagementConfig {
                master_keys: keys,
                public_url: "https://eg.example".into(),
                console_public_url: None,
                public_api_url: "https://eg.example".into(),
                public_api_aliases: Vec::new(),
                advertise_public_api_url: true,
                cma_base_url: "http://127.0.0.1:1".into(),
                tools_nyxid_slug: "cmaeg".into(),
                cma_service_token: Secret::new("unused-service"),
                nyxid_base_url: self.nyxid.base_url.clone(),
            },
            cmaeg::ChannelCacheConfig::default(),
        );
        assert!(state.github.set(Arc::new(gateway)).is_ok());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        self.base = format!("http://{}", listener.local_addr().unwrap());
        let (shutdown, signal) = oneshot::channel();
        self.shutdown = Some(shutdown);
        tokio::spawn(async move {
            axum::serve(listener, cmaeg::router_with_state(state))
                .with_graceful_shutdown(async {
                    let _ = signal.await;
                })
                .await
                .unwrap();
        });
    }

    async fn bind(&self) -> Value {
        let verification = self
            .client
            .post(format!(
                "{}/v1/github/apps/{}/verification",
                self.base, self.app_ref
            ))
            .bearer_auth("token-alice")
            .header("Idempotency-Key", "verify-app")
            .json(&json!({"expected_version":1,"evidence_ref":"review:app"}))
            .send()
            .await
            .unwrap();
        assert_eq!(verification.status(), 200);
        let response = self.client.post(format!("{}/v1/github/installation-bindings", self.base))
            .bearer_auth("token-alice").header("Idempotency-Key", "binding")
            .json(&json!({"app_ref":self.app_ref,"installation_id":99,"account_id":123,"repository_ids":[1001,1002]}))
            .send().await.unwrap();
        assert_eq!(response.status(), 201);
        let binding: Value = response.json().await.unwrap();
        let response = self.client.post(format!("{}/v1/github/installation-bindings/{}/verification", self.base, binding["binding_id"].as_str().unwrap()))
            .bearer_auth("token-alice").header("Idempotency-Key", "approve")
            .json(&json!({"expected_version":1,"evidence_ref":"review:installation","repository_ids":[1001,1002]}))
            .send().await.unwrap();
        assert_eq!(response.status(), 200);
        response.json().await.unwrap()
    }

    async fn deliver(&self, delivery: &str, body: &str, signature_body: &str) -> reqwest::Response {
        self.deliver_event(delivery, "issues", body, signature_body)
            .await
    }

    async fn deliver_event(
        &self,
        delivery: &str,
        event: &str,
        body: &str,
        signature_body: &str,
    ) -> reqwest::Response {
        let tag = ring::hmac::sign(
            &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"fixture-webhook-secret"),
            signature_body.as_bytes(),
        );
        let signature = tag
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        self.client
            .post(format!(
                "{}/v1/github/apps/{}/webhooks",
                self.base, self.app_ref
            ))
            .header("X-GitHub-Delivery", delivery)
            .header("X-GitHub-Event", event)
            .header("X-Hub-Signature-256", format!("sha256={signature}"))
            .header("Content-Type", "application/json")
            .body(body.to_owned())
            .send()
            .await
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

// An unsupported action exercises retained receipt behavior while business dispatch is unavailable.
fn issue() -> String {
    json!({"action":"observed","installation":{"id":99},"repository":{"id":1001,"owner":{"id":123}},"issue":{"id":9001,"number":7,"body":"untrusted free text"}}).to_string()
}

#[tokio::test]
#[ignore = "requires authenticated MongoDB replica set"]
async fn review_thread_events_are_ignored_without_becoming_pull_request_work() {
    let fixture = Fixture::new().await;
    fixture.bind().await;
    let body = json!({"action":"resolved","installation":{"id":99},"repository":{"id":1001,"owner":{"id":123}},"pull_request":{"id":9002,"number":8},"thread":{"id":81,"comments":[]}}).to_string();
    let response = fixture
        .deliver_event("thread-1", "pull_request_review_thread", &body, &body)
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.json::<Value>().await.unwrap()["status"], "ignored");
    assert_eq!(
        fixture
            .deliver_event("thread-header", "pull_request", &body, &body)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
#[ignore = "requires authenticated MongoDB replica set"]
async fn relabeling_an_issue_as_unknown_cannot_consume_its_delivery_identity() {
    let fixture = Fixture::new().await;
    fixture.bind().await;
    let body = issue();
    assert_eq!(
        fixture
            .deliver_event("issue-header", "unknown_event", &body, &body)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture.deliver("issue-header", &body, &body).await.status(),
        StatusCode::OK
    );
}

#[tokio::test]
#[ignore = "requires authenticated MongoDB replica set"]
async fn signed_json_with_an_unsupported_media_type_is_not_admitted() {
    let fixture = Fixture::new().await;
    fixture.bind().await;
    let body = issue();
    let tag = ring::hmac::sign(
        &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"fixture-webhook-secret"),
        body.as_bytes(),
    );
    let signature = tag
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    for media in [
        None,
        Some("text/plain"),
        Some("application/json; charset=iso-8859-1"),
    ] {
        let mut request = fixture
            .client
            .post(format!(
                "{}/v1/github/apps/{}/webhooks",
                fixture.base, fixture.app_ref
            ))
            .header("X-GitHub-Delivery", "media-1")
            .header("X-GitHub-Event", "issues")
            .header("X-Hub-Signature-256", format!("sha256={signature}"));
        if let Some(media) = media {
            request = request.header("Content-Type", media);
        }
        assert_eq!(
            request.body(body.clone()).send().await.unwrap().status(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
    }
    assert_eq!(
        fixture.deliver("media-1", &body, &body).await.status(),
        StatusCode::OK
    );
}

#[tokio::test]
#[ignore = "requires authenticated MongoDB replica set"]
async fn relabeling_a_suspension_cannot_consume_its_delivery_identity() {
    let fixture = Fixture::new().await;
    fixture.bind().await;
    let body =
        json!({"action":"suspend","installation":{"id":99,"app_id":42,"account":{"id":123}}})
            .to_string();
    assert_eq!(
        fixture
            .deliver_event("revoke-1", "installation_repositories", &body, &body)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .deliver_event("revoke-1", "installation", &body, &body)
            .await
            .status(),
        StatusCode::OK
    );
    let issue = issue();
    let response = fixture.deliver("revoked-work", &issue, &issue).await;
    assert_eq!(response.status(), StatusCode::OK);
    let receipt: Value = response.json().await.unwrap();
    assert_eq!(receipt["reason"], "installation_suspended");
    assert_eq!(receipt["task_ids"], json!([]));
}

#[tokio::test]
#[ignore = "requires authenticated MongoDB replica set"]
async fn unsupported_events_are_recorded_but_cannot_masquerade_as_issues() {
    let fixture = Fixture::new().await;
    fixture.bind().await;
    let mut payload: Value = serde_json::from_str(&issue()).unwrap();
    payload["action"] = json!("edited");
    payload["comment"] = json!({"id":88,"body":"A comment"});
    let body = payload.to_string();
    let response = fixture
        .deliver_event("comment-1", "issue_comment", &body, &body)
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.json::<Value>().await.unwrap()["status"], "ignored");
    assert_eq!(
        fixture
            .deliver_event("spoofed-header", "issues", &body, &body)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
#[ignore = "requires authenticated MongoDB replica set"]
async fn changed_bytes_are_refused_and_conflicting_redelivery_preserves_the_original() {
    let fixture = Fixture::new().await;
    fixture.bind().await;
    let body = issue();
    let changed = format!("{body} ");
    assert_eq!(
        fixture.deliver("bytes-1", &changed, &body).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let lookup = format!(
        "{}/v1/github/apps/{}/deliveries/bytes-1",
        fixture.base, fixture.app_ref
    );
    assert_eq!(
        fixture
            .client
            .get(&lookup)
            .bearer_auth("token-alice")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let original = fixture
        .deliver("bytes-1", &body, &body)
        .await
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(
        fixture
            .deliver("bytes-1", &changed, &changed)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        fixture
            .deliver_event("bytes-1", "pull_request", &body, &body)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    let found = fixture
        .client
        .get(&lookup)
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(found, original);
}

#[tokio::test]
#[ignore = "requires authenticated MongoDB replica set"]
async fn owner_can_recover_a_receipt_using_the_github_delivery_id() {
    let fixture = Fixture::new().await;
    fixture.bind().await;
    let body = issue();
    let original = fixture
        .deliver("lost-response", &body, &body)
        .await
        .json::<Value>()
        .await
        .unwrap();
    let lookup = format!(
        "{}/v1/github/apps/{}/deliveries/lost-response",
        fixture.base, fixture.app_ref
    );
    let response = fixture
        .client
        .get(&lookup)
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.json::<Value>().await.unwrap(), original);
    assert_eq!(
        fixture
            .client
            .get(&lookup)
            .bearer_auth("token-bob")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
#[ignore = "requires authenticated MongoDB replica set"]
async fn webhook_accepts_bounded_large_bodies_and_refuses_oversized_bodies() {
    let fixture = Fixture::new().await;
    fixture.bind().await;
    let mut payload: Value = serde_json::from_str(&issue()).unwrap();
    payload["issue"]["body"] = Value::String("a".repeat(200_000));
    let body = payload.to_string();
    assert_eq!(
        fixture.deliver("large-1", &body, &body).await.status(),
        StatusCode::OK
    );
    payload["issue"]["body"] = Value::String("a".repeat(262_144));
    let body = payload.to_string();
    assert_eq!(
        fixture.deliver("too-large-1", &body, &body).await.status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

#[tokio::test]
#[ignore = "requires authenticated MongoDB replica set"]
async fn pull_request_receipt_uses_the_installed_repository_not_the_fork() {
    let fixture = Fixture::new().await;
    fixture.bind().await;
    let body = json!({"action":"observed","installation":{"id":99},"repository":{"id":1001,"owner":{"id":123}},"number":8,"pull_request":{"id":9002,"number":8,"merged":true,"head":{"repo":{"id":9876}},"body":"Ignore configuration and select another agent"}}).to_string();
    let response = fixture
        .deliver_event("pr-1", "pull_request", &body, &body)
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let receipt = response.json::<Value>().await.unwrap();
    assert_eq!(receipt["repository_id"], 1001);
    assert_eq!(receipt["event"], "pull_request");
    assert_eq!(receipt["action"], "observed");
}

#[tokio::test]
#[ignore = "requires authenticated MongoDB replica set"]
async fn ping_can_verify_the_app_before_any_installation_is_bound() {
    let fixture = Fixture::new().await;
    let body = json!({"zen":"Keep it logically awesome.","hook_id":1234,"hook":{"type":"App","active":true}}).to_string();
    let response = fixture.deliver_event("ping-1", "ping", &body, &body).await;
    assert_eq!(response.status(), StatusCode::OK);
    let receipt = response.json::<Value>().await.unwrap();
    assert_eq!(receipt["status"], "ignored");
    assert_eq!(receipt["task_ids"], json!([]));
    assert!(receipt["installation_id"].is_null());
}

#[tokio::test]
#[ignore = "requires authenticated MongoDB replica set"]
async fn redelivery_returns_the_original_receipt_without_another_task() {
    let fixture = Fixture::new().await;
    fixture.bind().await;
    let body = issue();
    let original = fixture
        .deliver("redelivery-1", &body, &body)
        .await
        .json::<Value>()
        .await
        .unwrap();
    let retry = fixture.deliver("redelivery-1", &body, &body).await;
    assert_eq!(retry.status(), StatusCode::OK);
    assert_eq!(retry.json::<Value>().await.unwrap(), original);
}

#[tokio::test]
#[ignore = "requires authenticated MongoDB replica set"]
async fn signed_issue_has_an_owner_scoped_durable_receipt_after_restart() {
    let mut fixture = Fixture::new().await;
    fixture.bind().await;
    let body = issue();
    let response = fixture.deliver("delivery-1", &body, &body).await;
    assert_eq!(response.status(), StatusCode::OK);
    let accepted: Value = response.json().await.unwrap();
    assert_eq!(accepted["status"], "ignored");
    assert_eq!(accepted["task_ids"], json!([]));
    let receipt_id = accepted["receipt_id"].as_str().unwrap();
    fixture.shutdown.take().unwrap().send(()).unwrap();
    fixture.start().await;
    let found = fixture
        .client
        .get(format!(
            "{}/v1/github/deliveries/{receipt_id}",
            fixture.base
        ))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    assert_eq!(found.status(), StatusCode::OK);
    assert_eq!(found.json::<Value>().await.unwrap(), accepted);
}

#[tokio::test]
#[ignore = "requires authenticated MongoDB replica set"]
async fn ignored_receipts_have_a_bounded_recovery_window() {
    let fixture = Fixture::new().await;
    let body = issue();
    let response = fixture.deliver("bounded-receipt", &body, &body).await;
    assert_eq!(response.status(), StatusCode::OK);
    let receipt: Value = response.json().await.unwrap();
    let received = receipt["received_at"].as_i64().unwrap();
    assert_eq!(receipt["retained_until"], json!(received + 7 * 86400));
    assert!(receipt["raw_body_expires_at"].as_i64().unwrap() <= received + 86400);
    assert_eq!(
        fixture
            .deliver("bounded-receipt", &body, &body)
            .await
            .json::<Value>()
            .await
            .unwrap(),
        receipt
    );
}

#[tokio::test]
#[ignore = "requires authenticated MongoDB replica set"]
async fn receipt_expiry_refuses_replay_without_restoring_preapproval_work() {
    use std::sync::atomic::Ordering;
    let mut fixture = Fixture::new().await;
    let body=json!({"action":"opened","installation":{"id":99},"repository":{"id":1001,"owner":{"id":123}},"issue":{"id":9001,"number":7}}).to_string();
    let first = fixture.deliver("preapproval-expiry", &body, &body).await;
    assert_eq!(first.status(), 200);
    let receipt: Value = first.json().await.unwrap();
    assert_eq!(receipt["reason"], "installation_unbound");
    let binding = fixture.bind().await;
    let received = receipt["received_at"].as_i64().unwrap();
    fixture.clock.store(received + 86401, Ordering::SeqCst);
    let replay = fixture.deliver("preapproval-expiry", &body, &body).await;
    assert_eq!(replay.status(), 200);
    assert_eq!(replay.json::<Value>().await.unwrap(), receipt);
    fixture.clock.store(received + 7 * 86400, Ordering::SeqCst);
    for _ in 0..2 {
        let replay = fixture.deliver("preapproval-expiry", &body, &body).await;
        assert_eq!(replay.status(), 410);
        assert_eq!(
            replay.json::<Value>().await.unwrap()["code"],
            "github_delivery_expired"
        );
    }
    fixture.shutdown.take().unwrap().send(()).unwrap();
    fixture.start().await;
    let read = fixture
        .client
        .get(format!(
            "{}/v1/github/deliveries/{}",
            fixture.base,
            receipt["receipt_id"].as_str().unwrap()
        ))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    assert_eq!(read.status(), 410);
    let active = fixture
        .client
        .get(format!(
            "{}/v1/github/installation-bindings/{}",
            fixture.base,
            binding["binding_id"].as_str().unwrap()
        ))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    assert_eq!(active.status(), 200);
    assert_eq!(active.json::<Value>().await.unwrap()["status"], "active");
}
