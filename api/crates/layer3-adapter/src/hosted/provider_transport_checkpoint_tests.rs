use sha2::{Digest as _, Sha256};

use super::*;
use crate::hosted::{HostedBackend, ProviderHostedBackend};
use crate::{
    CheckpointHoldReceipt, CheckpointHoldRequest, CheckpointReceipt, CheckpointRequest, WriterState,
};

fn held(bytes: &[u8]) -> (CheckpointHoldRequest, CheckpointHoldReceipt) {
    let request = CheckpointHoldRequest {
        checkpoint: CheckpointRequest {
            epoch: 17,
            exclusions: vec![".cache".into()],
        },
        writer_resume_token: "b".repeat(64),
    };
    let sha256 = format!("{:x}", Sha256::digest(bytes));
    let receipt = CheckpointHoldReceipt {
        checkpoint: CheckpointReceipt {
            epoch: request.checkpoint.epoch,
            object_reference: format!("home-{sha256}.cma"),
            sha256,
            writers: WriterState::Held,
        },
        writer_resume_token: request.writer_resume_token.clone(),
    };
    (request, receipt)
}

#[tokio::test]
async fn published_hold_verification_uses_authenticated_control_without_archive_access() {
    let (request, receipt) = held(b"already published archive");
    let response = ResponseFrame::CheckpointForTeardown(receipt);
    let (transport, provider, runtime) = transport_fixture(json_response(&response)).await;
    let handle = RuntimeHandle::new("runtime-1");
    transport.bind_runtime(&handle, "sandbox-a").await.unwrap();
    let backend = ProviderHostedBackend::new(
        match &transport.client {
            RuntimeDiscovery::Provider(client) => client.clone(),
            _ => unreachable!(),
        },
        Arc::new(transport),
        "checkpoint-test",
    )
    .unwrap();

    assert_eq!(
        backend
            .verify_checkpoint_hold(&handle, &request)
            .await
            .unwrap(),
        response,
    );
    provider.await.unwrap();
    let runtime = request_text(runtime.await.unwrap());
    assert!(runtime.starts_with("POST /proxy/3001/runtime/control HTTP/1.1"));
    assert!(runtime
        .to_ascii_lowercase()
        .contains("open-sandbox-api-key: provider-secret"));
    assert_eq!(
        serde_json::from_str::<RequestFrame>(runtime.split_once("\r\n\r\n").unwrap().1).unwrap(),
        RequestFrame::CheckpointForTeardown(request),
    );
}

#[derive(Default)]
struct CaptureStore {
    uploaded: StdMutex<Vec<Vec<u8>>>,
}

#[async_trait::async_trait]
impl CheckpointArchiveStore for CaptureStore {
    async fn upload(
        &self,
        mut archive: CheckpointArchiveUpload,
    ) -> crate::Result<CheckpointArchiveReceipt> {
        let mut bytes = Vec::new();
        while let Some(chunk) = archive.chunks.recv().await {
            bytes.extend(chunk?);
        }
        assert_eq!(bytes.len() as u64, archive.size_bytes);
        assert_eq!(format!("{:x}", Sha256::digest(&bytes)), archive.sha256);
        self.uploaded.lock().unwrap().push(bytes);
        Ok(CheckpointArchiveReceipt {
            object_reference: archive.object_reference,
            sha256: archive.sha256,
            size_bytes: archive.size_bytes,
        })
    }

    async fn download(&self, _: &str) -> crate::Result<CheckpointArchiveDownload> {
        panic!("hold capture or verification must not download from durable storage")
    }
}

