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

struct RunningGit {
    task: tokio::task::JoinHandle<Result<GitOutput>>,
    process_group_file: PathBuf,
}

impl Drop for RunningGit {
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

fn start(path: &Path, cloning: bool) -> RunningGit {
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
        if cloning {
            runner.checked_clone(&path, args).await
        } else {
            runner.checked(&path, args, "test operation").await
        }
    });
    RunningGit {
        task,
        process_group_file,
    }
}

#[tokio::test]
async fn clone_can_finish_after_the_ordinary_git_deadline() {
    let fixture = Fixture::new();
    let mut running = start(&fixture.0, true);
    fixture.wait_started().await;
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(121)).await;
    tokio::task::yield_now().await;
    assert!(
        !running.task.is_finished(),
        "clone retained the ordinary 120s deadline"
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

#[tokio::test]
async fn ordinary_git_operations_keep_the_shorter_deadline() {
    let fixture = Fixture::new();
    let mut running = start(&fixture.0, false);
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
async fn clone_still_has_a_finite_deadline_and_terminates_the_process() {
    let fixture = Fixture::new();
    let mut running = start(&fixture.0, true);
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
        .expect("clone exceeded its bounded budget without failing");
    assert_eq!(error.code, "git_timeout");
    assert!(error.detail.contains("600 seconds"));
}
