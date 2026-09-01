use std::path::PathBuf;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::response::IntoResponse as _;

use super::{run_with_deadline, WorkspaceSetupRequest, MAX_OUTPUT_BYTES, TIMEOUT_EXIT_CODE};

fn root(name: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "cma-workspace-setup-{}-{sequence}-{name}",
        std::process::id()
    ))
}

fn restored_repository(name: &str) -> (PathBuf, PathBuf) {
    let workspace = root(name);
    let repository = workspace.join("project");
    std::fs::create_dir_all(repository.join(".git")).expect("the restored repository is created");
    (workspace, repository)
}

#[tokio::test]
async fn commands_run_in_repository_order_and_stop_at_the_first_failure() {
    let (workspace, repository) = restored_repository("order");
    let commands = vec![
        "printf 'one\\n' >> order; printf 'first-out\\n'; printf 'first-err\\n' >&2; pwd"
            .into(),
        "test \"$(cat order)\" = one; printf 'two\\n' >> order; printf 'failed-out\\n'; printf 'failed-err\\n' >&2; exit 17"
            .into(),
        "printf 'must-not-run\\n' >> order".into(),
    ];

    let receipt = run_with_deadline(&repository, &commands, Duration::from_secs(5))
        .await
        .expect("the bounded setup sequence runs");

    assert_eq!(receipt.failing_index, Some(1));
    assert_eq!(receipt.steps.len(), 2);
    assert_eq!(receipt.steps[0].exit_code, 0);
    assert!(receipt.steps[0].output_excerpt.contains("first-out"));
    assert!(receipt.steps[0].output_excerpt.contains("first-err"));
    assert!(receipt.steps[0]
        .output_excerpt
        .contains(repository.to_string_lossy().as_ref()));
    assert_eq!(receipt.steps[1].exit_code, 17);
    assert!(receipt.steps[1].output_excerpt.contains("failed-out"));
    assert!(receipt.steps[1].output_excerpt.contains("failed-err"));
    assert_eq!(
        std::fs::read_to_string(repository.join("order")).unwrap(),
        "one\ntwo\n"
    );
    std::fs::remove_dir_all(workspace).unwrap();
}

#[tokio::test]
async fn combined_output_keeps_only_one_utf8_safe_32_kib_tail() {
    let (workspace, repository) = restored_repository("tail");
    let commands = vec![
        "printf 'HEAD-MARKER\\n'; i=0; while [ $i -lt 40000 ]; do printf A; i=$((i+1)); done; printf '\\n'; i=0; while [ $i -lt 40000 ]; do printf B >&2; i=$((i+1)); done; printf '\\nTAIL-MARKER\\n' >&2"
            .into(),
    ];

    let receipt = run_with_deadline(&repository, &commands, Duration::from_secs(5))
        .await
        .expect("large mixed output is drained without buffering it all");
    let excerpt = &receipt.steps[0].output_excerpt;

    assert_eq!(receipt.failing_index, None);
    assert!(excerpt.len() <= MAX_OUTPUT_BYTES);
    assert!(excerpt.contains("TAIL-MARKER"));
    assert!(!excerpt.contains("HEAD-MARKER"));
    assert!(std::str::from_utf8(excerpt.as_bytes()).is_ok());
    std::fs::remove_dir_all(workspace).unwrap();
}

#[tokio::test]
async fn deadline_returns_a_nonzero_step_and_kills_the_command_session() {
    let (workspace, repository) = restored_repository("deadline");
    let commands = vec![
        "printf 'started\\n'; sleep 10; printf late > late".into(),
        "printf next > next".into(),
    ];
    let started = Instant::now();

    let receipt = run_with_deadline(&repository, &commands, Duration::from_millis(50))
        .await
        .expect("a deadline is a typed command failure");

    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(receipt.failing_index, Some(0));
    assert_eq!(receipt.steps.len(), 1);
    assert_eq!(receipt.steps[0].exit_code, TIMEOUT_EXIT_CODE);
    assert!(receipt.steps[0].output_excerpt.contains("started"));
    assert!(!repository.join("late").exists());
    assert!(!repository.join("next").exists());
    std::fs::remove_dir_all(workspace).unwrap();
}

#[test]
fn request_reasserts_the_settings_collection_and_byte_bounds() {
    assert!(WorkspaceSetupRequest {
        commands: vec![String::new(); 16]
    }
    .validate()
    .is_ok());
    assert!(WorkspaceSetupRequest {
        commands: vec![String::new(); 17]
    }
    .validate()
    .is_err());
    assert!(WorkspaceSetupRequest {
        commands: vec!["x".repeat(4097)]
    }
    .validate()
    .is_err());
}

#[tokio::test]
async fn public_route_uses_a_restored_repository_and_latches_first_conversation() {
    let (workspace, repository) = restored_repository("route");
    std::fs::write(repository.join("lean-toolchain"), "v4.31.0\n").unwrap();
    let mut state = crate::AppState::for_test(&[]);
    state.workspace_root = workspace.clone();
    let server_state = state.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the setup route fixture binds");
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, crate::runtime_router(server_state))
            .await
            .expect("the setup route fixture serves");
    });
    let client = reqwest::Client::new();

    let response = client
        .post(format!("http://{address}/runtime/workspace/setup"))
        .json(&serde_json::json!({
            "commands": ["pwd; cat lean-toolchain", "printf ready > setup-marker"]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let receipt: serde_json::Value = response.json().await.unwrap();
    assert_eq!(receipt["failing_index"], serde_json::Value::Null);
    assert_eq!(receipt["steps"].as_array().unwrap().len(), 2);
    assert!(receipt["steps"][0]["output_excerpt"]
        .as_str()
        .unwrap()
        .contains(repository.to_string_lossy().as_ref()));
    assert_eq!(
        std::fs::read_to_string(repository.join("setup-marker")).unwrap(),
        "ready"
    );

    state.conversation.lock().await.ever_started = true;
    let refused = client
        .post(format!("http://{address}/runtime/workspace/setup"))
        .json(&serde_json::json!({ "commands": ["printf forbidden"] }))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), axum::http::StatusCode::CONFLICT);

    server.abort();
    let _ = server.await;
    std::fs::remove_dir_all(workspace).unwrap();
}

#[tokio::test]
async fn setup_waits_for_the_startup_gate_and_a_fresh_incarnation_is_admitted() {
    let (workspace, repository) = restored_repository("startup-gate");
    let mut state = crate::AppState::for_test(&[]);
    state.workspace_root = workspace.clone();
    let held = state.startup_gate.read().await;
    let task_state = state.clone();
    let setup = tokio::spawn(async move {
        crate::workspace_setup_run(
            State(task_state),
            axum::Json(WorkspaceSetupRequest {
                commands: vec!["printf admitted > gate-marker".into()],
            }),
        )
        .await
        .into_response()
    });
    tokio::task::yield_now().await;
    assert!(!setup.is_finished());
    assert!(!repository.join("gate-marker").exists());

    drop(held);
    let response = tokio::time::timeout(Duration::from_secs(2), setup)
        .await
        .expect("setup proceeds when startup admission is released")
        .expect("the setup task joins");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert_eq!(
        std::fs::read_to_string(repository.join("gate-marker")).unwrap(),
        "admitted"
    );
    std::fs::remove_dir_all(workspace).unwrap();
}
