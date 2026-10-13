use super::binary_support::{assert_no_secrets, curl, free_address, spawn_ready, MountedSecrets};
use super::*;
use std::{fs, time::Duration};
use tokio::process::Command;

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn compiled_binary_loads_mounted_secrets_and_serves_curl_without_leaking_credentials() {
    binary_journey(false).await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
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
    let address = free_address();
    let log_path = files.dir.join("trace.log");
    let mut command = files.command(&address, &cma, &nyxid, &provider);
    let database =
        database::DatabaseFixture::new(std::path::Path::new(env!("CARGO_BIN_EXE_cmaeg"))).await;
    database.configure(&mut command);
    // Gateway records live in Mongo; the CMA origin now identifies the provider
    // whose management calls may receive the renewable service credential.
    command.env("CMAEG_CMA_BASE_URL", provider.base_url());
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
    let mut child = spawn_ready(command, &address, &log_path).await;
    let base = format!("http://{address}");
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
    assert_eq!(read["definition"]["name"], "support-bot");
    let document = curl("GET", &format!("{base}/openapi.json"), None, 200).await;
    assert!(document["paths"].get("/telegram/{channel_id}").is_some());
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
    curl("GET", &format!("{base}/v1/channels/{id}"), None, 404).await;
    // Reads now use Gateway Mongo. Provider teardown requires a fresh service
    // credential and proves renewal after the original credential expires.
    if renewable {
        assert!(
            nyxid
                .service_tokens
                .calls
                .load(std::sync::atomic::Ordering::SeqCst)
                >= 2
        );
    }

    assert!(provider.requests().iter().any(|request| {
        request.method == "DELETE" && request.authorization.as_deref() == Some("Bearer cma-service")
    }));
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
