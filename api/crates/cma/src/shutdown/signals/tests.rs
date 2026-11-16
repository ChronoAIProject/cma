use super::*;

#[tokio::test]
async fn termination_is_latched_before_startup_completion_or_late_subscription() {
    let (stop, signal) = tokio::sync::oneshot::channel();
    let termination = Termination::observe(async {
        let _ = signal.await;
    });
    stop.send(()).unwrap();
    termination.cancelled().await;
    assert!(termination.requested());
    let late = termination.clone();
    let mut activated = false;
    let result = late
        .startup(async {
            activated = true;
            Ok(())
        })
        .await;
    assert!(result.is_err());
    assert!(!activated);
}

#[cfg(unix)]
#[tokio::test]
async fn real_sigterm_during_startup_is_latched_and_exits_without_activation() {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "shutdown::signals::tests::termination_during_startup_child",
            "--ignored",
            "--nocapture",
        ])
        .env("CMA_TEST_TERMINATION_CHILD", "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let pid = child.id().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if lines
                .next_line()
                .await
                .unwrap()
                .expect("child startup output")
                == "STARTUP_HANDLER_ARMED"
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    assert!(std::process::Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .unwrap()
        .success());
    let status = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(
        status.success(),
        "termination fixture must exit gracefully: {status}"
    );
    let mut output = String::new();
    while let Some(line) = lines.next_line().await.unwrap() {
        output.push_str(&line);
    }
    assert!(
        output.contains("STARTUP_CANCELLED_NO_ACTIVATION"),
        "{output}"
    );
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "subprocess fixture for real SIGTERM during startup"]
async fn termination_during_startup_child() {
    assert_eq!(
        std::env::var("CMA_TEST_TERMINATION_CHILD").as_deref(),
        Ok("1")
    );
    let termination = Termination::install().unwrap();
    println!("STARTUP_HANDLER_ARMED");
    let result: Result<(), String> = termination.startup(std::future::pending()).await;
    assert!(result.is_err());
    assert!(termination.requested());
    let mut activated = false;
    assert!(termination
        .startup(async {
            activated = true;
            Ok(())
        })
        .await
        .is_err());
    assert!(!activated);
    println!("STARTUP_CANCELLED_NO_ACTIVATION");
}
