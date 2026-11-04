use super::*;
use std::path::{Path, PathBuf};

#[test]
fn inherited_git_environment_is_removed() {
    assert!(!retain_inherited_environment("GIT_DIR"));
    assert!(!retain_inherited_environment("GIT_CONFIG_COUNT"));
    assert!(retain_inherited_environment("PATH"));
    assert!(retain_inherited_environment("HTTPS_PROXY"));
}

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("cma-git-budget-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    async fn wait_started(&self) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while std::fs::read_to_string(self.0.join("started"))
                .ok()
                .and_then(|value| value.trim().parse::<i32>().ok())
                .is_none()
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("real Git subprocess did not start");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct RunningGit<T> {
    task: tokio::task::JoinHandle<Result<T>>,
    process_group_file: PathBuf,
}

impl<T> Drop for RunningGit<T> {
    fn drop(&mut self) {
        if !self.task.is_finished() {
            // A failed assertion or outer test timeout must not detach the real
            // shell alias. It runs in the GitRunner's separate process group.
            if let Some(group) = std::fs::read_to_string(&self.process_group_file)
                .ok()
                .and_then(|value| value.trim().parse::<i32>().ok())
                .filter(|group| *group > 1)
            {
                #[cfg(unix)]
                unsafe {
                    if group != libc::getpgrp() {
                        libc::kill(-group, libc::SIGKILL);
                    }
                }
            }
            self.task.abort();
        }
    }
}

fn start(path: &Path, operation: &'static str) -> RunningGit<GitOutput> {
    let path = path.to_owned();
    let process_group_file = path.join("started");
    let task = tokio::spawn(async move {
        // A real Git subprocess with controlled progress, without changing PATH
        // or process-global configuration shared by concurrent runtime tests.
        let args = [
            "-c",
            "alias.cma-budget=!ps -o pgid= -p $$ > started; while ! test -f release; do sleep 0.01; done; printf complete",
            "cma-budget",
        ];
        let runner = GitRunner::new(false);
        match operation {
            "clone workspace" => runner.checked_clone(&path, args).await,
            "add worktree" => runner.checked_checkout(&path, args, operation).await,
            _ => runner.checked(&path, args, operation).await,
        }
    });
    RunningGit {
        task,
        process_group_file,
    }
}

#[tokio::test]
async fn checkouts_can_finish_after_the_ordinary_git_deadline() {
    for operation in ["clone workspace", "add worktree"] {
        let fixture = Fixture::new();
        let mut running = start(&fixture.0, operation);
        fixture.wait_started().await;
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(121)).await;
        tokio::task::yield_now().await;
        assert!(
            !running.task.is_finished(),
            "{operation} retained the ordinary 120s deadline"
        );
        std::fs::write(fixture.0.join("release"), b"").unwrap();
        tokio::time::resume();
        let output = tokio::time::timeout(Duration::from_secs(10), &mut running.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(output.stdout, "complete");
    }
}

#[tokio::test]
async fn ordinary_git_operations_keep_the_shorter_deadline() {
    let fixture = Fixture::new();
    let mut running = start(&fixture.0, "test operation");
    fixture.wait_started().await;
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(121)).await;
    tokio::task::yield_now().await;
    tokio::time::resume();
    let error = tokio::time::timeout(Duration::from_secs(10), &mut running.task)
        .await
        .unwrap()
        .unwrap()
        .err()
        .expect("ordinary Git operation exceeded its budget without failing");
    assert_eq!(error.code, "git_timeout");
    assert!(error.detail.contains("120 seconds"));
}

#[tokio::test]
async fn checkouts_still_have_a_finite_deadline_and_terminate_the_process() {
    for operation in ["clone workspace", "add worktree"] {
        let fixture = Fixture::new();
        let mut running = start(&fixture.0, operation);
        fixture.wait_started().await;
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(601)).await;
        tokio::task::yield_now().await;
        tokio::time::resume();
        let error = tokio::time::timeout(Duration::from_secs(10), &mut running.task)
            .await
            .unwrap()
            .unwrap()
            .err()
            .expect("checkout exceeded its bounded budget without failing");
        assert_eq!(error.code, "git_timeout");
        assert!(error.detail.contains("600 seconds"));
        assert!(error.detail.contains(operation));
    }
}

#[tokio::test]
async fn actual_worktree_checkout_can_finish_after_two_minutes() {
    use layer3_adapter::{WorkspaceRequest, WorkspaceSource, WorktreeRequest};

    let fixture = Fixture::new();
    crate::workspace_source::prepare(
        &fixture.0,
        true,
        &WorkspaceRequest::new(
            "workspace:wks_checkout_budget:prepare:v1",
            "wks_checkout_budget",
            "project",
            WorkspaceSource::Init,
        ),
    )
    .await
    .unwrap();
    let repository = fixture.0.join("project");
    let git = GitRunner::new(true);
    std::fs::write(
        repository.join(".gitattributes"),
        "checkout.txt filter=cma-budget\n",
    )
    .unwrap();
    std::fs::write(repository.join("checkout.txt"), "complete checkout\n").unwrap();
    git.checked(
        &repository,
        ["add", ".gitattributes", "checkout.txt"],
        "stage fixture",
    )
    .await
    .unwrap();
    git.checked(
        &repository,
        [
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-m",
            "checkout fixture",
        ],
        "commit fixture",
    )
    .await
    .unwrap();
    let base = git
        .checked(&repository, ["rev-parse", "HEAD"], "read fixture commit")
        .await
        .unwrap()
        .stdout;
    let quote = |path: &Path| format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"));
    let smudge = format!(
        "ps -o pgid= -p $$ > {}; while ! test -f {}; do sleep 0.01; done; cat",
        quote(&fixture.0.join("started")),
        quote(&fixture.0.join("release")),
    );
    git.checked(
        &repository,
        ["config", "filter.cma-budget.smudge", &smudge],
        "delay fixture checkout",
    )
    .await
    .unwrap();
    let home = fixture.0.clone();
    let expected_base = base.clone();
    let mut running = RunningGit {
        task: tokio::spawn(async move {
            crate::worktree::add(
                &home,
                true,
                &WorktreeRequest::new(
                    "worktree:project:agent-slow:add:v1",
                    "project",
                    "agent-slow",
                    base,
                ),
            )
            .await
        }),
        process_group_file: fixture.0.join("started"),
    };
    fixture.wait_started().await;
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(121)).await;
    tokio::task::yield_now().await;
    assert!(
        !running.task.is_finished(),
        "actual worktree addition kept the ordinary Git deadline"
    );
    std::fs::write(fixture.0.join("release"), b"").unwrap();
    tokio::time::resume();
    let receipt = tokio::time::timeout(Duration::from_secs(10), &mut running.task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(receipt.base_commit, expected_base);
    assert_eq!(receipt.branch, "cma/agent-slow");
    assert_eq!(
        std::fs::read_to_string(Path::new(&receipt.path).join("checkout.txt")).unwrap(),
        "complete checkout\n"
    );
}
