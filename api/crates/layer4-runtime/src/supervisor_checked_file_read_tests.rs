use super::*;

#[tokio::test]
async fn a_checked_file_read_proves_the_live_home_and_preserves_byte_bounds() {
    let fixture = Fixture::new();
    let supervisor = fixture.supervisor();
    assert!(matches!(
        supervisor
            .dispatch(RequestFrame::Install(InstallRequest::new(
                "file-read-install",
                vec![]
            )))
            .await,
        ResponseFrame::Install(_)
    ));
    assert!(matches!(
        supervisor.dispatch(credential()).await,
        ResponseFrame::Credential(_)
    ));
    let ready = status(supervisor.dispatch(fixture.status()).await);
    let mut request = layer3_adapter::AgentFileReadRequest {
        owner_access: true,
        protected_paths: Vec::new(),
        sandbox_id: ready.sandbox_id,
        runtime_generation: ready.generation,
        boot_id: ready.boot_id,
        home_identity: Some(ready.home_identity),
        path: "preview.bin".into(),
        max_bytes: 4,
    };
    let bytes = [0, 127, 128, 255];
    std::fs::write(fixture.config.home.join(&request.path), bytes).unwrap();
    let ResponseFrame::ReadAgentFile(receipt) = supervisor
        .dispatch(RequestFrame::ReadAgentFile(request.clone()))
        .await
    else {
        panic!("ready bound Runtime must serve the checked read");
    };
    assert_eq!(receipt.content().unwrap(), bytes);
    assert_eq!(receipt.path, request.path);
    request.max_bytes = 3;
    assert_eq!(
        failure(
            supervisor
                .dispatch(RequestFrame::ReadAgentFile(request.clone()))
                .await
        ),
        "file_too_large"
    );
    request.max_bytes = 4;
    let mut escaped = request.clone();
    escaped.path = "../preview.bin".into();
    assert!(matches!(
        supervisor
            .dispatch(RequestFrame::ReadAgentFile(escaped))
            .await,
        ResponseFrame::Failure(_)
    ));
    let mut other_home = request.clone();
    other_home.home_identity = Some("b".repeat(43));
    assert_eq!(
        failure(
            supervisor
                .dispatch(RequestFrame::ReadAgentFile(other_home))
                .await
        ),
        "runtime_status_identity_invalid"
    );
    request.boot_id = "f".repeat(32);
    assert_eq!(
        failure(
            supervisor
                .dispatch(RequestFrame::ReadAgentFile(request))
                .await
        ),
        "filesystem_identity"
    );
}