#[tokio::test]
async fn initial_hold_still_publishes_the_complete_archive() {
    let bytes = b"new archive requiring durable publication";
    let (request, receipt) = held(bytes);
    let frame = RequestFrame::CheckpointForTeardown(request);
    let response = ResponseFrame::CheckpointForTeardown(receipt.clone());
    let runtime = listener().await;
    let endpoint = RuntimeEndpoint::parse(
        "http://127.0.0.1:1",
        &format!("{}/proxy/3001", runtime.local_addr().unwrap()),
    )
    .unwrap();
    let responses = [
        json_response(&response),
        http_response(
            "200 OK",
            &[("Content-Length", bytes.len().to_string())],
            bytes,
        ),
    ];
    let runtime = tokio::spawn(async move {
        let mut requests = Vec::new();
        for response in responses {
            let (mut stream, _) = runtime.accept().await.unwrap();
            requests.push(read_request(&mut stream).await);
            stream.write_all(&response).await.unwrap();
            stream.shutdown().await.unwrap();
        }
        requests
    });
    let transport = ProviderHostedControlTransport::new(
        SandboxClient::new("http://127.0.0.1:1", "provider-secret"),
        Arc::new(RecordingFactory::default()),
    )
    .unwrap();
    let handle = RuntimeHandle::new("runtime-1");
    let store = Arc::new(CaptureStore::default());
    transport.state.lock().await.archives.insert(
        handle.clone(),
        BoundArchive {
            sandbox_id: "sandbox-a".into(),
            store: store.clone(),
        },
    );

    assert_eq!(
        transport
            .exchange_at(&handle, &endpoint, frame)
            .await
            .unwrap(),
        response
    );
    assert_eq!(*store.uploaded.lock().unwrap(), [bytes.to_vec()]);
    let requests = runtime.await.unwrap();
    assert!(
        request_text(requests[0].clone()).starts_with("POST /proxy/3001/runtime/control HTTP/1.1")
    );
    assert!(request_text(requests[1].clone()).starts_with(&format!(
        "GET /proxy/3001/runtime/checkpoints/{} HTTP/1.1",
        receipt.checkpoint.object_reference,
    )));
}

#[tokio::test]
async fn hold_verification_rejects_uncorrelated_receipt_and_recovers_the_requested_token() {
    let (request, mut receipt) = held(b"already published archive");
    receipt.checkpoint.epoch += 1;
    let runtime = listener().await;
    let provider = listener().await;
    let client = SandboxClient::new(
        format!("http://{}", provider.local_addr().unwrap()),
        "provider-secret",
    );
    let provider = spawn_http(
        provider,
        json_response(&serde_json::json!({
            "endpoint": format!("{}/proxy/3001", runtime.local_addr().unwrap()),
        })),
    );
    let resume = crate::CheckpointWriterResumeReceipt {
        writer_resume_token: request.writer_resume_token.clone(),
        writers_resumed: true,
    };
    let responses = [
        json_response(&ResponseFrame::CheckpointForTeardown(receipt)),
        json_response(&ResponseFrame::ResumeCheckpointWriters(resume)),
    ];
    let runtime = tokio::spawn(async move {
        let mut requests = Vec::new();
        for response in responses {
            let (mut stream, _) = runtime.accept().await.unwrap();
            requests.push(read_request(&mut stream).await);
            stream.write_all(&response).await.unwrap();
            stream.shutdown().await.unwrap();
        }
        requests
    });
    let transport =
        ProviderHostedControlTransport::new(client, Arc::new(RecordingFactory::default())).unwrap();
    let handle = RuntimeHandle::new("runtime-1");
    transport.bind_runtime(&handle, "sandbox-a").await.unwrap();
    assert!(matches!(
        transport.verify_checkpoint_hold(&handle, &request).await,
        Err(AdapterError::Rejected(detail)) if detail.contains("uncorrelated"),
    ));
    provider.await.unwrap();
    let requests = runtime.await.unwrap();
    let recovery = request_text(requests[1].clone());
    assert_eq!(
        serde_json::from_str::<RequestFrame>(recovery.split_once("\r\n\r\n").unwrap().1).unwrap(),
        RequestFrame::ResumeCheckpointWriters(crate::CheckpointWriterResumeRequest {
            writer_resume_token: request.writer_resume_token,
        }),
    );
}
