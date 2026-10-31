use super::*;
use crate::config::RuntimeConfig;
use layer3_adapter::wire::AGENT_RESOURCES_STATUS_MEDIA_TYPE;
use layer3_adapter::RuntimeStatusRequest;

#[tokio::test]
async fn agent_resource_status_extension_is_explicitly_negotiated_per_request() {
    let config = RuntimeConfig {
        home_identity: Some("a".repeat(43)),
        ..RuntimeConfig::for_test(root("resources-negotiation"))
    };
    std::fs::create_dir_all(&config.home).unwrap();
    let supervisor = Supervisor::new(
        config.clone(),
        crate::writer_fence::SupervisorBoundary::default(),
    )
    .unwrap();
    let request = RequestFrame::RuntimeStatus(RuntimeStatusRequest {
        sandbox_id: config.sandbox_id.clone(),
        generation: config.generation,
        home_identity: config.home_identity.clone().unwrap(),
    });
    let (address, server) = serve(supervisor).await;
    let client = reqwest::Client::new();
    let mut original = None;
    for accept in [
        None,
        Some(AGENT_RESOURCES_STATUS_MEDIA_TYPE),
        Some("application/json"),
        None,
    ] {
        let mut http = client
            .post(format!("http://{address}/runtime/control"))
            .json(&request);
        if let Some(accept) = accept {
            http = http.header(header::ACCEPT, accept);
        }
        let mut value: serde_json::Value = http.send().await.unwrap().json().await.unwrap();
        assert_eq!(value["t"], "runtime_status");
        let opted_in = accept == Some(AGENT_RESOURCES_STATUS_MEDIA_TYPE);
        assert_eq!(value.get("agent_resource_preparation").is_some(), opted_in);
        let decoded: ResponseFrame = serde_json::from_value(value.clone()).unwrap();
        let ResponseFrame::RuntimeStatus(status) = decoded else {
            panic!("expected status")
        };
        assert_eq!(status.agent_resource_preparation, opted_in);
        value
            .as_object_mut()
            .unwrap()
            .remove("agent_resource_preparation");
        if let Some(original) = &original {
            assert_eq!(&value, original);
        } else {
            original = Some(value);
        }
    }
    // Other contract-3 frames embed status and must stay readable by old APIs.
    for accept in ["application/json", AGENT_RESOURCES_STATUS_MEDIA_TYPE] {
        let RequestFrame::RuntimeStatus(identity) = &request else {
            unreachable!()
        };
        let response: serde_json::Value = client
            .post(format!("http://{address}/runtime/control"))
            .header(header::ACCEPT, accept)
            .json(&RequestFrame::WorkspaceRemovalSupport(identity.clone()))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(response["t"], "workspace_removal_support");
        assert!(response["runtime"]
            .get("agent_resource_preparation")
            .is_none());
    }
    server.abort();
    let _ = server.await;
    for path in [&config.home, &config.state_root, &config.checkpoint_root] {
        let _ = std::fs::remove_dir_all(path);
    }
}
