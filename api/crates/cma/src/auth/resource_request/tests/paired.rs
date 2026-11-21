//! Opt-in boundary acceptance against the real, isolated NyxID fixture server.
//! No production credentials, runtime, database, or operation store is used by CMA.

use super::*;
use serde::Deserialize;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    base_url: String,
    client_id: String,
    client_secret: String,
    service_id: String,
    owner: String,
    ordinary_key: String,
    control_token: String,
}

fn manifest() -> Manifest {
    let file = std::env::var_os("CMA_NYXID_PAIR_MANIFEST")
        .expect("set CMA_NYXID_PAIR_MANIFEST to the private local fixture manifest");
    let metadata = std::fs::symlink_metadata(&file).expect("fixture manifest must exist");
    assert!(
        metadata.is_file(),
        "fixture manifest must be a regular file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(metadata.permissions().mode() & 0o077, 0);
    }
    let data = zeroize::Zeroizing::new(std::fs::read(file).unwrap());
    let manifest: Manifest = serde_json::from_slice(&data).expect("invalid fixture manifest");
    let base = url::Url::parse(&manifest.base_url).unwrap();
    assert_eq!(base.scheme(), "http");
    assert_eq!(base.host_str(), Some("127.0.0.1"));
    assert!(base.port().is_some_and(|port| port > 0));
    assert!(base.username().is_empty() && base.password().is_none());
    assert!(base.query().is_none() && base.fragment().is_none());
    assert_eq!(base.path(), "/");
    assert!(uuid::Uuid::parse_str(&manifest.owner).is_ok());
    manifest
}

struct Pair {
    manifest: Manifest,
    client: reqwest::Client,
    target: String,
    observed: Arc<RwLock<Option<String>>>,
    requests: Arc<AtomicUsize>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Pair {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Pair {
    async fn start(manifest: Manifest) -> Self {
        let store = Arc::new(MemoryAuthStore::default());
        // Independent credentials cannot borrow browser identity or custody.
        store.set_unavailable(true);
        let auth = AuthState::with_store(
            AuthMode::Provider {
                base_url: manifest.base_url.clone(),
                client_id: manifest.client_id.clone(),
                client_secret: manifest.client_secret.clone(),
                public_url: "http://127.0.0.1".into(),
            },
            None,
            store,
        )
        .with_agent_key_service(Some(manifest.service_id.clone()))
        .with_resource_request_enabled(true);
        let observed = Arc::new(RwLock::new(None));
        let recorded = observed.clone();
        let requests = Arc::new(AtomicUsize::new(0));
        let received = requests.clone();
        let app = public_routes::public_router_for(auth, &manifest.owner, "another-owner")
            .await
            .layer(axum::middleware::from_fn(
                move |request: Request, next: Next| {
                    let recorded = recorded.clone();
                    let received = received.clone();
                    async move {
                        received.fetch_add(1, Ordering::SeqCst);
                        if let Some(proof) = request.headers().get(HEADER) {
                            *recorded.write().await = Some(proof.to_str().unwrap().to_owned());
                        }
                        next.run(request).await
                    }
                },
            ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            manifest,
            target,
            observed,
            requests,
            server,
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(10))
                .no_proxy()
                .build()
                .unwrap(),
        }
    }

    async fn control(&self, value: Value) -> Value {
        let response = self
            .client
            .post(format!("{}/fixture/control", self.manifest.base_url))
            .bearer_auth(&self.manifest.control_token)
            .json(&value)
            .send()
            .await
            .expect("local fixture control transport failed");
        assert!(response.status().is_success(), "fixture control failed");
        response
            .json()
            .await
            .expect("fixture control returned invalid JSON")
    }

    async fn ordinary(&self, expected: StatusCode) -> Value {
        let response = self
            .client
            .get(format!("{}/api/v1/agents", self.target))
            .bearer_auth(&self.manifest.ordinary_key)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), expected.as_u16());
        response.json().await.unwrap()
    }

    async fn change(&self, value: Value) {
        let response = self.control(value).await;
        assert_eq!(response.get("ok").and_then(Value::as_bool), Some(true));
    }

    async fn replay(&self, proof: &str, method: Method, path: &str, expected: StatusCode) {
        let response = self
            .client
            .request(method, format!("{}{path}", self.target))
            .header(HEADER, proof)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), expected.as_u16());
    }

    async fn forward(&self, transport: &str, path: &str, expected: StatusCode) -> Value {
        let response = self
            .control(json!({
                "action": "forward", "target": self.target,
                "path": path, "transport": transport,
            }))
            .await;
        assert_eq!(
            response["status"].as_u64(),
            Some(u64::from(expected.as_u16()))
        );
        response["body"].clone()
    }
}

