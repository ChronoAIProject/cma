use super::*;
use layer3_adapter::{
    WorkspacePrepareRecoveryFrame, WorkspacePrepareRecoveryRequest, WorkspaceReceipt,
};

async fn ready(fixture: &Fixture) -> std::sync::Arc<Supervisor> {
    let supervisor = fixture.supervisor();
    assert!(matches!(
        supervisor
            .dispatch(RequestFrame::Install(InstallRequest::new(
                "install-recovery",
                vec![]
            )))
            .await,
        ResponseFrame::Install(_)
    ));
    assert!(matches!(
        supervisor.dispatch(credential()).await,
        ResponseFrame::Credential(_)
    ));
    supervisor
}

fn ambiguous(fixture: &Fixture) -> WorkspacePrepareRecoveryRequest {
    let request = WorkspaceRequest::new(
        "workspace:wks_recovery:prepare:v1",
        "wks_recovery",
        "project",
        WorkspaceSource::Init,
    );
    let crate::workspace_journal::Begin::New(ticket) =
        crate::workspace_journal::begin::<WorkspaceReceipt, _>(
            &fixture.config.state_root,
            &fixture.config.home,
            fixture.config.allow_local_git,
            &fixture.config.sandbox_id,
            &request.workspace_id,
            "prepare",
            &request.operation_id,
            &request.request_sha256,
            &request,
        )
        .unwrap()
    else {
        panic!("expected original admission");
    };
    ticket
        .indeterminate("original preparation never completed its identity marker")
        .unwrap();
    let staged = fixture.config.home.join(".cma/preparing/wks_recovery");
    std::fs::create_dir_all(staged.join(".git/info")).unwrap();
    std::fs::write(staged.join("original"), "preserve original").unwrap();
    WorkspacePrepareRecoveryRequest::new("explicit-recovery", request, None)
}

async fn recovery_frame(
    fixture: &Fixture,
    supervisor: &Supervisor,
    request: WorkspacePrepareRecoveryRequest,
) -> RequestFrame {
    RequestFrame::RecoverWorkspacePrepare(WorkspacePrepareRecoveryFrame {
        runtime: status(supervisor.dispatch(fixture.status()).await),
        request,
    })
}

#[tokio::test]
async fn preserving_recovery_fences_old_boot_delayed_prepare_and_exclusive_writer() {
    let fixture = Fixture::new();
    let old = ready(&fixture).await;
    let request = ambiguous(&fixture);
    let old_frame = recovery_frame(&fixture, &old, request.clone()).await;
    let current = ready(&fixture).await;
    assert_eq!(
        failure(current.dispatch(old_frame).await),
        "workspace_recovery_authority"
    );
    assert_eq!(
        failure(
            old.dispatch(RequestFrame::PrepareWorkspace(request.original.clone()))
                .await
        ),
        "workspace_removal_writer_stale"
    );
    let frame = recovery_frame(&fixture, &current, request).await;
    let held = current.removal_writer_guard().unwrap();
    assert_eq!(
        failure(current.dispatch(frame.clone()).await),
        "workspace_removal_writer_busy"
    );
    assert_eq!(
        failure(
            current
                .dispatch(RequestFrame::PrepareWorkspace(WorkspaceRequest::new(
                    "workspace:wks_delayed:prepare:v1",
                    "wks_delayed",
                    "delayed",
                    WorkspaceSource::Init,
                )))
                .await
        ),
        "workspace_removal_writer_busy"
    );
    assert!(!fixture.config.home.join("delayed").exists());
    drop(held);
    assert!(fixture
        .config
        .home
        .join(".cma/preparing/wks_recovery/original")
        .exists());
    let response = current.dispatch(frame).await;
    assert!(
        matches!(response, ResponseFrame::RecoverWorkspacePrepare(_)),
        "{response:?}"
    );
}

#[tokio::test]
async fn preserving_recovery_requires_live_credentials_and_correct_home_generation() {
    let fixture = Fixture::new();
    let supervisor = ready(&fixture).await;
    let request = ambiguous(&fixture);
    let frame = recovery_frame(&fixture, &supervisor, request.clone()).await;
    for mutation in ["generation", "home", "boot", "credential"] {
        let RequestFrame::RecoverWorkspacePrepare(mut changed) = frame.clone() else {
            unreachable!();
        };
        match mutation {
            "generation" => changed.runtime.generation += 1,
            "home" => changed.runtime.home_identity = "b".repeat(43),
            "boot" => changed.runtime.boot_id = uuid::Uuid::new_v4().to_string(),
            "credential" => changed.runtime.credential_ready = false,
            _ => unreachable!(),
        }
        assert!(matches!(
            supervisor
                .dispatch(RequestFrame::RecoverWorkspacePrepare(changed))
                .await,
            ResponseFrame::Failure(_)
        ));
        assert!(fixture
            .config
            .home
            .join(".cma/preparing/wks_recovery/original")
            .exists());
    }
    let response = supervisor.dispatch(frame).await;
    assert!(
        matches!(response, ResponseFrame::RecoverWorkspacePrepare(_)),
        "{response:?}"
    );
}

#[tokio::test]
async fn preserving_recovery_survives_caller_disconnect_during_real_git_preparation() {
    let fixture = Fixture::new();
    let supervisor = ready(&fixture).await;
    let request = ambiguous(&fixture);
    let frame = recovery_frame(&fixture, &supervisor, request.clone()).await;
    let task = {
        let supervisor = std::sync::Arc::clone(&supervisor);
        let frame = frame.clone();
        tokio::spawn(async move { supervisor.dispatch(frame).await })
    };
    let attempts = fixture
        .config
        .state_root
        .join("workspace-prepare-recovery/wks_recovery/attempts");
    let mut started = false;
    for _ in 0..200 {
        if std::fs::read_dir(&attempts).is_ok_and(|mut entries| {
            entries.any(|entry| entry.is_ok_and(|entry| entry.path().join("started.json").exists()))
        }) {
            started = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert!(started, "recovery never durably started");
    assert!(
        !task.is_finished(),
        "caller must disconnect while real preparation is still running"
    );
    task.abort();
    let response = supervisor.dispatch(frame).await;
    assert!(
        matches!(response, ResponseFrame::RecoverWorkspacePrepare(_)),
        "{response:?}"
    );
    assert!(fixture
        .config
        .home
        .join("project/.git/info/cma-workspace-id")
        .exists());
    assert_eq!(
        std::fs::read_to_string(
            fixture
                .config
                .home
                .join(".cma/prepare-recovery/wks_recovery/original/original")
        )
        .unwrap(),
        "preserve original"
    );
}
