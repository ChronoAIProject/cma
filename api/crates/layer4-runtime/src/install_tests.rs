use std::time::Duration;

use super::*;

fn config(name: &str) -> RuntimeConfig {
    let home = std::env::temp_dir().join(format!("cma-install-{}-{name}", std::process::id()));
    let config = RuntimeConfig::for_test(home);
    let _ = std::fs::remove_dir_all(&config.home);
    let _ = std::fs::remove_dir_all(&config.checkpoint_root);
    std::fs::create_dir_all(&config.home).unwrap();
    config
}

fn request(name: &str, command: &str) -> InstallRequest {
    InstallRequest::bounded(
        format!("sandbox-install-v1:{name}"),
        vec![command.into()],
        5,
        32 * 1024,
    )
}

fn cleanup(config: &RuntimeConfig) {
    std::fs::remove_dir_all(&config.home).unwrap();
    std::fs::remove_dir_all(&config.checkpoint_root).unwrap();
}

#[tokio::test]
async fn completed_install_replays_after_recovery_without_rerunning() {
    let config = config("completed-replay");
    let request = request("completed-replay", "printf 'ran\\n' >> runs");
    let first = run(&config, &request).await.unwrap();
    recover(&config).unwrap();
    let replay = run(&config, &request).await.unwrap();
    assert_eq!(replay, first);
    assert_eq!(
        std::fs::read_to_string(config.home.join("runs")).unwrap(),
        "ran\n"
    );
    cleanup(&config);
}

#[tokio::test]
async fn terminal_failure_is_replayed_without_repeating_partial_effects() {
    let config = config("failed-replay");
    let request = request("failed-replay", "printf 'ran\\n' >> runs; false");
    let first = run(&config, &request).await.unwrap_err();
    let replay = run(&config, &request).await.unwrap_err();
    assert_eq!(replay.code, first.code);
    assert_eq!(replay.detail, first.detail);
    assert!(!replay.retryable);
    assert_eq!(
        std::fs::read_to_string(config.home.join("runs")).unwrap(),
        "ran\n"
    );
    cleanup(&config);
}

#[tokio::test]
async fn changed_identity_never_reuses_or_repeats_a_completed_install() {
    let config = config("identity-mismatch");
    let original = request("identity-mismatch", "printf 'ran\\n' >> runs");
    run(&config, &original).await.unwrap();

    let renamed = request("renamed", "printf 'ran\\n' >> runs");
    let changed = request("identity-mismatch", "printf 'changed\\n' >> runs");
    assert_eq!(renamed.request_sha256, original.request_sha256);
    for mismatch in [&renamed, &changed] {
        let error = run(&config, mismatch).await.unwrap_err();
        assert_eq!(error.code, "install_replay_mismatch");
    }
    assert_eq!(
        std::fs::read_to_string(config.home.join("runs")).unwrap(),
        "ran\n"
    );
    cleanup(&config);
}

#[tokio::test]
async fn restarted_started_install_is_indeterminate_and_never_runs() {
    let config = config("indeterminate");
    let request = request("indeterminate", "touch must-not-run");
    let started = InstallRecord::Started {
        version: RECORD_VERSION,
        sandbox_id: config.sandbox_id.clone(),
        operation_id: request.operation_id.clone(),
        request_sha256: request.request_sha256.clone(),
    };
    assert!(write_initial_record(&config, &started).unwrap());
    recover(&config).unwrap();

    let error = run(&config, &request).await.unwrap_err();
    assert_eq!(error.code, "install_outcome_unknown");
    assert!(!error.retryable);
    assert!(!config.home.join("must-not-run").exists());
    cleanup(&config);
}

#[tokio::test]
async fn dropping_the_waiter_does_not_cancel_an_admitted_install() {
    let config = config("dropped-waiter");
    let request = request(
        "dropped-waiter",
        "touch started; sleep 0.1; printf 'ran\\n' >> runs",
    );
    let task_config = config.clone();
    let task_request = request.clone();
    let waiter = tokio::spawn(async move { run(&task_config, &task_request).await });
    wait_for(&config.home.join("started")).await;
    waiter.abort();
    wait_for(&config.home.join("runs")).await;
    wait_for_completion(&config).await;

    let replay = run(&config, &request).await.unwrap();
    assert_eq!(replay.operation_id, request.operation_id);
    assert_eq!(
        std::fs::read_to_string(config.home.join("runs")).unwrap(),
        "ran\n"
    );
    cleanup(&config);
}

#[test]
fn corrupt_journal_fails_closed_during_recovery() {
    let config = config("corrupt");
    let directory = ensure_state_directory(&config).unwrap();
    std::fs::write(directory.join(STATE_FILE), b"{truncated").unwrap();
    let error = recover(&config).unwrap_err();
    assert_eq!(error.code, "install_state_invalid");
    cleanup(&config);
}

async fn wait_for(path: &Path) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("install side effect appeared");
}

async fn wait_for_completion(config: &RuntimeConfig) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if matches!(
                read_record(config),
                Ok(Some(InstallRecord::Completed { .. }))
            ) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("install outcome became durable");
}
