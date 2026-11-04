use super::*;
use std::io::Write;
use std::sync::{Arc, Mutex};

use axum::{extract::Request, response::IntoResponse, routing::get, Json, Router};

struct Repository {
    root: PathBuf,
    reference: Arc<Mutex<String>>,
    reads: Arc<Mutex<Vec<String>>>,
    server: tokio::task::JoinHandle<()>,
}

impl Repository {
    async fn serve(config: &mut RuntimeConfig) -> Self {
        let root = PathBuf::from("/tmp").join(format!("cma-pin-{}", uuid::Uuid::new_v4().simple()));
        let proxy = crate::github_proxy::configuration(
            &root,
            &mut Default::default(),
            Some(&crate::config::ProxyConfig {
                role: "platform",
                bind: "127.0.0.1:9092".parse().unwrap(),
                upstream: "http://127.0.0.1:9093".into(),
            }),
        )
        .unwrap()
        .unwrap();
        std::fs::create_dir_all(proxy.socket().parent().unwrap()).unwrap();
        let listener = tokio::net::UnixListener::bind(proxy.socket()).unwrap();
        config.github_proxy = Some(proxy);
        let reference = Arc::new(Mutex::new("a".repeat(40)));
        let reads = Arc::new(Mutex::new(Vec::new()));
        let current = reference.clone();
        let requests = reads.clone();
        let app = Router::new().fallback(get(move |request: Request| {
            let reference = current.lock().unwrap().clone();
            requests.lock().unwrap().push(request.uri().to_string());
            async move {
                let path = request.uri().path();
                if path.ends_with("/commits") {
                    return Json(serde_json::json!([{ "sha": reference }])).into_response();
                }
                if let Some((_, commit)) = path.split_once("/zipball/") {
                    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
                    zip.start_file(
                        "project/retained-work",
                        zip::write::SimpleFileOptions::default(),
                    )
                    .unwrap();
                    zip.write_all(if commit == "a".repeat(40) {
                        b"original work"
                    } else {
                        b"moved source"
                    })
                    .unwrap();
                    return zip.finish().unwrap().into_inner().into_response();
                }
                Json(serde_json::json!({ "default_branch": "main" })).into_response()
            }
        }));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            root,
            reference,
            reads,
            server,
        }
    }
}

