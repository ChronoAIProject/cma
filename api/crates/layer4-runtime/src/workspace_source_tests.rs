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
        &WorkspaceRequest {
            name: "notes".into(),
            source: WorkspaceSource::Init,
        },
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
        &WorkspaceRequest {
            name: "copy".into(),
            source: WorkspaceSource::Clone {
                url: source.to_string_lossy().into_owned(),
                reference: Some("trunk".into()),
            },
        },
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
