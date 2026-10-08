//! Starting the compiled gateway binary against the shared fakes, for the
//! binary-level management tests.
use super::*;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::{Child, Command};

pub(super) struct MountedSecrets {
    pub(super) dir: PathBuf,
}
impl MountedSecrets {
    pub(super) fn new() -> Self {
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
    pub(super) fn command(
        &self,
        address: &str,
        cma: &FakeCma,
        nyxid: &FakeNyxid,
        provider: &FakeProvider,
    ) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cmaeg"));
        command.kill_on_drop(true)
            .env_remove("CMAEG_ROUTED_CHANNEL_MAX_TURNS")
            .env_remove("CMAEG_ROUTED_OWNER_MAX_TURNS")
            .env_remove("CMAEG_ROUTING_UNKNOWN_TARGET_TEXT")
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

pub(super) fn assert_no_secrets(value: &str) {
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

pub(super) async fn curl(
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

/// A loopback address that was free a moment ago, for the gateway to bind.
pub(super) fn free_address() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    drop(listener);
    address
}

/// Start the gateway `command` with its output in `log_path`, then wait until
/// it reports ready on `address`.
pub(super) async fn spawn_ready(mut command: Command, address: &str, log_path: &Path) -> Child {
    let log = fs::File::create(log_path).unwrap();
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
            fs::read_to_string(log_path).unwrap()
        );
        assert!(
            tokio::time::Instant::now() < deadline,
            "binary did not become ready"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    child
}