#[tokio::test]
#[ignore = "requires the real NyxID loopback acceptance fixture and dedicated MongoDB"]
async fn real_nyxid_keys_and_egress_authorize_only_current_owner_reads() {
    let pair = Pair::start(manifest()).await;
    let own_id = format!("agt_{}", pair.manifest.owner);
    let list = pair.ordinary(StatusCode::OK).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert_eq!(list["items"][0]["id"], own_id);
    let foreign = pair
        .client
        .get(format!("{}/api/v1/agents/agt_another-owner", pair.target))
        .bearer_auth(&pair.manifest.ordinary_key)
        .send()
        .await
        .unwrap();
    assert_eq!(foreign.status().as_u16(), 404);

    let path = "/api/v1/agents?limit=1";
    let list = pair.forward("mcp", path, StatusCode::OK).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert_eq!(list["items"][0]["id"], own_id);
    let proof = zeroize::Zeroizing::new(pair.observed.read().await.clone().unwrap());
    pair.replay(&proof, Method::GET, path, StatusCode::OK).await;
    pair.replay(
        &proof,
        Method::GET,
        "/api/v1/agents?limit=2",
        StatusCode::UNAUTHORIZED,
    )
    .await;
    pair.replay(
        &proof,
        Method::POST,
        "/api/v1/agents",
        StatusCode::FORBIDDEN,
    )
    .await;

    let transcript = pair
        .forward(
            "rest",
            &format!("/api/v1/agents/{own_id}/transcript"),
            StatusCode::OK,
        )
        .await;
    assert_eq!(transcript["agent_id"], own_id);
    assert!(transcript
        .to_string()
        .contains(&format!("Latest question from {}", pair.manifest.owner)));
    assert!(!transcript.to_string().contains("another-owner"));
    pair.forward(
        "rest",
        "/api/v1/agents/agt_another-owner",
        StatusCode::NOT_FOUND,
    )
    .await;

    pair.change(json!({"action":"permissions", "enabled":false}))
        .await;
    pair.ordinary(StatusCode::FORBIDDEN).await;
    pair.replay(&proof, Method::GET, path, StatusCode::FORBIDDEN)
        .await;
    pair.change(json!({"action":"permissions", "enabled":true}))
        .await;
    pair.ordinary(StatusCode::OK).await;
    pair.replay(&proof, Method::GET, path, StatusCode::OK).await;

    pair.change(json!({"action":"service_grant", "enabled":false}))
        .await;
    pair.ordinary(StatusCode::UNAUTHORIZED).await;
    pair.replay(&proof, Method::GET, path, StatusCode::UNAUTHORIZED)
        .await;
    pair.change(json!({"action":"service_grant", "enabled":true}))
        .await;
    pair.ordinary(StatusCode::OK).await;
    pair.replay(&proof, Method::GET, path, StatusCode::OK).await;

    pair.change(json!({"action":"revoke_ordinary"})).await;
    pair.ordinary(StatusCode::UNAUTHORIZED).await;
    pair.replay(&proof, Method::GET, path, StatusCode::OK).await;
    pair.change(json!({"action":"proof_key_active", "enabled":false}))
        .await;
    pair.replay(&proof, Method::GET, path, StatusCode::UNAUTHORIZED)
        .await;
    pair.change(json!({"action":"proof_key_active", "enabled":true}))
        .await;
    pair.replay(&proof, Method::GET, path, StatusCode::OK).await;
    pair.change(json!({"action":"stop_turn"})).await;
    pair.replay(&proof, Method::GET, path, StatusCode::UNAUTHORIZED)
        .await;
    *pair.observed.write().await = None;
    let received_before_stop_retry = pair.requests.load(Ordering::SeqCst);
    let stopped = pair
        .control(json!({
            "action":"forward", "target":pair.target, "path":path, "transport":"rest",
        }))
        .await;
    assert!(matches!(stopped["status"].as_u64(), Some(400..=499)));
    assert!(pair.observed.read().await.is_none());
    assert_eq!(
        pair.requests.load(Ordering::SeqCst),
        received_before_stop_retry
    );
    pair.change(json!({"action":"shutdown"})).await;
}
