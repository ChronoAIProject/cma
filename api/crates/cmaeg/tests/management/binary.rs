use super::*;
use std::{fs, path::PathBuf, process::Stdio, time::Duration};
use tokio::process::Command;

struct MountedSecrets {
    dir: PathBuf,
}
impl MountedSecrets {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("cmaeg-management-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::write(
            dir.join("keys"),
            format!("1:{}\n", STANDARD_NO_PAD.encode([1_u8; 32])),
        )
        .unwrap();
        fs::write(dir.join("token"), "cma-service\n").unwrap();
        Self { dir }
    }
    fn command(
        &self,
        address: &str,
        cma: &FakeCma,
        nyxid: &FakeNyxid,
        provider: &FakeProvider,
    ) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cmaeg"));
        command.kill_on_drop(true)
            .env_remove("CMAEG_MASTER_KEYS")
            .env_remove("CMAEG_CMA_SERVICE_TOKEN")
            .env_remove("CMAEG_CMA_CLIENT_ID")
            .env_remove("CMAEG_CMA_CLIENT_SECRET")
            .env("CMAEG_LISTEN", address)
            .env("CMAEG_METRICS_LISTEN", "127.0.0.1:0")
            .env("CMAEG_PUBLIC_URL", format!("http://{address}"))
            .env("CMAEG_MASTER_KEYS_FILE", self.dir.join("keys"))
            .env("CMAEG_CMA_SERVICE_TOKEN_FILE", self.dir.join("token"))
            .env("CMAEG_CMA_BASE_URL", &cma.base_url)
            .env("CMAEG_NYXID_BASE_URL", &nyxid.base_url)
            .env_remove("CMAEG_PROVIDER_CONNECT_TIMEOUT_MS")
            .env_remove("CMAEG_PROVIDER_REQUEST_TIMEOUT_MS")
            .env_remove("CMAEG_PROVIDER_MAX_INFLIGHT")
            .env_remove("CMAEG_TOOLS_NYXID_SLUG")
            .env_remove("CMAEG_CHANNEL_CACHE_TTL_MS")
            .env_remove("CMAEG_CHANNEL_NEGATIVE_TTL_MS")
            .env_remove("CMAEG_CHANNEL_CACHE_MAX")
            .env_remove("CMAEG_RESTORE_RATE_PER_MINUTE")
            .env_remove("CMAEG_RESTORE_TIMEOUT_MS")
            .env_remove("CMAEG_JWKS_TTL_MS")
            .env_remove("CMAEG_INGRESS_MAX_BODY")
            .env_remove("CMAEG_INGRESS_INFLIGHT")
            .env_remove("CMAEG_ACCEPT_BUDGET_MS")
            .env_remove("CMAEG_TURN_TIMEOUT_MS")
            .env_remove("CMAEG_DRAIN_SECONDS")
            .env_remove("CMAEG_BUSY_RETRY_MAX_MS")
            .env_remove("CMAEG_NYXID_REPLY_TIMEOUT_MS")
            .env_remove("CMAEG_HEARTBEAT_MS")
            .env_remove("CMAEG_ACK_TEXT")
            .env_remove("CMAEG_FAILURE_TEXT")
            .env("CMAEG_PROVIDERS", json!([{
                "slug":"cma_codex", "base_url":provider.base_url(), "kind":"responses_http",
                "paths": {"agent_card":"/.well-known/agent-card.json", "binding":"/api/v1/cmaeg/bindings/{binding_id}",
                    "conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}", "responses":"/api/v2/responses"},
                "max_inflight":8
            }]).to_string())
            .env("RUST_LOG", "cmaeg=trace");
        command
    }
}
impl Drop for MountedSecrets {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn assert_no_secrets(value: &str) {
    for secret in [
        "nyx_agent_secret",
        "nyx_route_secret",
        "aG1hYy1zZWNyZXQ=",
        "hmac-secret",
        "cma-service",
        "token-alice",
        "fixture-client-secret",
        "gateway-rekey-fixture",
    ] {
        assert!(
            !value.contains(secret),
            "secret leaked into response or trace"
        );
    }
}

async fn curl(
    method: &str,
    url: &str,
    file: Option<&std::path::Path>,
    status: u16,
) -> serde_json::Value {
    let mut command = Command::new("curl");
    command.args([
        "--silent",
        "--show-error",
        "--max-time",
        "5",
        "--request",
        method,
        "--header",
        "Authorization: Bearer token-alice",
        "--header",
        "Content-Type: application/json",
        "--write-out",
        "\n%{http_code}",
        url,
    ]);
    if let Some(file) = file {
        command
            .arg("--data-binary")
            .arg(format!("@{}", file.display()));
    }
    let output = command.output().await.unwrap();
    assert!(
        output.status.success(),
        "curl failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let raw = String::from_utf8(output.stdout).unwrap();
    assert_no_secrets(&raw);
    let (body, received) = raw.rsplit_once('\n').unwrap();
    assert_eq!(received, status.to_string(), "{body}");
    if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(body).unwrap()
    }
}

#[tokio::test]
async fn compiled_binary_loads_mounted_secrets_and_serves_curl_without_leaking_credentials() {
    binary_journey(false).await;
}

#[tokio::test]
async fn compiled_binary_renews_cma_auth_from_secret_environment_without_restart() {
    binary_journey(true).await;
}

async fn binary_journey(renewable: bool) {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    if renewable {
        nyxid
            .service_tokens
            .expires_in
            .store(3, std::sync::atomic::Ordering::SeqCst);
    }
    let files = MountedSecrets::new();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    drop(listener);
    let log_path = files.dir.join("trace.log");
    let log = fs::File::create(&log_path).unwrap();
    let mut command = files.command(&address, &cma, &nyxid, &provider);
    if renewable {
        command
            .env_remove("CMAEG_MASTER_KEYS_FILE")
            .env_remove("CMAEG_CMA_SERVICE_TOKEN_FILE")
            .env(
                "CMAEG_MASTER_KEYS",
                format!("1:{}", STANDARD_NO_PAD.encode([1_u8; 32])),
            )
            .env("CMAEG_CMA_SERVICE_TOKEN", "gateway-rekey-fixture")
            .env("CMAEG_CMA_CLIENT_ID", "fixture-client")
            .env("CMAEG_CMA_CLIENT_SECRET", "fixture-client-secret");
    }
    let mut child = command
        .stdout(Stdio::from(log.try_clone().unwrap()))
        .stderr(Stdio::from(log))
        .spawn()
        .unwrap();
    let base = format!("http://{address}");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(response) = reqwest::get(format!("{base}/readyz")).await {
            if response.status().is_success() {
                break;
            }
        }
        assert!(
            child.try_wait().unwrap().is_none(),
            "{}",
            fs::read_to_string(&log_path).unwrap()
        );
        assert!(
            tokio::time::Instant::now() < deadline,
            "binary did not become ready"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let create_file = files.dir.join("create.json");
    fs::write(&create_file, create_body().to_string()).unwrap();
    let created = curl(
        "POST",
        &format!("{base}/v1/channels"),
        Some(&create_file),
        201,
    )
    .await;
    let id = created["channel_id"].as_str().unwrap();
    assert!(id.starts_with("cmaeg1.ch."));
    if renewable {
        tokio::time::sleep(Duration::from_millis(3100)).await;
    }
    let read = curl("GET", &format!("{base}/v1/channels/{id}"), None, 200).await;
    if renewable {
        assert!(
            nyxid
                .service_tokens
                .calls
                .load(std::sync::atomic::Ordering::SeqCst)
                >= 2
        );
    }
    assert_eq!(read["definition"]["name"], "support-bot");
    let document = curl("GET", &format!("{base}/openapi.json"), None, 200).await;
    assert_eq!(document["paths"].as_object().unwrap().len(), 13);
    assert!(document["paths"]
        .get("/events/{event_ref}/reply-target")
        .is_some());
    assert!(document["paths"]
        .get("/reply-targets/{reply_target_ref}/messages")
        .is_some());
    let output = Command::new("curl")
        .args([
            "--silent",
            "--show-error",
            "--fail",
            "--max-time",
            "5",
            "--request",
            "POST",
            "--header",
            "Content-Type: application/json",
            "--header",
            "Authorization: Bearer nyx_abcdefghijklmnop",
            "--data-binary",
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
            &format!("{base}/mcp"),
        ])
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let listed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let tools = listed["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 4);
    for name in [
        "replyToEvent",
        "updateMessage",
        "sendTyping",
        "readEventContext",
    ] {
        assert!(tools.iter().any(|tool| tool["name"] == name));
        assert!(document["paths"].as_object().unwrap().values().any(|path| {
            path.as_object()
                .unwrap()
                .values()
                .any(|operation| operation["operationId"] == name)
        }));
    }
    curl(
        "DELETE",
        &format!("{base}/v1/channels/{id}?expected_version=1"),
        None,
        204,
    )
    .await;
    assert!(cma.records().is_empty());
    assert!(provider
        .requests()
        .iter()
        .any(|request| request.method == "DELETE"));
    child.kill().await.unwrap();
    child.wait().await.unwrap();
    let logs = fs::read_to_string(log_path).unwrap();
    assert!(logs.contains("CMAEG listening"));
    assert_no_secrets(&logs);
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}

#[tokio::test]
async fn compiled_binary_refuses_incomplete_or_malformed_service_account_environment() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let files = MountedSecrets::new();
    let secret = uuid::Uuid::new_v4().to_string();
    for (id, value) in [
        (Some("fixture-client"), None),
        (None, Some(secret.as_str())),
        (Some(""), Some(secret.as_str())),
        (Some("fixture-client"), Some("")),
        (Some("fixture-client"), Some("line-one\nline-two")),
    ] {
        let mut command = files.command("127.0.0.1:0", &cma, &nyxid, &provider);
        if let Some(id) = id {
            command.env("CMAEG_CMA_CLIENT_ID", id);
        }
        if let Some(value) = value {
            command.env("CMAEG_CMA_CLIENT_SECRET", value);
        }
        let output = command.output().await.unwrap();
        assert!(!output.status.success());
        let logs = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(logs.contains("client ID and secret must both be printable"));
        assert!(!logs.contains(&secret));
        assert!(!logs.contains("line-one"));
    }
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}

#[tokio::test]
async fn compiled_binary_refuses_missing_and_invalid_mounted_secrets() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let files = MountedSecrets::new();
    for key in [
        "CMAEG_MASTER_KEYS_FILE",
        "CMAEG_CMA_SERVICE_TOKEN_FILE",
        "CMAEG_PUBLIC_URL",
        "CMAEG_CMA_BASE_URL",
        "CMAEG_NYXID_BASE_URL",
    ] {
        let output = files
            .command("127.0.0.1:0", &cma, &nyxid, &provider)
            .env_remove(key)
            .output()
            .await
            .unwrap();
        assert!(!output.status.success());
        let logs = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(logs.contains(key), "{logs}");
    }
    for key in [
        "CMAEG_CHANNEL_CACHE_TTL_MS",
        "CMAEG_CHANNEL_NEGATIVE_TTL_MS",
        "CMAEG_CHANNEL_CACHE_MAX",
        "CMAEG_RESTORE_RATE_PER_MINUTE",
        "CMAEG_RESTORE_TIMEOUT_MS",
        "CMAEG_JWKS_TTL_MS",
        "CMAEG_INGRESS_MAX_BODY",
        "CMAEG_INGRESS_INFLIGHT",
        "CMAEG_ACCEPT_BUDGET_MS",
        "CMAEG_TURN_TIMEOUT_MS",
        "CMAEG_DRAIN_SECONDS",
        "CMAEG_BUSY_RETRY_MAX_MS",
        "CMAEG_NYXID_REPLY_TIMEOUT_MS",
        "CMAEG_HEARTBEAT_MS",
    ] {
        let output = files
            .command("127.0.0.1:0", &cma, &nyxid, &provider)
            .env(key, "0")
            .output()
            .await
            .unwrap();
        assert!(!output.status.success());
        let logs = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(logs.contains(key));
    }
    for (name, bad) in [
        ("keys", "invalid-master-fixture"),
        ("keys", "1:broken\n2:broken"),
        ("token", ""),
        ("token", "invalid token"),
    ] {
        let path = files.dir.join(name);
        let original = fs::read(&path).unwrap();
        fs::write(&path, bad).unwrap();
        let output = files
            .command("127.0.0.1:0", &cma, &nyxid, &provider)
            .output()
            .await
            .unwrap();
        assert!(!output.status.success());
        if !bad.is_empty() {
            let logs = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(!logs.contains(bad));
        }
        fs::write(path, original).unwrap();
    }
    for (name, bad) in [
        ("CMAEG_MASTER_KEYS", "invalid-master-fixture"),
        ("CMAEG_MASTER_KEYS", "1:broken\n2:broken"),
        ("CMAEG_CMA_SERVICE_TOKEN", ""),
        ("CMAEG_CMA_SERVICE_TOKEN", "invalid token"),
    ] {
        let output = files
            .command("127.0.0.1:0", &cma, &nyxid, &provider)
            .env_remove("CMAEG_MASTER_KEYS_FILE")
            .env_remove("CMAEG_CMA_SERVICE_TOKEN_FILE")
            .env(
                "CMAEG_MASTER_KEYS",
                format!("1:{}", STANDARD_NO_PAD.encode([1_u8; 32])),
            )
            .env("CMAEG_CMA_SERVICE_TOKEN", "cma-service")
            .env(name, bad)
            .output()
            .await
            .unwrap();
        assert!(!output.status.success());
        let logs = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(logs.contains(name));
        if !bad.is_empty() {
            assert!(!logs.contains(bad));
        }
        assert_no_secrets(&logs);
    }
    for name in ["CMAEG_MASTER_KEYS", "CMAEG_CMA_SERVICE_TOKEN"] {
        let output = files
            .command("127.0.0.1:0", &cma, &nyxid, &provider)
            .env(name, "ambiguous-secret-fixture")
            .output()
            .await
            .unwrap();
        assert!(!output.status.success());
        let logs = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(logs.contains("set only one"));
        assert!(!logs.contains("ambiguous-secret-fixture"));
    }
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}
