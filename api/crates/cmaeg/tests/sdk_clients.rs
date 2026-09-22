#[path = "fake_cma.rs"]
pub mod fake_cma;
#[path = "fake_nyxid.rs"]
pub mod fake_nyxid;
#[path = "fake_provider.rs"]
pub mod fake_provider;

use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use std::{path::PathBuf, process::Stdio, sync::Arc, time::Duration};
use tokio::{
    process::{Child, Command},
    sync::Mutex,
};

#[derive(Clone)]
struct Fixture {
    provider: Arc<fake_provider::FakeProvider>,
    config: Value,
    gateway: Arc<Mutex<(Command, Child)>>,
}

async fn ready(gateway: &mut Child, base: &str) {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if http
                .get(format!("{base}/readyz"))
                .send()
                .await
                .is_ok_and(|r| r.status().is_success())
            {
                break;
            }
            assert!(
                gateway.try_wait().unwrap().is_none(),
                "gateway exited before readiness"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

async fn restart(State(state): State<Fixture>) -> Json<Value> {
    let mut process = state.gateway.lock().await;
    let (command, gateway) = &mut *process;
    let old_pid = gateway.id().unwrap();
    gateway.kill().await.unwrap();
    gateway.wait().await.unwrap();
    *gateway = command.spawn().unwrap();
    assert_ne!(gateway.id().unwrap(), old_pid);
    ready(gateway, state.config["gateway"].as_str().unwrap()).await;
    Json(json!({"replaced":true}))
}

async fn event_ref(State(state): State<Fixture>, Path(id): Path<String>) -> Json<Value> {
    let found = state.provider.requests().iter().find_map(|request| {
        let value: Value = serde_json::from_str(&request.body).ok()?;
        let context = value.get("event_context")?;
        (context["activity"]["event_id"] == id).then(|| context["event_ref"].clone())
    });
    Json(json!({"event_ref":found}))
}

fn create(agent_key: &str, signing: &str) -> Value {
    json!({
        "schema_version":1,"name":"SDK contract","record_id":uuid::Uuid::new_v4().to_string(),
        "profile":{"schema_version":1,
            "metadata":{"name":"SDK contract","owner":{"kind":"human","subject":"alice"},"provider":"cma_codex"},
            "model":{"access":"managed","service":"cma","api_type":"responses","model":"test","thinking_effort":"low"},
            "permission":{"action_mode":"guarded","service_scope":[]},"context":{"skills":[],"system_prompt":"","mcp_servers":[]}},
        "provider":{"slug":"cma_codex"},"sources":[{"type":"custom","auth":{"type":"hmac","key_id":"ingress"}}],
        "partition":{"version":1,"key":"per_event"},"reply":{"auto_final":true,"ack_after_ms":0,"sinks":[{"type":"sync"}]},
        "credentials":{"agent_key":agent_key,"hmac_secrets":[{"key_id":"ingress","secret":signing}]}
    })
}

/// Runs the installed SDK clients against the compiled gateway and isolated HTTP fakes.
#[tokio::test]
#[ignore = "requires built TypeScript SDK, Node, and the Rust SDK toolchain"]
async fn both_sdk_clients_create_sign_stream_reply_read_context_and_delete() {
    let provider = Arc::new(
        fake_provider::FakeProvider::start(fake_provider::FakeProviderConfig {
            terminal_delay: Duration::from_millis(1500),
            ..Default::default()
        })
        .await,
    );
    let cma = fake_cma::FakeCma::start(false).await;
    let nyxid = fake_nyxid::FakeNyxid::start().await;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let base = format!("http://{address}");
    let mut key = Vec::from(*uuid::Uuid::new_v4().as_bytes());
    key.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    let registry = json!([{"slug":"cma_codex","base_url":provider.base_url(),"kind":"responses_http","paths":{
        "agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}",
        "conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"},"max_inflight":8}]);
    let mut command = Command::new(env!("CARGO_BIN_EXE_cmaeg"));
    command
        .kill_on_drop(true)
        .env_clear()
        .env("CMAEG_LISTEN", address.to_string())
        .env("CMAEG_METRICS_LISTEN", "127.0.0.1:0")
        .env("CMAEG_PUBLIC_URL", &base)
        .env("CMAEG_MASTER_KEYS", format!("1:{}", STANDARD.encode(key)))
        .env("CMAEG_CMA_SERVICE_TOKEN", "cma-service")
        .env("CMAEG_CMA_BASE_URL", &cma.base_url)
        .env("CMAEG_NYXID_BASE_URL", &nyxid.base_url)
        .env("CMAEG_PROVIDERS", registry.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut gateway = command.spawn().unwrap();
    ready(&mut gateway, &base).await;
    let gateway = Arc::new(Mutex::new((command, gateway)));
    let agent_key = format!("nyx_{}", uuid::Uuid::new_v4().simple());
    let signing = STANDARD.encode(uuid::Uuid::new_v4().as_bytes());
    let fixture = Fixture {
        provider: provider.clone(),
        gateway: gateway.clone(),
        config: json!({
            "gateway":base,"creator":"token-alice","agent_key":agent_key,"signing":signing,"create":create(&agent_key,&signing)
        }),
    };
    let router = Router::new()
        .route(
            "/config",
            get(|State(state): State<Fixture>| async move { Json(state.config) }),
        )
        .route("/event-ref/{id}", get(event_ref))
        .route("/restart", post(restart))
        .with_state(fixture);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let root = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mut typescript = Command::new("node");
    typescript
        .arg("scripts/cmaeg-fixture.mjs")
        .current_dir(repo.join("sdk/typescript"));
    let mut rust = Command::new("cargo");
    rust.args([
        "+1.96.1",
        "test",
        "--locked",
        "--test",
        "cmaeg_fixture",
        "--",
        "--ignored",
        "--nocapture",
    ])
    .current_dir(repo.join("sdk/rust"))
    .env(
        "CARGO_TARGET_DIR",
        std::env::var_os("CMAEG_SDK_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| repo.join("sdk/rust/target")),
    )
    .env("CARGO_INCREMENTAL", "0")
    .env("CARGO_PROFILE_TEST_DEBUG", "0")
    .env("CARGO_PROFILE_DEV_DEBUG", "0");
    for command in [&mut typescript, &mut rust] {
        let output = tokio::time::timeout(
            Duration::from_secs(180),
            command
                .kill_on_drop(true)
                .env("CMAEG_FIXTURE_ROOT", &root)
                .output(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            output.status.success(),
            "SDK fixture failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    server.abort();
    let _ = server.await;
    let mut process = gateway.lock().await;
    process.1.kill().await.unwrap();
    process.1.wait().await.unwrap();
    cma.close().await;
    nyxid.close().await;
    Arc::try_unwrap(provider).ok().unwrap().close().await;
}
