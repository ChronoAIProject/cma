use super::*;

fn root(name: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "chrono-workspace-source-{}-{sequence}-{name}",
        std::process::id()
    ))
}

fn git(cwd: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_COUNT", "0")
        .output()
        .expect("the test git command starts");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn fixture(name: &str) -> (PathBuf, PathBuf, WorkspaceSources, WorkspaceSourceRequest) {
    let base = root(name);
    let workspace = base.join("workspace");
    let repository = base.join("fixture");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&repository).unwrap();
    git(&repository, &["init", "--initial-branch", "dev"]);
    git(&repository, &["config", "user.name", "CMA fixture"]);
    git(
        &repository,
        &["config", "user.email", "fixture@example.test"],
    );
    std::fs::write(repository.join("README.md"), b"fixture\n").unwrap();
    git(&repository, &["add", "README.md"]);
    git(&repository, &["commit", "-m", "fixture"]);

    let requested_url = "https://fixture.invalid/repository".to_string();
    let local_url = format!("file://{}", repository.display());
    let sources = WorkspaceSources::with_url_rewrite(requested_url.clone(), local_url);
    let request = WorkspaceSourceRequest {
        url: requested_url,
        reference: "dev".into(),
        directory: "repository".into(),
        workspace_epoch: 7,
    };
    (base, workspace, sources, request)
}

#[test]
fn source_and_tree_inputs_are_closed_and_path_safe() {
    for url in [
        "http://example.test/repo",
        "https://user@example.test/repo",
        "https://example.test/repo?token=secret",
        "not a URL",
    ] {
        let request = WorkspaceSourceRequest {
            url: url.into(),
            reference: "dev".into(),
            directory: "repo".into(),
            workspace_epoch: 1,
        };
        assert!(matches!(
            request.validate(),
            Err(WorkspaceSourceError::Invalid(_))
        ));
    }
    for directory in ["", ".", "..", "nested/repo", "repo\\other", "r\u{e9}po"] {
        let request = WorkspaceSourceRequest {
            url: "https://example.test/repo".into(),
            reference: "dev".into(),
            directory: directory.into(),
            workspace_epoch: 1,
        };
        assert!(matches!(
            request.validate(),
            Err(WorkspaceSourceError::Invalid(_))
        ));
    }
    let invalid_names = vec![
        String::new(),
        "UPPER".into(),
        "with_underscore".into(),
        "nested/tree".into(),
        "a".repeat(41),
    ];
    for name in invalid_names {
        assert!(validate_tree_name(&name).is_err(), "accepted {name:?}");
    }
    for name in ["a".to_string(), "thread-42".into(), "z".repeat(40)] {
        assert!(validate_tree_name(&name).is_ok(), "refused {name:?}");
    }
}

