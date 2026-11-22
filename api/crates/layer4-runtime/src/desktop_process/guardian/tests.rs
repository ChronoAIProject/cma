use std::path::{Path, PathBuf};

use super::*;
use crate::desktop_process::DesktopProcess;

const MANAGED: &str = "desktop_process::guardian::tests::guardian_preserves_an_unrelated_agent";
const CHILD: &str = "desktop_process::guardian::tests::desktop_guardian_fixture";

#[tokio::test]
#[ignore = "subprocess fixture for the managed desktop guardian test"]
async fn desktop_guardian_fixture() {
    let root = PathBuf::from(std::env::var_os("CMA_GUARDIAN_FIXTURE_ROOT").unwrap());
    let mut command = Command::new("/bin/sh");
    command.current_dir(&root).args([
        "-c",
        r#"
            setsid /bin/sh -c 'echo "$$" > escaped; exec sleep 120' &
            /bin/sh -c 'exit 0'
            touch browser-closed
            while [ ! -f component-exited ]; do sleep .02; done
            exit 8
            "#,
    ]);
    supervise(command, |event| {
        std::fs::write(
            root.join(if event == STOPPED {
                "stopped"
            } else {
                "reaped"
            }),
            b"",
        )
        .map_err(|error| error.to_string())
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn guardian_preserves_an_unrelated_agent() {
    if std::env::var_os("CMA_GUARDIAN_MANAGED_FIXTURE").is_none() {
        // The runtime's managed reaper is process-wide. Give it a process
        // that contains only this test's registered children.
        let mut command = Command::new(std::env::current_exe().unwrap());
        let result = tokio::time::timeout(
            Duration::from_secs(18),
            command
                .args(["--exact", MANAGED, "--nocapture"])
                .env("CMA_GUARDIAN_MANAGED_FIXTURE", "1")
                .kill_on_drop(true)
                .output(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    let _boundary = crate::writer_fence::SupervisorBoundary::install().unwrap();
    let mut agent = Command::new("sleep");
    agent.arg("120");
    let mut agent = DesktopProcess::spawn_in_group(agent, true).unwrap();
    for intentional_shutdown in [false, true] {
        let root = std::env::temp_dir().join(format!("cma-guardian-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", CHILD, "--ignored"])
            .env("CMA_GUARDIAN_FIXTURE_ROOT", &root)
            .stdin(Stdio::piped());
        let mut desktop = DesktopProcess::spawn_in_group(command, true).unwrap();
        let disconnected_helper = wait_for(&root.join("escaped")).await;
        let pid: i32 = std::str::from_utf8(&disconnected_helper)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        wait_for(&root.join("browser-closed")).await;
        assert!(!root.join("stopped").exists());
        assert_ne!(unsafe { libc::getsid(pid) }, unsafe {
            libc::getsid(desktop.child.id().unwrap() as i32)
        });
        if intentional_shutdown {
            desktop.child.stdin.take();
        } else {
            std::fs::write(root.join("component-exited"), b"").unwrap();
        }
        wait_for(&root.join("stopped")).await;
        wait_for(&root.join("reaped")).await;
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
        // The runtime's own reaper must leave the managed group leader for
        // its DesktopProcess even after it becomes a zombie.
        let outcome = tokio::time::timeout(Duration::from_secs(2), desktop.child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(outcome.success());
        assert!(agent.child.try_wait().unwrap().is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
    agent.child.kill().await.unwrap();
}

async fn wait_for(path: &Path) -> Vec<u8> {
    tokio::time::timeout(Duration::from_secs(6), async {
        loop {
            match std::fs::read(path) {
                Ok(contents) if !contents.is_empty() || !path.ends_with("escaped") => {
                    return contents
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => panic!("read desktop fixture signal: {error}"),
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    })
    .await
    .unwrap()
}
