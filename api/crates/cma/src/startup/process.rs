use base64::Engine;
use ring::rand::SecureRandom;
use std::{path::Path, process::Stdio};

pub(super) fn command(uri: &str, database: &str, key: &Path) -> tokio::process::Command {
    let mut encryption = [0_u8; 32];
    ring::rand::SystemRandom::new()
        .fill(&mut encryption)
        .unwrap();
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "startup::tests::actual_startup_child",
            "--ignored",
            "--nocapture",
        ])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("RUST_MIN_STACK", "33554432")
        .env("CMA_TEST_STARTUP_CHILD", "1")
        .env("CHRONO_AUTH_MODE", "trusted")
        .env(
            "CHRONO_AUTH_ENCRYPTION_KEY",
            base64::engine::general_purpose::STANDARD.encode(encryption),
        )
        .env("CHRONO_SERVER_BIND", "127.0.0.1:0")
        .env("CHRONO_SERVER_PUBLIC_URL", "https://api.example")
        .env("CHRONO_METRICS_BIND", "127.0.0.1:0")
        .env("CHRONO_MONGODB_URI", uri)
        .env("CHRONO_MONGODB_DATABASE", database)
        .env("CHRONO_MONGODB_AUTHORITY_MODE", "shared-schema")
        .env("CHRONO_SANDBOX_BASE_URL", "http://127.0.0.1:9")
        .env("CHRONO_SANDBOX_API_KEY_FILE", key)
        .env(
            "CHRONO_SANDBOX_IMAGE",
            format!("registry.example/runtime@sha256:{}", "a".repeat(64)),
        )
        .env("CHRONO_SANDBOX_NAMESPACE", "synthetic-startup")
        .env("CHRONO_UPSTREAM_BASE_URL", "https://models.example")
        .env("CHRONO_UPSTREAM_WIRE_API", "responses")
        .env("CHRONO_PLATFORM_BASE_URL", "https://platform.example")
        .env("CHRONO_DEPLOYMENT_ID", "synthetic-startup")
        .env("CHRONO_AGENT_MANAGER_REPLICAS_ENABLED", "true")
        .env(
            "CHRONO_AGENT_MANAGER_RELAY_ADDRESS",
            "http://127.0.0.1:8081",
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // Preserve only explicit test instrumentation after clearing the child
    // environment; each child uses the runner's per-process profile path.
    if let Ok(profile) = std::env::var("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
    command
}
