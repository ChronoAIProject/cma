use std::path::PathBuf;
use std::time::Duration;

use layer3_adapter::wire::{RequestFrame, ResponseFrame};
use layer3_adapter::{
    CredentialFrame, InstallRequest, RestoreBootstrapRequest, RestoreRequest, RuntimeCredential,
    RuntimeStatus, RuntimeStatusRequest, WorkspaceRequest, WorkspaceSource,
};

use super::{Phase, RuntimeConfig, Supervisor, SupervisorBoundary};

#[path = "supervisor_process_generation_tests.rs"]
mod process_generation;
pub(super) use process_generation::restart_open;
#[path = "supervisor_workspace_preparation_tests.rs"]
mod workspace_preparation;
#[path = "supervisor_workspace_recovery_tests.rs"]
mod workspace_recovery;

struct Fixture {
    root: PathBuf,
    config: RuntimeConfig,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "cma-persistent-home-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let config = RuntimeConfig {
            home_identity: Some("a".repeat(43)),
            ..RuntimeConfig::for_test(root.join("home"))
        };
        std::fs::create_dir_all(&config.home).unwrap();
        Self { root, config }
    }

    fn supervisor(&self) -> std::sync::Arc<Supervisor> {
        Supervisor::new(self.config.clone(), SupervisorBoundary::default()).unwrap()
    }

    fn marker(&self) -> PathBuf {
        self.config.state_root.join("home-binding/binding.json")
    }

    fn status(&self) -> RequestFrame {
        RequestFrame::RuntimeStatus(RuntimeStatusRequest {
            sandbox_id: self.config.sandbox_id.clone(),
            generation: self.config.generation,
            home_identity: self.config.home_identity.clone().unwrap(),
        })
    }

    fn archive(&self) -> RestoreRequest {
        let source = self.root.join("source");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("changed"), "snapshot version").unwrap();
        std::fs::write(source.join("deleted"), "snapshot version").unwrap();
        let sealed =
            crate::checkpoint_archive::seal(&source, &self.config.checkpoint_root, &[]).unwrap();
        RestoreRequest {
            object_reference: sealed.reference,
            sha256: sealed.sha256,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

fn credential() -> RequestFrame {
    RequestFrame::Credential(CredentialFrame {
        operation_id: "persistent-credential".into(),
        value: RuntimeCredential::new("opaque-test-credential"),
        custody_generation: Some(1),
        github_route: None,
    })
}

fn bootstrap(request: RestoreRequest) -> RequestFrame {
    RequestFrame::BootstrapRestore(RestoreBootstrapRequest::new(
        "persistent-restore",
        "sandbox-test",
        request,
    ))
}

fn workspace() -> RequestFrame {
    RequestFrame::PrepareWorkspace(WorkspaceRequest::new(
        "workspace:workspace:prepare:v1",
        "workspace",
        "project",
        WorkspaceSource::Init,
    ))
}

fn status(response: ResponseFrame) -> RuntimeStatus {
    match response {
        ResponseFrame::RuntimeStatus(status) => status,
        other => panic!("expected runtime status: {other:?}"),
    }
}

fn failure(response: ResponseFrame) -> String {
    match response {
        ResponseFrame::Failure(failure) => failure.code,
        other => panic!("expected refusal: {other:?}"),
    }
}

#[tokio::test]
async fn persistent_restart_preserves_installation_and_files_but_requires_new_credentials() {
    let fixture = Fixture::new();
    let supervisor = fixture.supervisor();
    let initial = status(supervisor.dispatch(fixture.status()).await);
    assert!(!initial.installation_ready && !initial.credential_ready);
    let install = RequestFrame::Install(InstallRequest::new(
        "install-once",
        vec!["printf 'installed\n' >> install-count".into()],
    ));
    assert!(matches!(
        supervisor.dispatch(install.clone()).await,
        ResponseFrame::Install(_)
    ));
    assert!(matches!(
        supervisor.dispatch(credential()).await,
        ResponseFrame::Credential(_)
    ));
    assert!(status(supervisor.dispatch(fixture.status()).await).credential_ready);
    std::fs::write(
        fixture.config.home.join("latest-work"),
        "after installation",
    )
    .unwrap();
    drop(supervisor);

    let restarted = fixture.supervisor();
    let observed = status(restarted.dispatch(fixture.status()).await);
    assert_ne!(observed.boot_id, initial.boot_id);
    assert!(observed.installation_ready && !observed.credential_ready && !observed.restore_pending);
    assert_eq!(observed.agent_processes, 0);
    assert_eq!(
        failure(restarted.dispatch(workspace()).await),
        "credential_not_installed"
    );
    assert!(matches!(
        restarted.dispatch(install).await,
        ResponseFrame::Install(_)
    ));
    assert_eq!(
        std::fs::read_to_string(fixture.config.home.join("install-count")).unwrap(),
        "installed\n"
    );
    assert!(matches!(
        restarted.dispatch(credential()).await,
        ResponseFrame::Credential(_)
    ));
    assert!(matches!(
        restarted.dispatch(workspace()).await,
        ResponseFrame::PrepareWorkspace(_)
    ));
    assert_eq!(
        std::fs::read_to_string(fixture.config.home.join("latest-work")).unwrap(),
        "after installation"
    );
}

#[tokio::test]
async fn completed_persistent_restore_replay_preserves_subsequent_edits_deletions_and_creations() {
    let fixture = Fixture::new();
    let supervisor = fixture.supervisor();
    let restore = fixture.archive();
    let archive = std::fs::read(
        fixture
            .config
            .checkpoint_root
            .join(&restore.object_reference),
    )
    .unwrap();
    let receipt = supervisor.dispatch(bootstrap(restore.clone())).await;
    assert!(matches!(receipt, ResponseFrame::BootstrapRestore(_)));
    assert!(status(supervisor.dispatch(fixture.status()).await).restore_pending);
    assert!(matches!(
        supervisor.dispatch(credential()).await,
        ResponseFrame::Credential(_)
    ));
    assert!(matches!(
        supervisor
            .dispatch(RequestFrame::Restore(restore.clone()))
            .await,
        ResponseFrame::Restore(_)
    ));
    std::fs::write(fixture.config.home.join("changed"), "newer user work").unwrap();
    std::fs::remove_file(fixture.config.home.join("deleted")).unwrap();
    std::fs::create_dir(fixture.config.home.join("new-workspace")).unwrap();
    std::fs::write(
        fixture.config.home.join("new-workspace/new-file"),
        "new file",
    )
    .unwrap();
    drop(supervisor);

    let restarted = fixture.supervisor();
    let observed = status(restarted.dispatch(fixture.status()).await);
    assert!(observed.installation_ready && !observed.restore_pending && !observed.credential_ready);
    assert_eq!(
        restarted.dispatch(bootstrap(restore.clone())).await,
        receipt
    );
    assert_eq!(restarted.phase().await, Phase::Installed);
    assert!(matches!(
        restarted.dispatch(credential()).await,
        ResponseFrame::Credential(_)
    ));
    // The hosted transport may reimport a duplicate before replaying its control frame.
    let (path, expected) = restarted
        .checkpoint_store()
        .upload_path(&restore.object_reference)
        .unwrap();
    std::fs::write(&path, archive).unwrap();
    restarted
        .checkpoint_store()
        .commit_upload(path, restore.object_reference.clone(), expected)
        .await
        .unwrap();
    for _ in 0..2 {
        assert!(matches!(
            restarted
                .dispatch(RequestFrame::Restore(restore.clone()))
                .await,
            ResponseFrame::Restore(_)
        ));
    }
    assert_eq!(
        std::fs::read_to_string(fixture.config.home.join("changed")).unwrap(),
        "newer user work"
    );
    assert!(!fixture.config.home.join("deleted").exists());
    assert_eq!(
        std::fs::read_to_string(fixture.config.home.join("new-workspace/new-file")).unwrap(),
        "new file"
    );
    assert!(!fixture
        .config
        .checkpoint_root
        .join(&restore.object_reference)
        .exists());
    assert!(matches!(
        restarted.dispatch(workspace()).await,
        ResponseFrame::PrepareWorkspace(_)
    ));
    let changed = RestoreRequest {
        object_reference: format!("home-{}.cma", "b".repeat(64)),
        sha256: "b".repeat(64),
    };
    assert_eq!(
        failure(restarted.dispatch(RequestFrame::Restore(changed)).await),
        "restore_bootstrap_conflict"
    );
}

#[tokio::test]
async fn interrupted_restore_is_never_native_ready_and_retries_the_exact_archive() {
    let fixture = Fixture::new();
    let supervisor = fixture.supervisor();
    let restore = fixture.archive();
    assert!(matches!(
        supervisor.dispatch(bootstrap(restore.clone())).await,
        ResponseFrame::BootstrapRestore(_)
    ));
    assert!(!crate::persistent_home::begin_restore(&fixture.config, &restore).unwrap());
    std::fs::write(
        fixture.config.home.join("incomplete-home"),
        "partial extraction",
    )
    .unwrap();
    drop(supervisor);
    let restarted = fixture.supervisor();
    let observed = status(restarted.dispatch(fixture.status()).await);
    assert!(!observed.installation_ready && observed.restore_pending);
    assert!(matches!(
        restarted.dispatch(credential()).await,
        ResponseFrame::Credential(_)
    ));
    assert_eq!(
        failure(restarted.dispatch(workspace()).await),
        "restore_not_completed"
    );
    assert!(matches!(
        restarted.dispatch(RequestFrame::Restore(restore)).await,
        ResponseFrame::Restore(_)
    ));
    assert!(!fixture.config.home.join("incomplete-home").exists());
    assert_eq!(
        std::fs::read_to_string(fixture.config.home.join("changed")).unwrap(),
        "snapshot version"
    );
    assert!(status(restarted.dispatch(fixture.status()).await).installation_ready);
}

#[tokio::test]
async fn observation_does_not_wait_for_operation_gate_or_recover_journals() {
    let fixture = Fixture::new();
    let supervisor = fixture.supervisor();
    let install = InstallRequest::new("interrupted-install", vec![]);
    let record = crate::install_state::InstallRecord::started(&fixture.config, &install);
    assert!(crate::install_state::write_initial(&fixture.config, &record).unwrap());
    let workspace_journal = fixture.config.state_root.join("workspace-operations");
    std::fs::create_dir_all(&workspace_journal).unwrap();
    let corrupt = workspace_journal.join(format!("{}.json", "0".repeat(64)));
    std::fs::write(&corrupt, "{broken").unwrap();
    let marker_before = std::fs::read(fixture.marker()).unwrap();
    let guard = supervisor.operation_gate.lock().await;
    let observed = status(
        tokio::time::timeout(
            Duration::from_millis(500),
            supervisor.dispatch(fixture.status()),
        )
        .await
        .unwrap(),
    );
    assert!(!observed.installation_ready);
    assert_eq!(
        crate::install_state::inspect(&fixture.config).unwrap(),
        Some(record)
    );
    assert_eq!(std::fs::read_to_string(corrupt).unwrap(), "{broken");
    assert_eq!(std::fs::read(fixture.marker()).unwrap(), marker_before);
    drop(guard);
}

#[tokio::test]
async fn missing_or_foreign_persistent_identity_refuses_credentials_and_never_rebinds() {
    let fixture = Fixture::new();
    let supervisor = fixture.supervisor();
    assert!(matches!(
        supervisor
            .dispatch(RequestFrame::Install(InstallRequest::new(
                "install",
                vec![]
            )))
            .await,
        ResponseFrame::Install(_)
    ));
    let original = std::fs::read(fixture.marker()).unwrap();
    let mut foreign = fixture.config.clone();
    foreign.home_identity = Some("b".repeat(43));
    assert!(Supervisor::new(foreign, SupervisorBoundary::default()).is_err());
    assert_eq!(std::fs::read(fixture.marker()).unwrap(), original);
    let mut request = fixture.status();
    let RequestFrame::RuntimeStatus(ref mut identity) = request else {
        unreachable!()
    };
    identity.generation += 1;
    assert_eq!(
        failure(supervisor.dispatch(request).await),
        "runtime_status_identity_invalid"
    );
    std::fs::remove_file(fixture.marker()).unwrap();
    assert_eq!(
        failure(supervisor.dispatch(credential()).await),
        "persistent_home_invalid"
    );
    assert!(Supervisor::new(fixture.config.clone(), SupervisorBoundary::default()).is_err());
    assert!(!fixture.marker().exists());
}

#[test]
fn missing_mounted_home_is_never_recreated_as_an_empty_directory() {
    let fixture = Fixture::new();
    std::fs::remove_dir(&fixture.config.home).unwrap();
    assert!(Supervisor::new(fixture.config.clone(), SupervisorBoundary::default()).is_err());
    assert!(!fixture.config.home.exists());
}

#[tokio::test]
async fn published_restore_completion_recovers_when_the_in_memory_update_was_lost() {
    let fixture = Fixture::new();
    let supervisor = fixture.supervisor();
    let restore = fixture.archive();
    assert!(matches!(
        supervisor.dispatch(bootstrap(restore.clone())).await,
        ResponseFrame::BootstrapRestore(_)
    ));
    assert!(matches!(
        supervisor.dispatch(credential()).await,
        ResponseFrame::Credential(_)
    ));
    crate::persistent_home::begin_restore(&fixture.config, &restore).unwrap();
    supervisor.checkpoint.restore(&restore).await.unwrap();
    supervisor
        .checkpoint
        .discard(&restore.object_reference)
        .await
        .unwrap();
    crate::persistent_home::complete_restore(&fixture.config, &restore).unwrap();
    assert_eq!(supervisor.phase().await, Phase::RestoreCredentialed);
    // Only the durable completion exists, as when publication succeeded but
    // the response/phase update did not. Ordinary recovery must promote it.
    let response = supervisor.dispatch(workspace()).await;
    assert!(
        matches!(response, ResponseFrame::PrepareWorkspace(_)),
        "{response:?}"
    );
    assert_eq!(supervisor.phase().await, Phase::Credentialed);
}

#[test]
fn unpublished_partial_binding_writes_are_discarded_without_becoming_authority() {
    let fixture = Fixture::new();
    let directory = fixture.config.state_root.join("home-binding");
    std::fs::create_dir_all(&directory).unwrap();
    let stale = directory.join(format!(".home-{}.tmp", "0".repeat(32)));
    std::fs::write(&stale, "{partially written").unwrap();
    let supervisor = fixture.supervisor();
    assert!(!stale.exists());
    let binding = std::fs::read(fixture.marker()).unwrap();
    std::fs::write(&stale, "{another partial write").unwrap();
    drop(supervisor);
    let _restarted = fixture.supervisor();
    assert!(!stale.exists());
    assert_eq!(std::fs::read(fixture.marker()).unwrap(), binding);
}

#[cfg(unix)]
#[tokio::test]
async fn symlinked_or_corrupt_binding_cannot_authorize_home_use() {
    let fixture = Fixture::new();
    let supervisor = fixture.supervisor();
    let original = std::fs::read(fixture.marker()).unwrap();
    let outside = fixture.root.join("outside-binding");
    std::fs::rename(fixture.marker(), &outside).unwrap();
    std::os::unix::fs::symlink(&outside, fixture.marker()).unwrap();
    assert!(matches!(
        supervisor.dispatch(fixture.status()).await,
        ResponseFrame::Failure(_)
    ));
    assert!(Supervisor::new(fixture.config.clone(), SupervisorBoundary::default()).is_err());
    assert_eq!(std::fs::read(outside).unwrap(), original);
    std::fs::remove_file(fixture.marker()).unwrap();
    for bytes in [b"{broken".to_vec(), vec![b' '; 4097]] {
        std::fs::write(fixture.marker(), bytes).unwrap();
        assert!(matches!(
            supervisor.dispatch(credential()).await,
            ResponseFrame::Failure(_)
        ));
        assert!(Supervisor::new(fixture.config.clone(), SupervisorBoundary::default()).is_err());
    }
}

#[path = "supervisor_checked_file_read_tests.rs"]
mod checked_file_read;
