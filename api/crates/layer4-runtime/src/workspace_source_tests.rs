use super::*;

fn root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("cma-workspace-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn git(cwd: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_COUNT", "0")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn fixture(base: &Path) -> PathBuf {
    let repo = base.join("fixture");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-b", "trunk"]);
    git(&repo, &["config", "user.name", "Fixture"]);
    git(&repo, &["config", "user.email", "fixture@example.test"]);
    std::fs::write(repo.join("README.md"), "fixture\n").unwrap();
    git(&repo, &["add", "README.md"]);
    git(&repo, &["commit", "-m", "fixture"]);
    repo
}

#[tokio::test]
async fn init_creates_a_real_initial_commit_and_excludes_worktrees() {
    let home = root("init");
    let receipt = prepare(
        &home,
        true,
        &WorkspaceRequest::new(
            "workspace:wks_notes:prepare:v1",
            "wks_notes",
            "notes",
            WorkspaceSource::Init,
        ),
    )
    .await
    .unwrap();
    assert_eq!(receipt.name, "notes");
    assert_eq!(receipt.resolved_commit.len(), 40);
    assert_eq!(std::fs::read(home.join("notes/.gitkeep")).unwrap(), b"");
    assert!(
        std::fs::read_to_string(home.join("notes/.git/info/exclude"))
            .unwrap()
            .lines()
            .any(|line| line == ".worktrees/")
    );
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn clone_uses_a_real_local_repository_in_test_mode() {
    let home = root("clone");
    let source = fixture(&home);
    let receipt = prepare(
        &home,
        true,
        &WorkspaceRequest::new(
            "workspace:wks_copy:prepare:v1",
            "wks_copy",
            "copy",
            WorkspaceSource::Clone {
                url: source.to_string_lossy().into_owned(),
                reference: Some("trunk".into()),
            },
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(home.join("copy/README.md")).unwrap(),
        "fixture\n"
    );
    assert_eq!(receipt.resolved_commit.len(), 40);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn production_clone_urls_are_public_github_repositories_only() {
    assert!(validate_clone_url("https://github.com/owner/repository", false).is_ok());
    for value in [
        "http://github.com/owner/repository",
        "https://user@github.com/owner/repository",
        "https://github.com:443/owner/repository",
        "https://example.com/owner/repository",
        "https://github.com/owner/repository/issues",
        "file:///tmp/repository",
    ] {
        assert!(
            validate_clone_url(value, false).is_err(),
            "accepted {value}"
        );
    }
}

#[tokio::test]
async fn clone_depth_limits_the_actual_git_history() {
    let home = root("custom-depth");
    let source = fixture(&home);
    for _ in 0..3 {
        git(&source, &["commit", "--allow-empty", "-m", "next"]);
    }
    let request = WorkspaceRequest::with_clone_depth(
        "clone-custom",
        "wks-custom",
        "copy",
        WorkspaceSource::Clone {
            url: format!("file://{}", source.display()),
            reference: None,
        },
        2,
    );
    prepare(&home, true, &request).await.unwrap();
    let output = std::process::Command::new("git")
        .args(["rev-list", "--count", "HEAD"])
        .current_dir(home.join("copy"))
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "2");
    for depth in [0, 1_001] {
        assert!(validate_clone_depth(depth).is_err());
    }
    std::fs::remove_dir_all(home).unwrap();
}
