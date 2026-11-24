use std::path::{Path, PathBuf};

use layer3_adapter::wire::{RequestFrame, ResponseFrame};
use layer3_adapter::{
    CredentialFrame, InstallRequest, RestoreBootstrapRequest, RestoreRequest, RuntimeCredential,
    WorkspaceRequest, WorkspaceSource,
};

use super::*;

fn root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "cma-supervisor-restore-{}-{name}",
        std::process::id()
    ))
}

fn config(name: &str) -> RuntimeConfig {
    let home = root(name);
    cleanup(&home);
    RuntimeConfig::for_test(home)
}

fn cleanup(home: &Path) {
    let _ = std::fs::remove_dir_all(home);
    let _ = std::fs::remove_dir_all(home.with_extension("checkpoints"));
    let _ = std::fs::remove_dir_all(home.with_extension("runtime-state"));
    let _ = std::fs::remove_dir_all(home.with_extension("source"));
}

fn sealed_restore(config: &RuntimeConfig) -> RestoreRequest {
    let source = config.home.with_extension("source");
    std::fs::create_dir_all(source.join(".local/bin")).unwrap();
    std::fs::write(source.join(".local/bin/inherited-tool"), "installed\n").unwrap();
    std::fs::write(source.join("dirty-worktree-file"), "uncommitted\n").unwrap();
    let sealed = crate::checkpoint_archive::seal(&source, &config.checkpoint_root, &[]).unwrap();
    RestoreRequest {
        object_reference: sealed.reference,
        sha256: sealed.sha256,
    }
}

fn bootstrap(restore: RestoreRequest) -> RestoreBootstrapRequest {
    RestoreBootstrapRequest::new(
        "sandbox:sandbox-test:restore-bootstrap:v1",
        "sandbox-test",
        restore,
    )
}

fn credential() -> RequestFrame {
    RequestFrame::Credential(CredentialFrame {
        operation_id: "replacement-credential".into(),
        value: RuntimeCredential::new("secret"),
        custody_generation: Some(2),
        github_route: None,
    })
}

fn failure(response: ResponseFrame) -> layer3_adapter::wire::WireFailure {
    match response {
        ResponseFrame::Failure(failure) => failure,
        other => panic!("expected failure, got {other:?}"),
    }
}

fn workspace() -> RequestFrame {
    RequestFrame::PrepareWorkspace(WorkspaceRequest::new(
        "workspace:wks-after-restore:prepare:v1",
        "wks-after-restore",
        "project",
        WorkspaceSource::Init,
    ))
}

