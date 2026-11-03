use super::*;
use layer3_adapter::hosted::{HostedControlTransport, ProviderHostedControlTransport};
use layer3_adapter::provider::{ProviderSandboxRequest, SandboxClient};

pub(super) struct NativeBoundary {
    pub transport: ProviderHostedControlTransport,
    origin: String,
}

impl NativeBoundary {
    pub async fn from_environment() -> Option<Self> {
        let directory = std::env::var("CMA_DESKTOP_RELAY_DIRECTORY").ok()?;
        let bytes = tokio::fs::read(std::path::Path::new(&directory).join("host-ready.json"))
            .await
            .unwrap();
        let ready: Value = serde_json::from_slice(&bytes).unwrap();
        let origin = ready["origin"].as_str().unwrap().to_owned();
        Some(Self {
            transport: ProviderHostedControlTransport::for_retained_opensandbox(
                SandboxClient::new(&origin, "local-proof-provider"),
            )
            .unwrap(),
            origin,
        })
    }

    pub async fn provision(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> layer3_adapter::Result<()> {
        let response = reqwest::Client::new()
            .post(format!("{}/proof/provision", self.origin))
            .json(request)
            .send()
            .await
            .map_err(|_| {
                AdapterError::Unavailable("native proof provider is unavailable".into())
            })?;
        if !response.status().is_success() {
            return Err(AdapterError::Unavailable(
                "native proof provider failed".into(),
            ));
        }
        self.transport
            .bind_runtime(handle, &request.env["CHRONO_SANDBOX_ID"])
            .await
    }
}

pub(super) async fn serve(service: Router, body: &Value) -> Option<tokio::task::JoinHandle<()>> {
    let directory = std::env::var("CMA_DESKTOP_RELAY_DIRECTORY").ok()?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ready =
        json!({ "origin": format!("http://{}", listener.local_addr().unwrap()), "body": body });
    tokio::fs::write(
        std::path::Path::new(&directory).join("public-ready.json"),
        serde_json::to_vec(&ready).unwrap(),
    )
    .await
    .unwrap();
    let service = service.route(
        "/openapi.json",
        axum::routing::get(|| async { axum::Json(layer1_public::rest::openapi::document()) }),
    );
    Some(tokio::spawn(async move {
        axum::serve(listener, service).await.unwrap();
    }))
}

pub(super) async fn creation(body: &Value) -> (String, Value) {
    let directory = std::env::var("CMA_DESKTOP_RELAY_DIRECTORY").unwrap();
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            if let Ok(bytes) =
                tokio::fs::read(std::path::Path::new(&directory).join("creation.json")).await
            {
                let result: Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(&result["body"], body);
                return (
                    result["key"].as_str().unwrap().to_owned(),
                    result["receipt"].clone(),
                );
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap()
}

pub(super) async fn finish() {
    let Ok(directory) = std::env::var("CMA_DESKTOP_RELAY_DIRECTORY") else {
        return;
    };
    let root = std::path::Path::new(&directory);
    tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            if let Ok(bytes) = tokio::fs::read(root.join("web-result.json")).await {
                let result: Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(result["passed"], true, "{result}");
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    tokio::fs::write(root.join("stop.json"), b"{}")
        .await
        .unwrap();
}

pub(super) async fn verify_process_recovery(
    database: &mongodb::Database,
    body: &Value,
    receipt: &Value,
    agent: &Agent,
) {
    let child = "sandbox::agent_worker_tests::sdk_conformance_tests::task_home_tests::subprocess::task_home_create_child";
    let invoke = |binary: std::path::PathBuf, request: Value| async move {
        tokio::time::timeout(
            Duration::from_secs(30),
            tokio::process::Command::new(binary)
                .args(["--exact", child, "--ignored", "--nocapture"])
                .env("CMA_TASK_CHILD_DATABASE", database.name())
                .env("CMA_TASK_CHILD_REQUEST", request.to_string())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .unwrap()
        .unwrap()
    };
    let result = invoke(std::env::current_exe().unwrap(), body.clone()).await;
    let output = String::from_utf8_lossy(&result.stdout);
    assert!(
        result.status.success(),
        "{output}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(output.contains("1 passed"), "{output}");
    let recovered: Value = serde_json::from_str(
        output
            .lines()
            .find_map(|line| line.strip_prefix("TASK_HOME_RECEIPT="))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(recovered["receipt"]["id"], receipt["id"]);
    assert_eq!(recovered["agent_id"], agent.id.as_str());
    if let Ok(binary) = std::env::var("CMA_DESKTOP_OLD_READER") {
        let mut old_request = body.clone();
        old_request.as_object_mut().unwrap().remove("desktop");
        old_request["workspace_id"] = json!(agent.workspace_id);
        let result = invoke(binary.into(), old_request).await;
        let output = format!(
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            !result.status.success(),
            "old strict reader unexpectedly accepted new desktop journal"
        );
        assert!(output.contains("temporarily_unavailable"), "{output}");
        if let Ok(directory) = std::env::var("CMA_DESKTOP_COMPAT_DIRECTORY") {
            tokio::fs::write(
                std::path::Path::new(&directory).join("old-reader-restart.log"),
                output,
            )
            .await
            .unwrap();
            tokio::fs::write(
                std::path::Path::new(&directory).join("new-reader-restart.json"),
                serde_json::to_vec_pretty(&recovered).unwrap(),
            )
            .await
            .unwrap();
        }
    }
}
