use super::*;
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, BufReader};

pub(crate) async fn child_with_open_output(
) -> (DesktopProcess, BufReader<tokio::process::ChildStdout>) {
    let mut command = Command::new("/bin/bash");
    command
        .args(["-c", "sleep 120 & echo started; wait"])
        .stdout(std::process::Stdio::piped());
    let mut desktop = DesktopProcess::spawn(command).unwrap();
    let mut output = BufReader::new(desktop.child.stdout.take().unwrap());
    let mut line = String::new();
    output.read_line(&mut line).await.unwrap();
    assert_eq!(line, "started\n");
    (desktop, output)
}

pub(crate) fn exiting_child(code: i32) -> DesktopProcess {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", &format!("exit {code}")]);
    DesktopProcess::spawn(command).unwrap()
}

pub(crate) async fn assert_child_output_closed(mut output: BufReader<tokio::process::ChildStdout>) {
    let mut rest = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        output.read_to_end(&mut rest),
    )
    .await
    .expect("desktop background child retained its output pipe")
    .unwrap();
}

#[tokio::test]
async fn dropping_desktop_closes_output_held_by_its_background_child() {
    let (desktop, output) = child_with_open_output().await;
    drop(desktop);
    assert_child_output_closed(output).await;
}

#[test]
fn desktop_supervision_requires_an_explicit_boolean() {
    let key = "CHRONO_RUNTIME_DESKTOP_SUPERVISION_ENABLED";
    let base = std::collections::BTreeMap::from([
        ("CHRONO_SANDBOX_ID".into(), "desktop-fixture".into()),
        ("CHRONO_RUNTIME_HOME".into(), "/home/cma".into()),
    ]);
    assert!(
        !RuntimeConfig::from_values(base.clone())
            .unwrap()
            .desktop_supervision_enabled
    );
    for (value, enabled) in [("false", false), ("true", true)] {
        let mut env = base.clone();
        env.insert(key.into(), value.into());
        assert_eq!(
            RuntimeConfig::from_values(env)
                .unwrap()
                .desktop_supervision_enabled,
            enabled
        );
    }
    for value in ["1", "TRUE", " true", ""] {
        let mut env = base.clone();
        env.insert(key.into(), value.into());
        assert!(RuntimeConfig::from_values(env).is_err());
    }
}
