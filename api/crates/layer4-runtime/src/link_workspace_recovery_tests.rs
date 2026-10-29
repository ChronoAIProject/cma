use super::*;
use crate::config::RuntimeConfig;
use crate::workspace_journal::{self, Begin};
use layer3_adapter::{
    RuntimeStatus, RuntimeStatusRequest, WorkspacePrepareRecoveryFrame,
    WorkspacePrepareRecoveryRequest, WorkspaceReceipt,
};
use std::os::unix::fs::MetadataExt;

async fn runtime_status(
    client: &reqwest::Client,
    address: SocketAddr,
    config: &RuntimeConfig,
) -> RuntimeStatus {
    let request = RuntimeStatusRequest {
        sandbox_id: config.sandbox_id.clone(),
        generation: config.generation,
        home_identity: config.home_identity.clone().unwrap(),
    };
    let response = exchange(
        client,
        address,
        &RequestFrame::RuntimeStatus(request.clone()),
    )
    .await;
    let ResponseFrame::RuntimeStatus(status) = response else {
        panic!("{response:?}");
    };
    status.validate_for(&request).unwrap();
    status
}

async fn recover(
    client: &reqwest::Client,
    address: SocketAddr,
    frame: &WorkspacePrepareRecoveryFrame,
) -> ResponseFrame {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let response = exchange(
            client,
            address,
            &RequestFrame::RecoverWorkspacePrepare(frame.clone()),
        )
        .await;
        // Other parallel Git fixtures can briefly inherit a writer descriptor.
        // Retry only the typed no-effect contention result with this exact frame.
        if !matches!(&response, ResponseFrame::Failure(failure)
            if failure.code == "workspace_removal_writer_busy" && failure.retryable)
            || std::time::Instant::now() >= deadline
        {
            return response;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn workspace_prepare_recovery_over_http_preserves_original_and_fences_stale_boots() {
    let config = RuntimeConfig {
        home_identity: Some("a".repeat(43)),
        ..RuntimeConfig::for_test(root("workspace-prepare-recovery"))
    };
    std::fs::create_dir_all(&config.home).unwrap();
    let old = Supervisor::new(
        config.clone(),
        crate::writer_fence::SupervisorBoundary::default(),
    )
    .unwrap();
    let (old_address, old_server) = serve(old).await;
    let client = reqwest::Client::new();
    let installed = exchange(
        &client,
        old_address,
        &RequestFrame::Install(InstallRequest::new("install-recovery-http", Vec::new())),
    )
    .await;
    assert!(matches!(installed, ResponseFrame::Install(_)));
    assert!(matches!(
        exchange(&client, old_address, &credential()).await,
        ResponseFrame::Credential(_)
    ));
    let original = WorkspaceRequest::new(
        "workspace:wks_http_recovery:prepare:v1",
        "wks_http_recovery",
        "project",
        WorkspaceSource::Init,
    );
    let Begin::New(ticket) = workspace_journal::begin::<WorkspaceReceipt, _>(
        &config.state_root,
        &config.home,
        config.allow_local_git,
        &config.sandbox_id,
        &original.workspace_id,
        "prepare",
        &original.operation_id,
        &original.request_sha256,
        &original,
    )
    .unwrap() else {
        panic!("expected original preparation admission");
    };
    ticket
        .indeterminate("original preparation stopped before publication")
        .unwrap();
    let journal = config.state_root.join("workspace-operations").join(format!(
        "{:x}.json",
        Sha256::digest(original.operation_id.as_bytes())
    ));
    let original_journal = std::fs::read(&journal).unwrap();
    let staged = config
        .home
        .join(".cma/preparing")
        .join(&original.workspace_id);
    std::fs::create_dir_all(staged.join(".git/info")).unwrap();
    std::fs::write(staged.join("retained-work"), b"original checkout bytes").unwrap();
    let original_inode = staged.metadata().unwrap().ino();
    let request = WorkspacePrepareRecoveryRequest::new("recovery-http", original.clone(), None);
    let old_frame = WorkspacePrepareRecoveryFrame {
        runtime: runtime_status(&client, old_address, &config).await,
        request: request.clone(),
    };

    // A replacement boot owns the retained home. Exercise both old HTTP
    // connections and delayed requests against the actual transport boundary.
    let current = Supervisor::new(
        config.clone(),
        crate::writer_fence::SupervisorBoundary::default(),
    )
    .unwrap();
    let (address, server) = serve(current).await;
    assert!(matches!(
        exchange(&client, address, &credential()).await,
        ResponseFrame::Credential(_)
    ));
    for (endpoint, code) in [
        (old_address, "workspace_removal_writer_stale"),
        (address, "workspace_recovery_authority"),
    ] {
        let response = recover(&client, endpoint, &old_frame).await;
        let failure = response
            .failure()
            .expect("stale writer must be rejected over HTTP");
        assert_eq!(failure.operation, WireOperation::RecoverWorkspacePrepare);
        assert_eq!(failure.code, code);
        assert!(!failure.retryable);
        assert_eq!(staged.metadata().unwrap().ino(), original_inode);
    }
    let frame = WorkspacePrepareRecoveryFrame {
        runtime: runtime_status(&client, address, &config).await,
        request,
    };
    assert_ne!(frame.runtime.boot_id, old_frame.runtime.boot_id);
    let response = recover(&client, address, &frame).await;
    let ResponseFrame::RecoverWorkspacePrepare(receipt) = response else {
        panic!("expected recovery through HTTP, got {response:?}");
    };
    assert_eq!(receipt.prepared.operation_id, frame.request.operation_id);
    assert_eq!(
        receipt.prepared.request_sha256,
        frame.request.request_sha256
    );
    assert_eq!(receipt.prepared.workspace_id, original.workspace_id);
    assert_eq!(receipt.prepared.name, original.name);
    let preserved = PathBuf::from(&receipt.preserved_path);
    assert_eq!(preserved.metadata().unwrap().ino(), original_inode);
    assert_eq!(
        std::fs::read(preserved.join("retained-work")).unwrap(),
        b"original checkout bytes"
    );
    assert!(!staged.exists());
    assert!(!preserved.join(".git/info/cma-workspace-id").exists());
    assert_eq!(std::fs::read(&journal).unwrap(), original_journal);
    let published = config.home.join(&original.name);
    assert_eq!(
        std::fs::read_to_string(published.join(".git/info/cma-workspace-id")).unwrap(),
        original.workspace_id
    );
    let published_inode = published.metadata().unwrap().ino();
    assert_eq!(
        recover(&client, address, &frame).await,
        ResponseFrame::RecoverWorkspacePrepare(receipt)
    );
    assert_eq!(published.metadata().unwrap().ino(), published_inode);
    assert_eq!(std::fs::read(&journal).unwrap(), original_journal);
    server.abort();
    old_server.abort();
    let _ = tokio::join!(server, old_server);
    for path in [&config.home, &config.state_root, &config.checkpoint_root] {
        let _ = std::fs::remove_dir_all(path);
    }
}
