use sha2::{Digest as _, Sha256};

use super::*;
use crate::wire::Acknowledgement;
use crate::{RestoreBootstrapReceipt, RestoreBootstrapRequest, RestoreRequest};

fn restore_request(bytes: &[u8]) -> RestoreRequest {
    let sha256 = format!("{:x}", Sha256::digest(bytes));
    RestoreRequest {
        object_reference: format!("home-{sha256}.cma"),
        sha256,
    }
}

#[tokio::test]
async fn bootstrap_admits_exact_identity_without_archive_access() {
    let request = RestoreBootstrapRequest::new(
        "sandbox:sandbox-a:bootstrap_restore",
        "sandbox-a",
        restore_request(b"not downloaded during bootstrap"),
    );
    let response = ResponseFrame::BootstrapRestore(RestoreBootstrapReceipt {
        operation_id: request.operation_id.clone(),
        request_sha256: request.request_sha256.clone(),
        restore_request_sha256: request.checkpoint.computed_sha256(),
        installation_inherited: true,
    });
    let (transport, provider, runtime) = transport_fixture(json_response(&response)).await;
    let handle = RuntimeHandle::new("runtime-1");
    transport.bind_runtime(&handle, "sandbox-a").await.unwrap();
    let frame = RequestFrame::BootstrapRestore(request);

    let actual = transport
        .exchange(&handle, frame.clone())
        .await
        .expect("bootstrap must succeed with the archive store unavailable");
    assert_eq!(actual, response);
    provider.await.unwrap();
    let runtime = request_text(runtime.await.unwrap());
    assert!(runtime.starts_with("POST /proxy/3001/runtime/control HTTP/1.1"));
    assert_eq!(
        serde_json::from_str::<RequestFrame>(runtime.split_once("\r\n\r\n").unwrap().1).unwrap(),
        frame
    );
}

struct RestoreStore {
    identity: RestoreRequest,
    bytes: Vec<u8>,
    downloads: StdMutex<Vec<String>>,
}

