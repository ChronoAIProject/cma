//! Native persistent homes never acquire CMA archive authority or issue checkpoint commands.
use super::*;
use crate::agent_sandbox::{AgentSandboxClient, AgentSandboxConfig};
use crate::{
    CheckpointHoldRequest, CheckpointRequest, CheckpointWriterResumeRequest,
    RestoreBootstrapRequest, RestoreRequest,
};

fn native_transport(address: std::net::SocketAddr) -> ProviderHostedControlTransport {
    let client = AgentSandboxClient::new(
        kube::Client::try_from(kube::Config::new(
            format!("http://{address}").parse().unwrap(),
        ))
        .unwrap(),
        AgentSandboxConfig {
            namespace: "cma-test".into(),
            deployment: "cma-test".into(),
            service_account: "cma-runtime".into(),
            runtime_class: None,
            image_pull_secrets: vec![],
            template_network_policy: None,
            control_key: "fixture".repeat(8),
        },
        "fixture",
    )
    .unwrap();
    ProviderHostedControlTransport::for_agent_sandbox(client).unwrap()
}

#[tokio::test]
async fn native_binding_and_control_work_without_an_archive_store() {
    let api = listener().await;
    let transport = native_transport(api.local_addr().unwrap());
    let handle = RuntimeHandle::new("agent-sandbox:runtime-one:original-uid");
    transport.bind_runtime(&handle, "sandbox-a").await.unwrap();
    transport.bind_runtime(&handle, "sandbox-a").await.unwrap();
    assert!(matches!(
        transport.bind_runtime(&handle, "sandbox-b").await,
        Err(AdapterError::Rejected(_))
    ));
    let runtime = listener().await;
    let endpoint = RuntimeEndpoint::parse(
        "http://runtime.invalid",
        &format!("http://{}", runtime.local_addr().unwrap()),
    )
    .unwrap()
    .with_control_token("fixture-runtime-control".into());
    let response = ResponseFrame::Resources(ResourceReading::default());
    let served = spawn_http(runtime, json_response(&response));
    assert_eq!(
        transport
            .exchange_at(&handle, &endpoint, RequestFrame::Resources)
            .await
            .unwrap(),
        response
    );
    let request = request_text(served.await.unwrap());
    assert!(request.starts_with("POST /runtime/control HTTP/1.1"));
    assert!(request
        .to_ascii_lowercase()
        .contains("x-cma-runtime-token: fixture-runtime-control"));
    assert!(matches!(
        transport.archive(&handle).await,
        Err(AdapterError::Unsupported(_))
    ));
    transport.retire(&handle).await.unwrap();
    transport.bind_runtime(&handle, "sandbox-b").await.unwrap();
}

#[tokio::test]
async fn native_checkpoint_and_restore_are_rejected_before_any_provider_or_runtime_io() {
    let api = listener().await;
    let transport = native_transport(api.local_addr().unwrap());
    let handle = RuntimeHandle::new("agent-sandbox:runtime-one:original-uid");
    transport.bind_runtime(&handle, "sandbox-a").await.unwrap();
    let checkpoint = CheckpointRequest {
        epoch: 1,
        exclusions: vec![],
    };
    let held = CheckpointHoldRequest {
        checkpoint: checkpoint.clone(),
        writer_resume_token: "a".repeat(64),
    };
    let restore = RestoreRequest {
        object_reference: format!("home-{}.cma", "b".repeat(64)),
        sha256: "b".repeat(64),
    };
    for frame in [
        RequestFrame::Checkpoint(checkpoint),
        RequestFrame::CheckpointForTeardown(held.clone()),
        RequestFrame::ResumeCheckpointWriters(CheckpointWriterResumeRequest {
            writer_resume_token: "a".repeat(64),
        }),
        RequestFrame::BootstrapRestore(RestoreBootstrapRequest::new(
            "bootstrap-one",
            "sandbox-a",
            restore.clone(),
        )),
        RequestFrame::Restore(restore),
    ] {
        assert!(matches!(
            transport.exchange(&handle, frame).await,
            Err(AdapterError::Unsupported(_))
        ));
    }
    assert!(matches!(
        transport.verify_checkpoint_hold(&handle, &held).await,
        Err(AdapterError::Unsupported(_))
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), api.accept())
            .await
            .is_err()
    );
}