#[tokio::test]
async fn bootstrap_requires_credential_and_exact_restore_again_after_restart() {
    let config = config("restart");
    let restore = sealed_restore(&config);
    let archived = std::fs::read(config.checkpoint_root.join(&restore.object_reference)).unwrap();
    std::fs::create_dir_all(&config.home).unwrap();
    std::fs::write(config.home.join("replace-me"), "new runtime\n").unwrap();
    let supervisor = Supervisor::new(config.clone(), SupervisorBoundary::default()).unwrap();
    let request = bootstrap(restore.clone());

    let first = supervisor
        .dispatch(RequestFrame::BootstrapRestore(request.clone()))
        .await;
    let replay = supervisor
        .dispatch(RequestFrame::BootstrapRestore(request.clone()))
        .await;
    assert_eq!(first, replay);
    assert_eq!(supervisor.phase().await, Phase::RestorePending);

    let state = std::fs::read_to_string(
        config
            .checkpoint_root
            .join("runtime-state")
            .join("install-v2.json"),
    )
    .unwrap();
    assert!(!state.contains(&restore.object_reference));
    assert!(state.contains("request_sha256"));
    assert!(state.contains("restore_request_sha256"));

    let install = failure(
        supervisor
            .dispatch(RequestFrame::Install(InstallRequest::new(
                "must-not-install",
                vec!["touch must-not-run".into()],
            )))
            .await,
    );
    assert_eq!(install.code, "install_mode_conflict");
    assert!(!config.home.join("must-not-run").exists());

    let too_early = failure(
        supervisor
            .dispatch(RequestFrame::Restore(restore.clone()))
            .await,
    );
    assert_eq!(too_early.code, "credential_not_installed");
    assert!(matches!(
        supervisor.dispatch(credential()).await,
        ResponseFrame::Credential(_)
    ));
    assert_eq!(supervisor.phase().await, Phase::RestoreCredentialed);
    let blocked_work = failure(supervisor.dispatch(workspace()).await);
    assert_eq!(blocked_work.code, "restore_not_completed");

    let changed_digest = "b".repeat(64);
    let changed = RestoreRequest {
        object_reference: format!("home-{changed_digest}.cma"),
        sha256: changed_digest,
    };
    let mismatch = failure(supervisor.dispatch(RequestFrame::Restore(changed)).await);
    assert_eq!(mismatch.code, "restore_bootstrap_conflict");
    assert!(matches!(
        supervisor
            .dispatch(RequestFrame::Restore(restore.clone()))
            .await,
        ResponseFrame::Restore(_)
    ));
    assert_eq!(supervisor.phase().await, Phase::Credentialed);
    assert!(!config.home.join("replace-me").exists());
    assert_eq!(
        std::fs::read_to_string(config.home.join(".local/bin/inherited-tool")).unwrap(),
        "installed\n"
    );
    drop(supervisor);

    let restarted = Supervisor::new(config.clone(), SupervisorBoundary::default()).unwrap();
    assert_eq!(restarted.phase().await, Phase::RestorePending);
    assert_eq!(
        restarted
            .dispatch(RequestFrame::BootstrapRestore(request))
            .await,
        first
    );
    assert_eq!(restarted.phase().await, Phase::RestorePending);
    assert_eq!(
        failure(restarted.dispatch(workspace()).await).code,
        "credential_not_installed"
    );
    assert!(matches!(
        restarted.dispatch(credential()).await,
        ResponseFrame::Credential(_)
    ));
    assert_eq!(
        failure(
            restarted
                .dispatch(RequestFrame::Restore(restore.clone()))
                .await
        )
        .code,
        "checkpoint_not_found"
    );
    // The production transport imports the exact durable object before every
    // restore request, including retries after a supervisor restart.
    let store = restarted.checkpoint_store();
    let (temporary, expected) = store.upload_path(&restore.object_reference).unwrap();
    std::fs::write(&temporary, archived).unwrap();
    store
        .commit_upload(temporary, restore.object_reference.clone(), expected)
        .await
        .unwrap();
    assert!(matches!(
        restarted.dispatch(RequestFrame::Restore(restore)).await,
        ResponseFrame::Restore(_)
    ));
    assert_eq!(restarted.phase().await, Phase::Credentialed);
    cleanup(&config.home);
}

#[tokio::test]
async fn ordinary_install_and_changed_bootstrap_identities_are_mutually_exclusive() {
    let installed_config = config("ordinary-first");
    let installed =
        Supervisor::new(installed_config.clone(), SupervisorBoundary::default()).unwrap();
    assert!(matches!(
        installed
            .dispatch(RequestFrame::Install(InstallRequest::new(
                "ordinary-install",
                Vec::new(),
            )))
            .await,
        ResponseFrame::Install(_)
    ));
    let restore = sealed_restore(&installed_config);
    assert_eq!(
        failure(
            installed
                .dispatch(RequestFrame::BootstrapRestore(bootstrap(restore)))
                .await
        )
        .code,
        "install_mode_conflict"
    );
    cleanup(&installed_config.home);

    let inherited_config = config("changed-bootstrap");
    let restore = sealed_restore(&inherited_config);
    let inherited =
        Supervisor::new(inherited_config.clone(), SupervisorBoundary::default()).unwrap();
    let original = bootstrap(restore.clone());
    assert!(matches!(
        inherited
            .dispatch(RequestFrame::BootstrapRestore(original))
            .await,
        ResponseFrame::BootstrapRestore(_)
    ));
    let changed = RestoreBootstrapRequest::new(
        "sandbox:sandbox-test:restore-bootstrap:v2",
        "sandbox-test",
        restore,
    );
    assert_eq!(
        failure(
            inherited
                .dispatch(RequestFrame::BootstrapRestore(changed))
                .await
        )
        .code,
        "restore_bootstrap_conflict"
    );
    cleanup(&inherited_config.home);
}

#[tokio::test]
async fn restored_home_contents_are_never_installation_authority() {
    let config = config("home-is-not-proof");
    std::fs::create_dir_all(config.home.join(".local/bin")).unwrap();
    std::fs::write(config.home.join(".local/bin/tool"), "present\n").unwrap();

    let supervisor = Supervisor::new(config.clone(), SupervisorBoundary::default()).unwrap();
    assert_eq!(supervisor.phase().await, Phase::Provisioned);
    let refusal = failure(supervisor.dispatch(credential()).await);
    assert_eq!(refusal.code, "install_not_completed");
    cleanup(&config.home);
}