impl RestoreStore {
    fn new(bytes: &[u8]) -> Self {
        Self {
            identity: restore_request(bytes),
            bytes: bytes.to_vec(),
            downloads: StdMutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl CheckpointArchiveStore for RestoreStore {
    async fn upload(
        &self,
        _archive: CheckpointArchiveUpload,
    ) -> crate::Result<CheckpointArchiveReceipt> {
        panic!("restore must not upload to the durable archive store")
    }

    async fn download(&self, reference: &str) -> crate::Result<CheckpointArchiveDownload> {
        self.downloads.lock().unwrap().push(reference.into());
        let (sender, chunks) = tokio::sync::mpsc::channel(1);
        sender.try_send(Ok(self.bytes.clone())).unwrap();
        Ok(CheckpointArchiveDownload {
            object_reference: self.identity.object_reference.clone(),
            sha256: self.identity.sha256.clone(),
            size_bytes: self.bytes.len() as u64,
            chunks,
        })
    }
}

struct RestoreFactory(Arc<RestoreStore>);

impl CheckpointArchiveStoreFactory for RestoreFactory {
    fn for_sandbox(&self, sandbox_id: &str) -> crate::Result<Arc<dyn CheckpointArchiveStore>> {
        assert_eq!(sandbox_id, "sandbox-a");
        Ok(self.0.clone())
    }
}

async fn transport(store: Arc<RestoreStore>) -> ProviderHostedControlTransport {
    let transport = ProviderHostedControlTransport::new(
        SandboxClient::new("http://127.0.0.1:1", "provider-secret"),
        Arc::new(RestoreFactory(store)),
    )
    .unwrap();
    transport
        .bind_runtime(&RuntimeHandle::new("runtime-1"), "sandbox-a")
        .await
        .unwrap();
    transport
}

fn endpoint(address: std::net::SocketAddr) -> RuntimeEndpoint {
    RuntimeEndpoint::parse("http://127.0.0.1:1", &format!("{address}/proxy/3001")).unwrap()
}

#[tokio::test]
async fn restore_imports_exact_archive_before_control_dispatch() {
    let store = Arc::new(RestoreStore::new(b"sealed archive"));
    let request = store.identity.clone();
    let transport = transport(Arc::clone(&store)).await;
    let runtime = listener().await;
    let endpoint = endpoint(runtime.local_addr().unwrap());
    let runtime = tokio::spawn(async move {
        let mut requests = Vec::new();
        for response in [
            http_response("204 No Content", &[("Content-Length", "0".into())], &[]),
            json_response(&ResponseFrame::Restore(Acknowledgement::default())),
        ] {
            let (mut stream, _) = runtime.accept().await.unwrap();
            requests.push(read_request(&mut stream).await);
            stream.write_all(&response).await.unwrap();
            stream.shutdown().await.unwrap();
        }
        requests
    });

    let frame = RequestFrame::Restore(request.clone());
    let actual = transport
        .exchange_at(&RuntimeHandle::new("runtime-1"), &endpoint, frame.clone())
        .await
        .unwrap();
    assert_eq!(actual, ResponseFrame::Restore(Acknowledgement::default()));
    assert_eq!(
        store.downloads.lock().unwrap().as_slice(),
        std::slice::from_ref(&request.object_reference)
    );
    let requests = runtime.await.unwrap();
    let upload = request_text(requests[0].clone());
    assert!(upload.starts_with(&format!(
        "PUT /proxy/3001/runtime/checkpoints/{} HTTP/1.1",
        request.object_reference
    )));
    assert_eq!(
        upload.split_once("\r\n\r\n").unwrap().1.as_bytes(),
        store.bytes
    );
    let control = request_text(requests[1].clone());
    assert!(control.starts_with("POST /proxy/3001/runtime/control HTTP/1.1"));
    assert_eq!(
        serde_json::from_str::<RequestFrame>(control.split_once("\r\n\r\n").unwrap().1).unwrap(),
        frame
    );
}

#[tokio::test]
async fn restore_refuses_mismatched_archive_before_runtime_access() {
    for mismatch_reference in [false, true] {
        let mut store = RestoreStore::new(b"sealed archive");
        let request = store.identity.clone();
        if mismatch_reference {
            store.identity.object_reference = restore_request(b"other archive").object_reference;
        } else {
            store.identity.sha256 = restore_request(b"other archive").sha256;
        }
        let store = Arc::new(store);
        let transport = transport(Arc::clone(&store)).await;
        let runtime = listener().await;
        let endpoint = endpoint(runtime.local_addr().unwrap());

        let error = transport
            .exchange_at(
                &RuntimeHandle::new("runtime-1"),
                &endpoint,
                RequestFrame::Restore(request.clone()),
            )
            .await
            .expect_err("different archive identity must not reach the runtime");
        assert!(
            matches!(error, AdapterError::Rejected(detail) if detail.contains("different or invalid archive identity"))
        );
        assert_eq!(*store.downloads.lock().unwrap(), [request.object_reference]);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), runtime.accept())
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn restore_refuses_corrupt_stream_even_when_upload_is_acknowledged() {
    let mut store = RestoreStore::new(b"sealed archive");
    let request = store.identity.clone();
    store.bytes[0] ^= 1;
    let store = Arc::new(store);
    let transport = transport(Arc::clone(&store)).await;
    let runtime = listener().await;
    let endpoint = endpoint(runtime.local_addr().unwrap());
    let runtime = spawn_http(
        runtime,
        http_response("204 No Content", &[("Content-Length", "0".into())], &[]),
    );

    let error = transport
        .exchange_at(
            &RuntimeHandle::new("runtime-1"),
            &endpoint,
            RequestFrame::Restore(request.clone()),
        )
        .await
        .expect_err("corrupt archive must not dispatch Restore control");
    assert!(
        matches!(error, AdapterError::Rejected(detail) if detail.contains("stream did not match its declared identity"))
    );
    assert_eq!(*store.downloads.lock().unwrap(), [request.object_reference]);
    assert!(
        request_text(runtime.await.unwrap()).starts_with("PUT /proxy/3001/runtime/checkpoints/")
    );
}
