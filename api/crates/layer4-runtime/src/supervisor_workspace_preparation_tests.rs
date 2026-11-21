use super::*;
use crate::workspace_journal::{self, Begin};
use layer3_adapter::WorkspaceReceipt;

fn request(source: WorkspaceSource) -> WorkspaceRequest {
    WorkspaceRequest::new(
        "workspace:wks_pending:prepare:v1",
        "wks_pending",
        "project",
        source,
    )
}

fn clone_request() -> WorkspaceRequest {
    request(WorkspaceSource::Clone {
        url: "https://github.com/example/project".into(),
        reference: Some("main".into()),
    })
}

fn journal(fixture: &Fixture, request: &WorkspaceRequest) -> Begin<WorkspaceReceipt> {
    workspace_journal::begin(
        &fixture.config.state_root,
        &fixture.config.home,
        fixture.config.allow_local_git,
        &fixture.config.sandbox_id,
        &request.workspace_id,
        "prepare",
        &request.operation_id,
        &request.request_sha256,
        request,
    )
    .unwrap()
}

fn interrupted_clone(fixture: &Fixture) -> PathBuf {
    let staged = fixture.config.home.join(".cma/preparing/wks_pending");
    std::fs::create_dir_all(staged.join(".git/info")).unwrap();
    std::fs::write(staged.join(".git/index.lock"), b"unfinished index").unwrap();
    std::fs::write(staged.join("retained-file"), b"retained checkout contents").unwrap();
    staged
}

async fn startup(fixture: &Fixture) -> std::sync::Arc<Supervisor> {
    Supervisor::start(fixture.config.clone(), SupervisorBoundary::default())
        .await
        .unwrap()
}

async fn startup_error(fixture: &Fixture) -> crate::failure::RuntimeError {
    match Supervisor::start(fixture.config.clone(), SupervisorBoundary::default()).await {
        Ok(_) => panic!("startup accepted unresolved workspace I/O"),
        Err(error) => error,
    }
}

#[tokio::test]
async fn interrupted_clone_settles_without_losing_files_or_inventing_runtime_readiness() {
    for installed in [false, true] {
        let fixture = Fixture::new();
        let original = fixture.supervisor();
        if installed {
            assert!(matches!(
                original
                    .dispatch(RequestFrame::Install(InstallRequest::new(
                        "install-before-clone",
                        vec![],
                    )))
                    .await,
                ResponseFrame::Install(_)
            ));
        }
        let request = clone_request();
        assert!(matches!(journal(&fixture, &request), Begin::New(_)));
        let staged = interrupted_clone(&fixture);
        drop(original);

        // Both the first recovery and a subsequent boot must serve the actual
        // installation state while retaining the terminal workspace outcome.
        for _ in 0..2 {
            let restarted = startup(&fixture).await;
            let observed = status(restarted.dispatch(fixture.status()).await);
            assert_eq!(observed.installation_ready, installed);
            assert!(!observed.credential_ready && !observed.restore_pending);
            assert_eq!(observed.agent_processes, 0);
            let Begin::Indeterminate(error) = journal(&fixture, &request) else {
                panic!("partial clone did not retain its indeterminate outcome");
            };
            assert_eq!(error.code, "workspace_prepare_outcome_unknown");
            assert!(!error.retryable);
            assert!(!fixture.config.home.join("project").exists());
            assert!(!staged.join(".git/info/cma-workspace-id").exists());
            assert_eq!(
                std::fs::read(staged.join(".git/index.lock")).unwrap(),
                b"unfinished index"
            );
            assert_eq!(
                std::fs::read(staged.join("retained-file")).unwrap(),
                b"retained checkout contents"
            );
            if installed {
                assert!(matches!(
                    restarted.dispatch(credential()).await,
                    ResponseFrame::Credential(_)
                ));
                assert_eq!(
                    failure(
                        restarted
                            .dispatch(RequestFrame::PrepareWorkspace(request.clone()))
                            .await
                    ),
                    "workspace_prepare_outcome_unknown"
                );
                let mut changed = request.clone();
                changed.name = "different-project".into();
                changed.request_sha256 = changed.computed_sha256();
                assert_eq!(
                    failure(
                        restarted
                            .dispatch(RequestFrame::PrepareWorkspace(changed))
                            .await
                    ),
                    "workspace_replay_mismatch"
                );
            }
        }
    }
}

