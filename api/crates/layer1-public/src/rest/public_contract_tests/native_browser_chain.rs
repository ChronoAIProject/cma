use axum::Extension;
use layer3_adapter::{
    AdapterError, BrowserControlReceipt, BrowserControlRequest, BrowserReceipt, BrowserRequest,
    Result,
};
use serde_json::{json, Value};
use std::{path::Path, sync::Arc, time::Duration};

pub(super) async fn forward(directory: &Path, request: &BrowserRequest) -> Result<BrowserReceipt> {
    let value = forward_frame(directory, serde_json::to_value(request).unwrap(), "browser").await?;
    serde_json::from_value(value)
        .map_err(|_| AdapterError::Unavailable("invalid runtime browser reply".into()))
}

pub(super) async fn forward_control(
    directory: &Path,
    request: &BrowserControlRequest,
) -> Result<BrowserControlReceipt> {
    let value = forward_frame(
        directory,
        serde_json::to_value(request).unwrap(),
        "browser_control",
    )
    .await?;
    serde_json::from_value(value)
        .map_err(|_| AdapterError::Unavailable("invalid runtime control reply".into()))
}

async fn forward_frame(directory: &Path, mut frame: Value, operation: &str) -> Result<Value> {
    let identifier = uuid::Uuid::new_v4().to_string();
    let temporary = directory.join(format!("{identifier}.tmp"));
    let pending = directory.join(format!("{identifier}.request.json"));
    let response = directory.join(format!("{identifier}.response.json"));
    frame["t"] = json!(operation);
    tokio::fs::write(&temporary, serde_json::to_vec(&frame).unwrap())
        .await
        .map_err(|_| AdapterError::Unavailable("private proof transport unavailable".into()))?;
    tokio::fs::rename(temporary, &pending)
        .await
        .map_err(|_| AdapterError::Unavailable("private proof transport unavailable".into()))?;
    for _ in 0..500 {
        if let Ok(bytes) = tokio::fs::read(&response).await {
            let _ = tokio::fs::remove_file(&response).await;
            let value: Value = serde_json::from_slice(&bytes)
                .map_err(|_| AdapterError::Unavailable("invalid private proof reply".into()))?;
            if value["t"] == "failure" && value["retryable"] == false {
                return Err(AdapterError::Rejected(value["code"].to_string()));
            }
            if value["t"] != operation {
                return Err(AdapterError::Unavailable(
                    "runtime refused browser proof".into(),
                ));
            }
            return Ok(value);
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    let _ = tokio::fs::remove_file(pending).await;
    Err(AdapterError::Unavailable(
        "private proof transport timed out".into(),
    ))
}

#[tokio::test]
#[ignore = "requires the explicit pinned native runtime proof relay and Web driver"]
async fn serve_actual_runtime_browser_public_chain() {
    let directory = std::path::PathBuf::from(
        std::env::var("CMA_BROWSER_RELAY_DIRECTORY").expect("explicit private proof directory"),
    );
    let ready: Value =
        serde_json::from_slice(&tokio::fs::read(directory.join("ready.json")).await.unwrap())
            .unwrap();
    assert_eq!(ready["agent_id"], "agt_shared_read");
    assert_eq!(ready["sandbox_id"], "sbx_route");
    if let Ok(path) = std::env::var("CMA_CONTROL_PROOF_MANIFEST") {
        let manifest: Value =
            serde_json::from_slice(&tokio::fs::read(path).await.unwrap()).unwrap();
        assert_eq!(ready["control_protocol"], 1);
        assert_eq!(manifest["runtime_sha256"].as_str().unwrap().len(), 64);
        assert_eq!(manifest["source_commit"].as_str().unwrap().len(), 40);
        assert_eq!(ready["runtime_sha256"], manifest["runtime_sha256"]);
        assert_eq!(ready["source_commit"], manifest["source_commit"]);
    } else {
        assert_eq!(
            ready["runtime_sha256"],
            "364d71a3424476e5c7b27583b0df39cce1751e0d92be06347baa62a1e6dbf531"
        );
    }
    let boot = ready["boot_id"].as_str().unwrap();
    let mut servers = Vec::new();
    let mut origins = serde_json::Map::new();
    for kind in ["owner", "foreign", "viewer"] {
        let mut runtime = super::runtime::Files::new("browser_native_chain");
        runtime.browser_relay = Some(directory.clone());
        let fixture = super::scenario_with_runtime_boot(
            &format!("filesystem_{kind}"),
            Arc::new(runtime),
            boot,
        )
        .await
        .unwrap();
        let router = fixture
            .router
            .layer(Extension(crate::AuthenticatedSubject(
                layer2_core::SubjectId::new(fixture.subject.unwrap()),
            )))
            .layer(Extension(fixture.authorization));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        origins.insert(
            kind.into(),
            json!(format!("http://{}", listener.local_addr().unwrap())),
        );
        servers.push(tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        }));
    }
    tokio::fs::write(
        directory.join("public-ready.json"),
        serde_json::to_vec(&origins).unwrap(),
    )
    .await
    .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(240);
    while !directory.join("public-stop.json").exists() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "Web proof did not finish"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    for server in servers {
        server.abort();
    }
    let result: Value = serde_json::from_slice(
        &tokio::fs::read(directory.join("web-chain-result.json"))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(result["passed"], true);
    println!("Actual runtime browser public chain: {}", result);
}