#[tokio::test]
async fn local_clone_is_idempotent_and_worktrees_round_trip() {
    let (base, workspace, mut sources, request) = fixture("round-trip");
    sources.prepare(&workspace, request.clone()).await.unwrap();
    assert_eq!(
        std::fs::read(workspace.join("repository/README.md")).unwrap(),
        b"fixture\n"
    );

    sources.prepare(&workspace, request).await.unwrap();
    let receipt = sources
        .add_tree(
            &workspace,
            WorkspaceTreeRequest {
                name: "thread-1".into(),
                base: "dev".into(),
            },
        )
        .await
        .unwrap();
    let tree = workspace.join("worktrees/thread-1");
    assert_eq!(receipt.path, tree.to_string_lossy());
    assert_eq!(std::fs::read(tree.join("README.md")).unwrap(), b"fixture\n");
    sources.remove_tree(&workspace, "thread-1").await.unwrap();
    assert!(!tree.exists());
    assert!(matches!(
        sources.remove_tree(&workspace, "thread-1").await,
        Err(WorkspaceSourceError::NotFound)
    ));
    std::fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn occupied_or_different_sources_conflict_without_replacing_bytes() {
    let (base, workspace, mut sources, request) = fixture("occupied");
    let occupied = workspace.join("repository");
    std::fs::create_dir_all(&occupied).unwrap();
    std::fs::write(occupied.join("keep"), b"owner bytes").unwrap();
    assert!(matches!(
        sources.prepare(&workspace, request).await,
        Err(WorkspaceSourceError::Conflict(_))
    ));
    assert_eq!(
        std::fs::read(occupied.join("keep")).unwrap(),
        b"owner bytes"
    );
    std::fs::remove_dir_all(base).unwrap();

    let (base, workspace, mut sources, request) = fixture("different");
    sources.prepare(&workspace, request.clone()).await.unwrap();
    let mut different = request;
    different.reference = "other".into();
    assert!(matches!(
        sources.prepare(&workspace, different).await,
        Err(WorkspaceSourceError::Conflict(_))
    ));
    std::fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn handlers_refuse_clone_after_agent_start_and_admit_trees_behind_checkpoint_gate() {
    use axum::extract::State;
    use axum::response::IntoResponse as _;

    let (base, workspace, sources, request) = fixture("gates");
    let mut state = crate::AppState::for_test(&["/bin/cat"]);
    state.workspace_root = workspace.clone();
    state.workspace_source = std::sync::Arc::new(tokio::sync::Mutex::new(sources));
    let prepared =
        crate::workspace_source_prepare(State(state.clone()), axum::Json(request.clone()))
            .await
            .into_response();
    assert_eq!(prepared.status(), axum::http::StatusCode::OK);

    state
        .terminal_session("workspace-source-gate", 80, 24)
        .await
        .expect("the fixture agent starts");
    let refused = crate::workspace_source_prepare(State(state.clone()), axum::Json(request))
        .await
        .into_response();
    assert_eq!(refused.status(), axum::http::StatusCode::CONFLICT);

    let held = state.checkpoint_gate.lock().await;
    let mut adding = Box::pin(crate::workspace_tree_add(
        State(state.clone()),
        axum::Json(WorkspaceTreeRequest {
            name: "live-thread".into(),
            base: "dev".into(),
        }),
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), adding.as_mut())
            .await
            .is_err(),
        "worktree mutation bypassed the checkpoint gate"
    );
    drop(held);
    let added = tokio::time::timeout(Duration::from_secs(5), adding)
        .await
        .expect("the gated worktree operation completes")
        .into_response();
    assert_eq!(added.status(), axum::http::StatusCode::OK);
    assert!(workspace.join("worktrees/live-thread").exists());

    state.registry.remove("workspace-source-gate").await;
    let removed =
        crate::workspace_tree_remove(State(state), axum::extract::Path("live-thread".to_string()))
            .await
            .into_response();
    assert_eq!(removed.status(), axum::http::StatusCode::NO_CONTENT);
    std::fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn readiness_names_the_versioned_workspace_source_capability() {
    use axum::extract::State;
    use axum::response::IntoResponse as _;

    let response = crate::ready(State(crate::AppState::for_test(&[])))
        .await
        .into_response();
    let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        body["workspaceSource"],
        serde_json::json!({ "version": CAPABILITY_VERSION })
    );
}

#[tokio::test]
async fn public_routes_expose_the_closed_source_and_tree_contract() {
    let (base, workspace, sources, request) = fixture("routes");
    let mut state = crate::AppState::for_test(&[]);
    state.workspace_root = workspace.clone();
    state.workspace_source = std::sync::Arc::new(tokio::sync::Mutex::new(sources));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the route fixture binds");
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, crate::runtime_router(state))
            .await
            .expect("the route fixture serves");
    });
    let client = reqwest::Client::new();

    let prepared = client
        .post(format!("http://{address}/runtime/workspace/source"))
        .json(&serde_json::json!({
            "url": request.url,
            "reference": request.reference,
            "directory": request.directory,
            "workspace_epoch": request.workspace_epoch,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(prepared.status(), axum::http::StatusCode::OK);

    let invalid = client
        .post(format!("http://{address}/runtime/workspace/trees"))
        .json(&serde_json::json!({ "name": "Not-Safe", "base": "dev" }))
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), axum::http::StatusCode::BAD_REQUEST);

    let missing = client
        .delete(format!("http://{address}/runtime/workspace/trees/missing"))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), axum::http::StatusCode::NOT_FOUND);

    let added = client
        .post(format!("http://{address}/runtime/workspace/trees"))
        .json(&serde_json::json!({ "name": "route-tree", "base": "dev" }))
        .send()
        .await
        .unwrap();
    assert_eq!(added.status(), axum::http::StatusCode::OK);
    let receipt: serde_json::Value = added.json().await.unwrap();
    assert_eq!(
        receipt["path"],
        workspace
            .join("worktrees/route-tree")
            .to_string_lossy()
            .as_ref()
    );

    let removed = client
        .delete(format!(
            "http://{address}/runtime/workspace/trees/route-tree"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(removed.status(), axum::http::StatusCode::NO_CONTENT);
    server.abort();
    std::fs::remove_dir_all(base).unwrap();
}
