use super::*;
use layer3_adapter::{WorkspaceRequest, WorkspaceSource};

struct Fixture {
    root: PathBuf,
    config: RuntimeConfig,
    request: WorkspacePrepareRecoveryRequest,
    journal: PathBuf,
    before: Vec<u8>,
}
impl Fixture {
    fn new(source: WorkspaceSource) -> Self {
        let root = std::env::temp_dir().join(format!(
            "cma-prepare-recovery-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let config = RuntimeConfig {
            home_identity: Some("a".repeat(43)),
            ..RuntimeConfig::for_test(root.join("home"))
        };
        std::fs::create_dir_all(&config.home).unwrap();
        let original = WorkspaceRequest::new(
            "workspace:wks_recover:prepare:v1",
            "wks_recover",
            "project",
            source,
        );
        let crate::workspace_journal::Begin::New(ticket) =
            crate::workspace_journal::begin::<WorkspaceReceipt, _>(
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
            .unwrap()
        else {
            panic!("new original admission");
        };
        ticket
            .indeterminate("original clone stopped before completed metadata")
            .unwrap();
        let staged = config.home.join(".cma/preparing/wks_recover");
        std::fs::create_dir_all(staged.join(".git/info")).unwrap();
        std::fs::write(staged.join("retained-work"), "original work").unwrap();
        std::fs::write(staged.join(".git/index.lock"), "original index").unwrap();
        let journal = config
            .state_root
            .join("workspace-operations")
            .join(format!("{}.json", operation_hash(&original.operation_id)));
        let before = std::fs::read(&journal).unwrap();
        let request = WorkspacePrepareRecoveryRequest::new("recovery-first", original, None);
        Self {
            root,
            config,
            request,
            journal,
            before,
        }
    }
    fn retry(&self, id: &str) -> WorkspacePrepareRecoveryRequest {
        WorkspacePrepareRecoveryRequest::new(
            id,
            self.request.original.clone(),
            Some(self.request.operation_id.clone()),
        )
    }
    fn assert_original(&self) {
        let paths = Paths::new(&self.config, &self.request);
        assert_eq!(std::fs::read(&self.journal).unwrap(), self.before);
        assert_eq!(
            std::fs::read_to_string(paths.preserved.join("retained-work")).unwrap(),
            "original work"
        );
        assert_eq!(
            std::fs::read_to_string(paths.preserved.join(".git/index.lock")).unwrap(),
            "original index"
        );
        assert!(!paths.preserved.join(".git/info/cma-workspace-id").exists());
        assert!(!paths.staged.exists());
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[tokio::test]
async fn preserving_recovery_publishes_same_workspace_and_replays_without_repeating_effects() {
    let fixture = Fixture::new(WorkspaceSource::Init);
    let paths = Paths::new(&fixture.config, &fixture.request);
    let original_inode = directory(&paths.staged).unwrap().unwrap();
    let receipt = execute(&fixture.config, &fixture.request).await.unwrap();
    assert_eq!(
        receipt.prepared.workspace_id,
        fixture.request.original.workspace_id
    );
    assert_eq!(receipt.prepared.name, "project");
    assert_eq!(receipt.prepared.operation_id, fixture.request.operation_id);
    assert_eq!(directory(&paths.preserved).unwrap(), Some(original_inode));
    let published_inode = directory(&paths.published).unwrap();
    std::fs::write(paths.published.join("owner-work"), "after success").unwrap();
    assert_eq!(
        execute(&fixture.config, &fixture.request).await.unwrap(),
        receipt
    );
    assert_eq!(directory(&paths.published).unwrap(), published_inode);
    assert_eq!(
        std::fs::read_to_string(paths.published.join("owner-work")).unwrap(),
        "after success"
    );
    assert!(
        execute(&fixture.config, &fixture.retry("new-after-success"))
            .await
            .is_err()
    );
    fixture.assert_original();
}

#[tokio::test]
async fn preserving_recovery_recovers_lost_reply_and_restart_around_atomic_publication() {
    for before_rename in [false, true] {
        let fixture = Fixture::new(WorkspaceSource::Init);
        let paths = Paths::new(&fixture.config, &fixture.request);
        let receipt = execute(&fixture.config, &fixture.request).await.unwrap();
        std::fs::remove_file(paths.attempt(&fixture.request).join("terminal.json")).unwrap();
        if before_rename {
            std::fs::rename(&paths.published, &paths.candidate).unwrap();
        }
        let recovered = execute(&fixture.config, &fixture.request).await.unwrap();
        assert_eq!(recovered, receipt);
        assert!(paths.published.join(".git/info/cma-workspace-id").exists());
        fixture.assert_original();
    }
}

#[tokio::test]
async fn preserving_recovery_requires_terminal_prior_attempt_and_preserves_failed_candidate() {
    let fixture = Fixture::new(WorkspaceSource::Init);
    let paths = Paths::new(&fixture.config, &fixture.request);
    let receipt = execute(&fixture.config, &fixture.request).await.unwrap();
    let attempt = paths.attempt(&fixture.request);
    std::fs::remove_file(attempt.join("terminal.json")).unwrap();
    std::fs::remove_file(attempt.join("publication.json")).unwrap();
    std::fs::rename(&paths.published, &paths.candidate).unwrap();
    std::fs::remove_file(paths.candidate.join(".git/info/cma-workspace-id")).unwrap();
    std::fs::write(paths.candidate.join("partial-work"), "attempt work").unwrap();
    let retry = fixture.retry("retry-after-interrupted-candidate");
    assert!(execute(&fixture.config, &retry).await.is_err());
    let error = execute(&fixture.config, &fixture.request)
        .await
        .unwrap_err();
    assert_eq!(error.code, "workspace_recovery_attempt_failed");
    assert!(!error.retryable);
    let recovered = execute(&fixture.config, &retry).await.unwrap();
    assert_eq!(
        recovered.prepared.workspace_id,
        receipt.prepared.workspace_id
    );
    assert_eq!(
        std::fs::read_to_string(paths.candidate.join("partial-work")).unwrap(),
        "attempt work"
    );
    assert_eq!(
        execute(&fixture.config, &fixture.request)
            .await
            .unwrap_err()
            .code,
        "workspace_recovery_attempt_failed"
    );
    fixture.assert_original();
}

#[tokio::test]
async fn preserving_recovery_rejects_changed_source_name_home_and_original_marker() {
    for mutation in ["source", "name", "home", "marker", "published", "symlink"] {
        let mut fixture = Fixture::new(WorkspaceSource::Init);
        let paths = Paths::new(&fixture.config, &fixture.request);
        match mutation {
            "source" => {
                fixture.request.original.source = WorkspaceSource::Clone {
                    url: "https://github.com/example/other".into(),
                    reference: None,
                }
            }
            "name" => fixture.request.original.name = "other".into(),
            "home" => fixture.config.home_identity = None,
            "marker" => std::fs::write(
                paths.staged.join(".git/info/cma-workspace-id"),
                "wks_recover",
            )
            .unwrap(),
            "published" => std::fs::create_dir(&paths.published).unwrap(),
            "symlink" => {
                std::fs::remove_dir(paths.staged.join(".git/info")).unwrap();
                std::os::unix::fs::symlink(&fixture.config.home, paths.staged.join(".git/info"))
                    .unwrap();
            }
            _ => unreachable!(),
        }
        fixture.request.original.request_sha256 = fixture.request.original.computed_sha256();
        fixture.request.request_sha256 = fixture.request.computed_sha256();
        assert!(
            execute(&fixture.config, &fixture.request).await.is_err(),
            "accepted {mutation}"
        );
        assert_eq!(std::fs::read(&fixture.journal).unwrap(), fixture.before);
        assert!(paths.staged.join("retained-work").exists());
        assert!(!paths.preserved.exists());
    }
}

#[tokio::test]
async fn preserving_recovery_allows_transport_failure_without_runtime_admission() {
    let mut fixture = Fixture::new(WorkspaceSource::Init);
    fixture.request.previous_recovery_operation_id = Some("recovery-never-reached-runtime".into());
    fixture.request.request_sha256 = fixture.request.computed_sha256();
    execute(&fixture.config, &fixture.request).await.unwrap();
    fixture.assert_original();
}

#[tokio::test]
async fn snapshot_recovery_pins_source_commit_through_publication_and_terminal_replay() {
    let fixture = Fixture::new(WorkspaceSource::RepositorySnapshot {
        url: "https://github.com/example/project".into(),
        reference: Some("main".into()),
    });
    let paths = Paths::new(&fixture.config, &fixture.request);
    assert!(execute(&fixture.config, &fixture.request).await.is_err());
    let terminal_path = paths.attempt(&fixture.request).join("terminal.json");
    std::fs::remove_file(&terminal_path).unwrap();
    let staged = WorkspaceRequest::new(
        "local-snapshot-candidate",
        "wks_recover",
        "wks_recover",
        WorkspaceSource::Init,
    );
    crate::workspace_source::prepare(&paths.candidate_root, true, &staged)
        .await
        .unwrap();
    let source_commit = "c".repeat(40);
    std::fs::write(
        paths.candidate.join(".git/info/cma-source-commit"),
        &source_commit,
    )
    .unwrap();
    let receipt = execute(&fixture.config, &fixture.request).await.unwrap();
    assert_eq!(
        receipt.prepared.source_commit.as_deref(),
        Some(source_commit.as_str())
    );
    let publication: Publication = read(&paths.attempt(&fixture.request).join("publication.json"))
        .unwrap()
        .unwrap();
    assert_eq!(publication.source_commit, receipt.prepared.source_commit);
    std::fs::write(
        paths.published.join(".git/info/cma-source-commit"),
        "d".repeat(40),
    )
    .unwrap();
    assert_eq!(
        execute(&fixture.config, &fixture.request).await.unwrap(),
        receipt
    );
    std::fs::remove_file(&terminal_path).unwrap();
    assert!(execute(&fixture.config, &fixture.request).await.is_err());
    std::fs::write(
        paths.published.join(".git/info/cma-source-commit"),
        source_commit,
    )
    .unwrap();
    assert_eq!(
        execute(&fixture.config, &fixture.request).await.unwrap(),
        receipt
    );
    let mut changed = receipt.clone();
    changed.prepared.source_commit = Some("d".repeat(40));
    std::fs::write(
        &terminal_path,
        serde_json::to_vec(&Terminal::Completed { receipt: changed }).unwrap(),
    )
    .unwrap();
    assert!(execute(&fixture.config, &fixture.request).await.is_err());
    fixture.assert_original();
}

#[test]
fn legacy_recovery_publication_keeps_its_original_shape() {
    let legacy = serde_json::json!({
        "directory": { "device": 1, "inode": 2 },
        "resolved_commit": "a".repeat(40),
    });
    let publication: Publication = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(publication.source_commit, None);
    assert_eq!(serde_json::to_value(publication).unwrap(), legacy);
}
