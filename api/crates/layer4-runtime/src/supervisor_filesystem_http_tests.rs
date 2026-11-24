use axum::{body::to_bytes, http::Request, Router};
use base64::Engine as _;
use layer3_adapter::{CredentialFrame, InstallRequest, RuntimeCredential};
use serde_json::{json, Value};
use std::{os::unix::fs::symlink, path::PathBuf};
use tower::ServiceExt as _;

use super::*;

struct FilesystemFixture(PathBuf);

#[path = "supervisor_browser_http_tests.rs"]
mod browser_http;

impl Drop for FilesystemFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn exchange(router: &Router, frame: Value) -> Value {
    let response = router
        .clone()
        .oneshot(
            Request::post("/runtime/control")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(frame.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap()
}

#[tokio::test]
async fn browser_observation_is_disabled_by_default_and_fences_the_runtime_boot() {
    let fixture = FilesystemFixture(std::env::temp_dir().join(format!(
        "cma-browser-scope-{}",
        uuid::Uuid::new_v4().simple()
    )));
    let config = RuntimeConfig::for_test(fixture.0.clone());
    let supervisor = Supervisor::new(config, SupervisorBoundary::default()).unwrap();
    let router = crate::link::router(Arc::clone(&supervisor));
    exchange(
        &router,
        serde_json::to_value(RequestFrame::Install(InstallRequest::new(
            "browser-scope-install",
            Vec::new(),
        )))
        .unwrap(),
    )
    .await;
    exchange(
        &router,
        serde_json::to_value(RequestFrame::Credential(CredentialFrame {
            operation_id: "browser-scope-credential".into(),
            value: RuntimeCredential::new("browser-scope-credential"),
            custody_generation: Some(1),
            github_route: None,
        }))
        .unwrap(),
    )
    .await;
    let request = json!({"t":"browser", "sandbox_id":"sandbox-test", "runtime_generation":1,
        "boot_id":supervisor.boot_id, "agent_id":"agent-a", "operation":"status"});
    let response = exchange(&router, request.clone()).await;
    assert_eq!(response["t"], "browser");
    assert_eq!(response["observation"]["state"], "not_enabled");
    assert!(response["observation"]["frame"].is_null());
    let mut stale = request.clone();
    stale["boot_id"] = json!("0".repeat(32));
    assert_eq!(exchange(&router, stale).await["code"], "browser_identity");
    let mut absent = request;
    absent["operation"] = json!("enable");
    assert_eq!(
        exchange(&router, absent).await["code"],
        "browser_agent_absent"
    );
}

#[tokio::test]
async fn filesystem_http_hides_configured_private_state_and_refuses_aliases_without_breaking_settings(
) {
    let fixture = FilesystemFixture(
        std::env::temp_dir().join(format!("cma-file-scope-{}", uuid::Uuid::new_v4().simple())),
    );
    let mut config = RuntimeConfig::for_test(fixture.0.clone());
    config.state_root = fixture.0.join("runtime-private");
    config.checkpoint_root = fixture.0.join("checkpoint-private");
    let supervisor = Supervisor::new(config.clone(), SupervisorBoundary::default()).unwrap();
    let router = crate::link::router(Arc::clone(&supervisor));
    let installed = exchange(
        &router,
        serde_json::to_value(RequestFrame::Install(InstallRequest::new(
            "file-scope-install",
            Vec::new(),
        )))
        .unwrap(),
    )
    .await;
    assert_eq!(installed["t"], "install");
    let credentialed = exchange(
        &router,
        serde_json::to_value(RequestFrame::Credential(CredentialFrame {
            operation_id: "file-scope-credential".into(),
            value: RuntimeCredential::new("file-scope-credential"),
            custody_generation: Some(1),
            github_route: None,
        }))
        .unwrap(),
    )
    .await;
    assert_eq!(credentialed["t"], "credential");
    for path in [
        "runtime-private/state.json",
        "checkpoint-private/archive.bin",
    ] {
        std::fs::create_dir_all(fixture.0.join(path).parent().unwrap()).unwrap();
        std::fs::write(fixture.0.join(path), b"private retained state").unwrap();
    }
    std::fs::create_dir_all(config.agent_home("agent-a")).unwrap();
    std::fs::write(
        config.agent_home("agent-a").join("AGENTS.md"),
        b"managed instructions",
    )
    .unwrap();
    std::fs::write(fixture.0.join("workspace.txt"), b"shared workspace").unwrap();
    symlink("runtime-private/state.json", fixture.0.join("shortcut")).unwrap();
    std::fs::hard_link(
        fixture.0.join("checkpoint-private/archive.bin"),
        fixture.0.join("hardlink"),
    )
    .unwrap();
    let frame = |operation: &str, path: &str| {
        let mut value = json!({
            "t": operation,
            "owner_access": true,
            "protected_paths": [],
            "sandbox_id": "sandbox-test",
            "runtime_generation": 1,
            "boot_id": supervisor.boot_id,
            "path": path,
        });
        if operation == "read_agent_file" {
            value["max_bytes"] = json!(1024);
        }
        value
    };
    for path in [
        "runtime-private/state.json",
        "checkpoint-private/archive.bin",
        "shortcut",
        "hardlink",
    ] {
        let reply = exchange(&router, frame("read_agent_file", path)).await;
        assert_eq!(reply["t"], "failure", "{path}: {reply}");
    }
    let listing = exchange(&router, frame("list_agent_files", "")).await;
    assert_eq!(listing["t"], "list_agent_files");
    assert!(listing["entries"].as_array().unwrap().iter().all(|entry| {
        entry["name"] != "runtime-private" && entry["name"] != "checkpoint-private"
    }));
    for path in ["runtime-private", "checkpoint-private"] {
        let mut deletion = frame("delete_agent_file", path);
        deletion["recursive"] = json!(true);
        assert_eq!(exchange(&router, deletion).await["t"], "failure");
        for directory in [false, true] {
            let mut mutation = frame("write_agent_file", &format!("{path}/new-entry"));
            mutation["content_base64"] = json!("");
            mutation["expected_sha256"] = Value::Null;
            mutation["directory"] = json!(directory);
            assert_eq!(exchange(&router, mutation).await["t"], "failure");
        }
    }
    for (path, expected) in [
        ("workspace.txt", "shared workspace"),
        (".cma/agents/agent-a/home/AGENTS.md", "managed instructions"),
    ] {
        let reply = exchange(&router, frame("read_agent_file", path)).await;
        assert_eq!(reply["t"], "read_agent_file", "{reply}");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(reply["content_base64"].as_str().unwrap())
                .unwrap(),
            expected.as_bytes()
        );
    }
}
