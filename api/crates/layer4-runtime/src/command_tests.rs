use std::collections::BTreeMap;
use std::time::Duration;

use sha2::{Digest as _, Sha256};

use super::*;

fn root(name: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("cma-command-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn limits(timeout: Duration, output_limit: usize) -> CommandLimits {
    CommandLimits {
        timeout,
        total_timeout: timeout,
        output_limit,
    }
}

#[test]
fn requested_total_deadline_is_validated_and_retained() {
    let custom = CommandLimits::install(2, 3, 1_024).unwrap();
    assert_eq!(custom.timeout, Duration::from_secs(2));
    assert_eq!(custom.total_timeout, Duration::from_secs(3));
    for (command, total) in [(0, 1), (601, 1_800), (2, 1), (1, 1_801)] {
        assert!(CommandLimits::install(command, total, 1_024).is_err());
    }
}

#[tokio::test]
async fn total_deadline_stops_the_batch_before_a_later_side_effect() {
    let root = root("total-deadline");
    let error = run_shell_commands(
        &[
            "sleep 0.08".into(),
            "sleep 0.08".into(),
            "touch should-not-exist".into(),
        ],
        &root,
        &BTreeMap::new(),
        CommandLimits {
            timeout: Duration::from_millis(100),
            total_timeout: Duration::from_millis(120),
            output_limit: 1_024,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "command_timeout");
    assert!(!root.join("should-not-exist").exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn shell_receipts_hash_both_bounded_streams() {
    let root = root("hashes");
    let receipts = run_shell_commands(
        &["printf stdout; printf stderr >&2".into()],
        &root,
        &BTreeMap::new(),
        limits(Duration::from_secs(2), 1024),
    )
    .await
    .unwrap();
    assert_eq!(receipts[0].index, 0);
    assert_eq!(receipts[0].stdout_sha256, hex(&Sha256::digest(b"stdout")));
    assert_eq!(receipts[0].stderr_sha256, hex(&Sha256::digest(b"stderr")));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn first_failure_stops_later_commands_and_names_its_index_and_code() {
    let root = root("stop-first");
    let error = run_shell_commands(
        &[
            "true".into(),
            "exit 17".into(),
            "touch should-not-exist".into(),
        ],
        &root,
        &BTreeMap::new(),
        limits(Duration::from_secs(2), 1024),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "command_failed");
    assert!(error.detail.contains("command 1"));
    assert!(error.detail.contains("code 17"));
    assert!(!root.join("should-not-exist").exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn output_and_time_are_hard_limits() {
    let root = root("bounds");
    let output = run_shell_commands(
        &["printf 12345".into()],
        &root,
        &BTreeMap::new(),
        limits(Duration::from_secs(2), 4),
    )
    .await
    .unwrap_err();
    assert_eq!(output.code, "command_output_limit");

    let timeout = run_shell_commands(
        &["sleep 5".into()],
        &root,
        &BTreeMap::new(),
        limits(Duration::from_millis(30), 1024),
    )
    .await
    .unwrap_err();
    assert_eq!(timeout.code, "command_timeout");
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn a_command_may_close_its_output_before_it_exits() {
    let root = root("closed-output");
    let receipts = run_shell_commands(
        &["exec 1>&-; exec 2>&-; sleep 0.05".into()],
        &root,
        &BTreeMap::new(),
        limits(Duration::from_secs(2), 1024),
    )
    .await
    .unwrap();
    assert_eq!(receipts[0].exit_code, 0);
    std::fs::remove_dir_all(root).unwrap();
}