impl Drop for Repository {
    fn drop(&mut self) {
        self.server.abort();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[tokio::test]
async fn snapshot_recovery_retains_source_pinned_before_workspace_identity_publication() {
    let mut fixture = Fixture::new(WorkspaceSource::RepositorySnapshot {
        url: "https://github.com/example/project".into(),
        reference: Some("main".into()),
    });
    let paths = Paths::new(&fixture.config, &fixture.request);
    std::fs::remove_dir_all(&paths.staged).unwrap();
    let repository = Repository::serve(&mut fixture.config).await;
    let config = fixture.config.clone();
    let mut request = fixture.request.original.clone();
    request.name.clone_from(&request.workspace_id);
    let preparation = tokio::spawn(async move {
        crate::repository_snapshot::prepare(&config, &config.home.join(".cma/preparing"), &request)
            .await
    });
    let pin = paths.staged.join(".git/info/cma-source-commit");
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !pin.exists() {
            assert!(
                !preparation.is_finished(),
                "preparation ended before interruption"
            );
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    preparation.abort();
    assert!(preparation.await.unwrap_err().is_cancelled());
    assert!(!paths.staged.join(".git/info/cma-workspace-id").exists());
    assert_eq!(std::fs::read_to_string(&pin).unwrap(), "a".repeat(40));
    std::fs::write(paths.staged.join(".git/index.lock"), "original index").unwrap();
    *repository.reference.lock().unwrap() = "b".repeat(40);
    repository.reads.lock().unwrap().clear();

    let receipt = execute(&fixture.config, &fixture.request).await.unwrap();
    assert_eq!(receipt.prepared.source_commit, Some("a".repeat(40)));
    assert_eq!(
        std::fs::read_to_string(paths.published.join("retained-work")).unwrap(),
        "original work"
    );
    assert!(!repository
        .reads
        .lock()
        .unwrap()
        .iter()
        .any(|uri| uri.contains("/commits")));
    assert_eq!(
        execute(&fixture.config, &fixture.request).await.unwrap(),
        receipt
    );
    fixture.assert_original();
}

#[tokio::test]
async fn snapshot_recovery_refuses_unestablished_source_pin_without_touching_original_files() {
    for evidence in ["missing", "malformed", "oversized", "symlink", "directory"] {
        let fixture = Fixture::new(WorkspaceSource::RepositorySnapshot {
            url: "https://github.com/example/project".into(),
            reference: Some("main".into()),
        });
        let paths = Paths::new(&fixture.config, &fixture.request);
        let pin = paths.staged.join(".git/info/cma-source-commit");
        match evidence {
            "missing" => (),
            "malformed" => std::fs::write(&pin, "main").unwrap(),
            "oversized" => std::fs::write(&pin, "a".repeat(65)).unwrap(),
            "symlink" => {
                let target = fixture.root.join("other-pin");
                std::fs::write(&target, "a".repeat(40)).unwrap();
                std::os::unix::fs::symlink(target, &pin).unwrap();
            }
            "directory" => std::fs::create_dir(&pin).unwrap(),
            _ => unreachable!(),
        }
        let error = execute(&fixture.config, &fixture.request)
            .await
            .unwrap_err();
        assert_eq!(error.code, "workspace_recovery_ineligible", "{evidence}");
        assert_eq!(std::fs::read(&fixture.journal).unwrap(), fixture.before);
        assert_eq!(
            std::fs::read_to_string(paths.staged.join("retained-work")).unwrap(),
            "original work"
        );
        assert!(!paths.preserved.exists());
        assert!(!paths.published.exists());
        assert!(!paths.candidate.exists());
    }
}

#[tokio::test]
async fn snapshot_recovery_refuses_an_interrupted_candidate_with_a_different_source_pin() {
    let fixture = Fixture::new(WorkspaceSource::RepositorySnapshot {
        url: "https://github.com/example/project".into(),
        reference: Some("main".into()),
    });
    let paths = Paths::new(&fixture.config, &fixture.request);
    std::fs::write(
        paths.staged.join(".git/info/cma-source-commit"),
        "a".repeat(40),
    )
    .unwrap();
    assert_eq!(
        execute(&fixture.config, &fixture.request)
            .await
            .unwrap_err()
            .code,
        "workspace_recovery_attempt_failed"
    );
    std::fs::remove_file(paths.attempt(&fixture.request).join("terminal.json")).unwrap();
    let staged = WorkspaceRequest::new(
        "old-candidate",
        "wks_recover",
        "wks_recover",
        WorkspaceSource::Init,
    );
    crate::workspace_source::prepare(&paths.candidate_root, true, &staged)
        .await
        .unwrap();
    std::fs::write(
        paths.candidate.join(".git/info/cma-source-commit"),
        "b".repeat(40),
    )
    .unwrap();
    assert_eq!(
        execute(&fixture.config, &fixture.request)
            .await
            .unwrap_err()
            .code,
        "workspace_recovery_attempt_failed"
    );
    assert!(!paths.published.exists());
    assert!(paths.candidate.exists());
    assert!(!paths
        .attempt(&fixture.request)
        .join("publication.json")
        .exists());
    fixture.assert_original();
}

#[tokio::test]
async fn snapshot_recovery_preserves_a_valid_pin_from_a_legacy_recovery_admission() {
    let mut fixture = Fixture::new(WorkspaceSource::RepositorySnapshot {
        url: "https://github.com/example/project".into(),
        reference: Some("main".into()),
    });
    let paths = Paths::new(&fixture.config, &fixture.request);
    std::fs::write(
        paths.staged.join(".git/info/cma-source-commit"),
        "a".repeat(40),
    )
    .unwrap();
    assert_eq!(
        execute(&fixture.config, &fixture.request)
            .await
            .unwrap_err()
            .code,
        "workspace_recovery_attempt_failed"
    );
    let admission = paths.evidence.join("original.json");
    let mut legacy: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&admission).unwrap()).unwrap();
    legacy.as_object_mut().unwrap().remove("source_commit");
    let before = serde_json::to_vec(&legacy).unwrap();
    std::fs::write(&admission, &before).unwrap();
    let repository = Repository::serve(&mut fixture.config).await;
    *repository.reference.lock().unwrap() = "b".repeat(40);
    let retry = fixture.retry("recovery-second");
    let receipt = execute(&fixture.config, &retry).await.unwrap();
    assert_eq!(receipt.prepared.source_commit, Some("a".repeat(40)));
    assert_eq!(std::fs::read(&admission).unwrap(), before);
    assert_eq!(execute(&fixture.config, &retry).await.unwrap(), receipt);
    std::fs::write(
        paths.preserved.join(".git/info/cma-source-commit"),
        "b".repeat(40),
    )
    .unwrap();
    assert_eq!(
        execute(&fixture.config, &retry).await.unwrap_err().code,
        "workspace_recovery_ineligible"
    );
    fixture.assert_original();
}