#[tokio::test]
async fn completed_staging_with_matching_identity_publishes_exact_receipt() {
    let fixture = Fixture::new();
    drop(fixture.supervisor());
    let request = request(WorkspaceSource::Init);
    assert!(matches!(journal(&fixture, &request), Begin::New(_)));
    let staging_root = fixture.config.home.join(".cma/preparing");
    let mut staged_request = request.clone();
    staged_request.name.clone_from(&request.workspace_id);
    let prepared = crate::workspace_source::prepare(&staging_root, true, &staged_request)
        .await
        .unwrap();
    std::fs::write(
        staging_root.join("wks_pending/retained-file"),
        b"completed checkout contents",
    )
    .unwrap();

    let _restarted = startup(&fixture).await;
    let Begin::Completed(receipt) = journal(&fixture, &request) else {
        panic!("completed clone did not recover its receipt");
    };
    assert_eq!(receipt.operation_id, request.operation_id);
    assert_eq!(receipt.request_sha256, request.request_sha256);
    assert_eq!(receipt.workspace_id, request.workspace_id);
    assert_eq!(receipt.name, request.name);
    assert_eq!(receipt.resolved_commit, prepared.resolved_commit);
    assert_eq!(
        receipt.path,
        fixture.config.home.join("project").to_string_lossy()
    );
    assert!(!staging_root.join("wks_pending").exists());
    assert_eq!(
        std::fs::read(fixture.config.home.join("project/retained-file")).unwrap(),
        b"completed checkout contents"
    );
    assert_eq!(
        std::fs::read(
            fixture
                .config
                .home
                .join("project/.git/info/cma-workspace-id")
        )
        .unwrap(),
        b"wks_pending"
    );
}

#[tokio::test]
async fn wrong_staged_identity_remains_a_terminal_mismatch_without_publication() {
    let fixture = Fixture::new();
    drop(fixture.supervisor());
    let request = clone_request();
    assert!(matches!(journal(&fixture, &request), Begin::New(_)));
    let staged = interrupted_clone(&fixture);
    let marker = staged.join(".git/info/cma-workspace-id");
    std::fs::write(&marker, b"wks_successor").unwrap();

    let _restarted = startup(&fixture).await;
    let Begin::Failed(error) = journal(&fixture, &request) else {
        panic!("foreign identity did not preserve its terminal mismatch");
    };
    assert_eq!(error.code, "workspace_incarnation_mismatch");
    assert!(!error.retryable);
    assert_eq!(std::fs::read(marker).unwrap(), b"wks_successor");
    assert!(staged.join("retained-file").is_file());
    assert!(!fixture.config.home.join("project").exists());
}

#[tokio::test]
async fn real_staged_identity_io_errors_leave_preparation_pending_and_block_startup() {
    for unreadable in ["marker-directory", "info-file", "dangling-marker"] {
        let fixture = Fixture::new();
        drop(fixture.supervisor());
        let request = clone_request();
        assert!(matches!(journal(&fixture, &request), Begin::New(_)));
        let staged = interrupted_clone(&fixture);
        let marker = staged.join(".git/info/cma-workspace-id");
        match unreadable {
            "marker-directory" => std::fs::create_dir(&marker).unwrap(),
            "info-file" => {
                std::fs::remove_dir(staged.join(".git/info")).unwrap();
                std::fs::write(staged.join(".git/info"), b"preserve invalid entry").unwrap();
            }
            "dangling-marker" => {
                std::os::unix::fs::symlink("absent-identity", &marker).unwrap();
            }
            _ => unreachable!(),
        }
        let error = startup_error(&fixture).await;
        assert_eq!(error.code, "workspace_io_failed", "{unreadable}");
        assert!(error.retryable, "{unreadable}");
        assert!(matches!(journal(&fixture, &request), Begin::Started(_)));
        assert!(staged.join("retained-file").is_file());
        assert!(!fixture.config.home.join("project").exists());
    }
}

#[tokio::test]
async fn missing_published_identity_does_not_gain_staging_recovery_authority() {
    let fixture = Fixture::new();
    drop(fixture.supervisor());
    let request = clone_request();
    assert!(matches!(journal(&fixture, &request), Begin::New(_)));
    let published = fixture.config.home.join("project");
    std::fs::create_dir_all(published.join(".git/info")).unwrap();
    std::fs::write(published.join("retained-file"), b"published contents").unwrap();

    let error = startup_error(&fixture).await;
    assert_eq!(error.code, "workspace_io_failed");
    assert!(error.retryable);
    assert!(matches!(journal(&fixture, &request), Begin::Started(_)));
    assert_eq!(
        std::fs::read(published.join("retained-file")).unwrap(),
        b"published contents"
    );
}
