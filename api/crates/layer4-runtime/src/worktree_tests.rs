use layer3_adapter::{WorkspaceRequest, WorkspaceSource};
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

fn root() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "cma-worktree-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

#[tokio::test]
async fn preexisting_worktree_branch_is_preserved() {
    let home = root();
    let workspace = crate::workspace_source::prepare(
        &home,
        true,
        &WorkspaceRequest::new(
            "workspace:wks_project_one:prepare:v1",
            "wks_project_one",
            "project",
            WorkspaceSource::Init,
        ),
    )
    .await
    .unwrap();
    let repository = home.join("project");
    let git = GitRunner::new(true);
    git.checked(
        &repository,
        ["branch", "cma/agent-1", workspace.resolved_commit.as_str()],
        "create fixture branch",
    )
    .await
    .unwrap();

    let error = add(
        &home,
        true,
        &WorktreeRequest {
            workspace: "project".into(),
            name: "agent-1".into(),
            base: workspace.resolved_commit,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "worktree_exists");
    assert!(branch_exists(&git, &repository, "cma/agent-1")
        .await
        .unwrap());
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn real_git_worktrees_use_the_contract_path_and_branch() {
    let home = root();
    let workspace = crate::workspace_source::prepare(
        &home,
        true,
        &WorkspaceRequest::new(
            "workspace:wks_project_two:prepare:v1",
            "wks_project_two",
            "project",
            WorkspaceSource::Init,
        ),
    )
    .await
    .unwrap();
    let receipt = add(
        &home,
        true,
        &WorktreeRequest {
            workspace: "project".into(),
            name: "agent-1".into(),
            base: workspace.resolved_commit,
        },
    )
    .await
    .unwrap();
    assert_eq!(receipt.branch, "cma/agent-1");
    assert_eq!(
        PathBuf::from(&receipt.path),
        home.join("project/.worktrees/agent-1")
    );
    assert!(Path::new(&receipt.path).join(".gitkeep").is_file());
    assert_eq!(
        parse_path(&home, Path::new(&receipt.path)),
        Some(("project".into(), "agent-1".into()))
    );

    remove(
        &home,
        true,
        &WorktreeRef {
            workspace: "project".into(),
            name: "agent-1".into(),
        },
    )
    .await
    .unwrap();
    assert!(!Path::new(&receipt.path).exists());
    let branches = std::process::Command::new("git")
        .args(["branch", "--list", "cma/agent-1"])
        .current_dir(home.join("project"))
        .output()
        .unwrap();
    assert!(branches.stdout.is_empty());
    std::fs::remove_dir_all(home).unwrap();
}
