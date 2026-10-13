//! Retained OpenSandbox recovery remains available after CMA archive retirement.
use super::*;
use crate::{
    CheckpointHoldRequest, CheckpointRequest, CheckpointWriterResumeReceipt,
    CheckpointWriterResumeRequest, RestoreBootstrapRequest, RestoreRequest,
};

#[tokio::test]
async fn retired_opensandbox_rejects_archive_operations_before_provider_io() {
    let provider = listener().await;
    let transport = ProviderHostedControlTransport::for_retained_opensandbox(SandboxClient::new(
        format!("http://{}", provider.local_addr().unwrap()),
        "fixture",
    ))
    .unwrap();
    let handle = RuntimeHandle::new("runtime-one");
    transport.bind_runtime(&handle, "sandbox-a").await.unwrap();
    assert!(matches!(
        transport.bind_runtime(&handle, "sandbox-b").await,
        Err(AdapterError::Rejected(_))
    ));
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
        tokio::time::timeout(Duration::from_millis(50), provider.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn retired_opensandbox_releases_existing_writer_holds_without_archive_authority() {
    let runtime = listener().await;
    let provider = listener().await;
    let transport = ProviderHostedControlTransport::for_retained_opensandbox(SandboxClient::new(
        format!("http://{}", provider.local_addr().unwrap()),
        "fixture",
    ))
    .unwrap();
    let provider_task = spawn_http(
        provider,
        json_response(&serde_json::json!({
            "endpoint": format!("{}/proxy/3001", runtime.local_addr().unwrap()),
        })),
    );
    let receipt = ResponseFrame::ResumeCheckpointWriters(CheckpointWriterResumeReceipt {
        writer_resume_token: "a".repeat(64),
        writers_resumed: true,
    });
    let runtime_task = spawn_http(runtime, json_response(&receipt));
    let handle = RuntimeHandle::new("runtime-one");
    transport.bind_runtime(&handle, "sandbox-a").await.unwrap();
    let frame = RequestFrame::ResumeCheckpointWriters(CheckpointWriterResumeRequest {
        writer_resume_token: "a".repeat(64),
    });
    assert_eq!(
        transport.exchange(&handle, frame.clone()).await.unwrap(),
        receipt
    );
    assert!(matches!(
        transport.archive(&handle).await,
        Err(AdapterError::Unsupported(_))
    ));
    provider_task.await.unwrap();
    let request = request_text(runtime_task.await.unwrap());
    assert_eq!(
        serde_json::from_str::<RequestFrame>(request.split_once("\r\n\r\n").unwrap().1).unwrap(),
        frame
    );
}
