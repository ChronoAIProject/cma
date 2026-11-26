use std::net::SocketAddr;
use std::time::Duration;

use layer3_adapter::{
    CheckpointRequest, CheckpointWriterResumeRequest, CredentialFrame, RestoreRequest,
    RuntimeCredential,
};

use super::*;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap()
}

async fn fixture(name: &str) -> (RuntimeConfig, Arc<Supervisor>) {
    let home =
        std::env::temp_dir().join(format!("cma-checkpoint-http-{}-{name}", std::process::id()));
    let config = RuntimeConfig::for_test(home);
    cleanup(&config);
    let supervisor = Supervisor::new(config.clone(), SupervisorBoundary::default()).unwrap();
    assert!(matches!(
        supervisor
            .dispatch(RequestFrame::Install(InstallRequest::new(
                "checkpoint-http-install",
                Vec::new(),
            )))
            .await,
        ResponseFrame::Install(_)
    ));
    assert!(matches!(
        supervisor
            .dispatch(RequestFrame::Credential(CredentialFrame {
                operation_id: "checkpoint-http-credential".into(),
                value: RuntimeCredential::new("test-credential"),
                custody_generation: Some(1),
                github_route: None,
            }))
            .await,
        ResponseFrame::Credential(_)
    ));
    std::fs::write(config.home.join("preserved"), "checkpoint-http-payload").unwrap();
    (config, supervisor)
}

fn cleanup(config: &RuntimeConfig) {
    let _ = std::fs::remove_dir_all(&config.home);
    let _ = std::fs::remove_dir_all(&config.checkpoint_root);
    let _ = std::fs::remove_dir_all(&config.state_root);
}

async fn serve(supervisor: Arc<Supervisor>) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, crate::link::router(supervisor))
            .await
            .unwrap();
    });
    (address, server)
}

async fn control(address: SocketAddr, request: &RequestFrame) -> ResponseFrame {
    reqwest::Client::new()
        .post(format!("http://{address}/runtime/control"))
        .json(request)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[test]
fn checkpoint_retirement_failure_keeps_hold_and_archive_until_exact_retry() {
    runtime().block_on(async {
        let (config, supervisor) = fixture("retirement-retry").await;
        let (address, server) = serve(Arc::clone(&supervisor)).await;
        let hold = RequestFrame::CheckpointForTeardown(CheckpointHoldRequest {
            checkpoint: CheckpointRequest { epoch: 74, exclusions: Vec::new() },
            writer_resume_token: "a".repeat(64),
        });
        let first = control(address, &hold).await;
        let ResponseFrame::CheckpointForTeardown(receipt) = &first else {
            panic!("expected a held checkpoint");
        };
        assert_eq!(receipt.checkpoint.writers, layer3_adapter::WriterState::Held);
        let archive = config.checkpoint_root.join(&receipt.checkpoint.object_reference);
        let archive_size = std::fs::metadata(&archive).unwrap().len();
        let obstruction = config.checkpoint_root.join("checkpoint-writer-state");
        std::fs::write(&obstruction, b"not a directory").unwrap();
        let wrong = RequestFrame::ResumeCheckpointWriters(CheckpointWriterResumeRequest {
            writer_resume_token: "b".repeat(64),
        });
        assert!(matches!(control(address, &wrong).await,
            ResponseFrame::Failure(failure) if failure.code == "checkpoint_hold_conflict"));
        let resume = RequestFrame::ResumeCheckpointWriters(CheckpointWriterResumeRequest {
            writer_resume_token: "a".repeat(64),
        });
        for _ in 0..2 {
            assert!(matches!(control(address, &resume).await,
                ResponseFrame::Failure(failure) if failure.code == "checkpoint_writer_state_invalid"));
            assert_eq!(supervisor.held_checkpoint.lock().await.as_ref().unwrap().receipt, *receipt);
            assert_eq!(std::fs::metadata(&archive).unwrap().len(), archive_size);
            assert!(matches!(control(address, &hold).await,
                ResponseFrame::Failure(failure) if failure.code == "checkpoint_writer_state_invalid"));
            assert!(matches!(control(address, &RequestFrame::Checkpoint(CheckpointRequest {
                epoch: 75, exclusions: Vec::new(),
            })).await, ResponseFrame::Failure(failure) if failure.code == "checkpoint_writers_held"));
        }
        std::fs::remove_file(obstruction).unwrap();
        assert_eq!(control(address, &hold).await, first);
        let resumed = control(address, &resume).await;
        assert!(matches!(&resumed, ResponseFrame::ResumeCheckpointWriters(receipt)
            if receipt.writers_resumed && receipt.writer_resume_token == "a".repeat(64)));
        assert!(supervisor.held_checkpoint.lock().await.is_none());
        assert!(!archive.exists());
        assert_eq!(control(address, &resume).await, resumed);
        assert!(matches!(control(address, &hold).await,
            ResponseFrame::Failure(failure) if failure.code == "checkpoint_hold_retired"));
        server.abort();
        let _ = server.await;
        cleanup(&config);
    });
}

async fn pause_archive_worker() -> (std::sync::mpsc::Sender<()>, tokio::task::JoinHandle<()>) {
    let (release, waiting) = std::sync::mpsc::channel();
    let (started, observed) = tokio::sync::oneshot::channel();
    let task = tokio::task::spawn_blocking(move || {
        let _ = started.send(());
        let _ = waiting.recv();
    });
    observed.await.unwrap();
    (release, task)
}

async fn disconnect_during_operation(
    address: SocketAddr,
    supervisor: &Arc<Supervisor>,
    request: &RequestFrame,
) {
    let mut response = tokio::time::timeout(
        Duration::from_secs(2),
        reqwest::Client::new()
            .post(format!("http://{address}/runtime/control"))
            .json(request)
            .send(),
    )
    .await
    .expect("response headers do not wait for archive work")
    .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "application/json");
    assert_eq!(response.chunk().await.unwrap().unwrap().as_ref(), b"\n");
    tokio::time::timeout(Duration::from_secs(2), async {
        while supervisor.operation_gate.try_lock().is_ok() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("archive operation owns the mutation gate");
    drop(response);
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(
        supervisor.operation_gate.try_lock().is_err(),
        "HTTP disconnect must not release the gate while archive work remains"
    );
}

async fn assert_queued_until_archive_finishes(
    supervisor: &Arc<Supervisor>,
    release: std::sync::mpsc::Sender<()>,
    blocker: tokio::task::JoinHandle<()>,
) {
    let queued_supervisor = Arc::clone(supervisor);
    let mut queued =
        tokio::spawn(async move { queued_supervisor.dispatch(RequestFrame::Resources).await });
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut queued)
            .await
            .is_err(),
        "another operation cannot overlap the detached archive worker"
    );
    release.send(()).unwrap();
    blocker.await.unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(3), queued)
            .await
            .unwrap()
            .unwrap(),
        ResponseFrame::Resources(_)
    ));
}

#[test]
fn held_checkpoint_http_disconnect_retains_fence_and_exact_replay() {
    runtime().block_on(async {
        let (config, supervisor) = fixture("held").await;
        let (address, server) = serve(Arc::clone(&supervisor)).await;
        let hold = CheckpointHoldRequest {
            checkpoint: CheckpointRequest {
                epoch: 71,
                exclusions: Vec::new(),
            },
            writer_resume_token: "c".repeat(64),
        };
        let request = RequestFrame::CheckpointForTeardown(hold.clone());
        let (release, blocker) = pause_archive_worker().await;
        disconnect_during_operation(address, &supervisor, &request).await;
        assert!(supervisor.held_checkpoint.try_lock().is_err());
        assert_queued_until_archive_finishes(&supervisor, release, blocker).await;
        let first = supervisor
            .held_checkpoint
            .lock()
            .await
            .as_ref()
            .expect("detached capture retains its completed writer hold")
            .receipt
            .clone();
        let archive = config
            .checkpoint_root
            .join(&first.checkpoint.object_reference);
        assert!(archive.is_file(), "held capture must remain exportable");
        let replay = reqwest::Client::new()
            .post(format!("http://{address}/runtime/control"))
            .json(&request)
            .send()
            .await
            .unwrap()
            .json::<ResponseFrame>()
            .await
            .unwrap();
        assert_eq!(replay, ResponseFrame::CheckpointForTeardown(first));
        assert!(matches!(
            supervisor
                .dispatch(RequestFrame::ResumeCheckpointWriters(
                    CheckpointWriterResumeRequest {
                        writer_resume_token: hold.writer_resume_token,
                    },
                ))
                .await,
            ResponseFrame::ResumeCheckpointWriters(_)
        ));
        assert!(supervisor.held_checkpoint.lock().await.is_none());
        assert!(
            !archive.exists(),
            "writer resumption releases local archive storage"
        );
        server.abort();
        let _ = server.await;
        cleanup(&config);
    });
}

#[test]
fn checkpoint_http_disconnect_finishes_one_archive_before_unlocking() {
    runtime().block_on(async {
        let (config, supervisor) = fixture("ordinary").await;
        let (address, server) = serve(Arc::clone(&supervisor)).await;
        let request = RequestFrame::Checkpoint(CheckpointRequest {
            epoch: 72,
            exclusions: Vec::new(),
        });
        let (release, blocker) = pause_archive_worker().await;
        disconnect_during_operation(address, &supervisor, &request).await;
        assert_queued_until_archive_finishes(&supervisor, release, blocker).await;
        let entries = std::fs::read_dir(&config.checkpoint_root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            entries
                .iter()
                .filter(|name| name.starts_with("home-"))
                .count(),
            1
        );
        assert!(!entries.iter().any(|name| name.starts_with(".capture-")));
        assert!(supervisor.held_checkpoint.lock().await.is_none());
        server.abort();
        let _ = server.await;
        cleanup(&config);
    });
}

#[test]
fn restore_http_disconnect_keeps_gate_and_finishes_phase_transition() {
    runtime().block_on(async {
        let (source_config, source) = fixture("restore-source").await;
        let sealed = source
            .checkpoint_store()
            .capture(&CheckpointRequest {
                epoch: 73,
                exclusions: Vec::new(),
            })
            .await
            .unwrap();
        let restore = RestoreRequest {
            object_reference: sealed.object_reference,
            sha256: sealed.sha256,
        };
        let config = RuntimeConfig::for_test(std::path::PathBuf::from(format!(
            "{}-target",
            source_config.home.display()
        )));
        cleanup(&config);
        let supervisor = Supervisor::new(config.clone(), SupervisorBoundary::default()).unwrap();
        std::fs::create_dir_all(&config.checkpoint_root).unwrap();
        std::fs::copy(
            source_config
                .checkpoint_root
                .join(&restore.object_reference),
            config.checkpoint_root.join(&restore.object_reference),
        )
        .unwrap();
        assert!(matches!(
            supervisor
                .dispatch(RequestFrame::BootstrapRestore(
                    RestoreBootstrapRequest::new(
                        "checkpoint-http-bootstrap",
                        config.sandbox_id.clone(),
                        restore.clone(),
                    )
                ))
                .await,
            ResponseFrame::BootstrapRestore(_)
        ));
        assert!(matches!(
            supervisor
                .dispatch(RequestFrame::Credential(CredentialFrame {
                    operation_id: "checkpoint-http-restored-credential".into(),
                    value: RuntimeCredential::new("test-credential"),
                    custody_generation: Some(2),
                    github_route: None,
                }))
                .await,
            ResponseFrame::Credential(_)
        ));
        std::fs::write(config.home.join("preserved"), "replace-me").unwrap();
        let (address, server) = serve(Arc::clone(&supervisor)).await;
        let (release, blocker) = pause_archive_worker().await;
        let archive = config.checkpoint_root.join(&restore.object_reference);
        disconnect_during_operation(address, &supervisor, &RequestFrame::Restore(restore)).await;
        assert_eq!(supervisor.phase().await, Phase::RestoreCredentialed);
        assert_queued_until_archive_finishes(&supervisor, release, blocker).await;
        assert_eq!(supervisor.phase().await, Phase::Credentialed);
        assert!(
            !archive.exists(),
            "successful restore releases its local transfer"
        );
        assert_eq!(
            std::fs::read_to_string(config.home.join("preserved")).unwrap(),
            "checkpoint-http-payload"
        );
        assert!(!std::fs::read_dir(&config.checkpoint_root)
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".restore-")));
        server.abort();
        let _ = server.await;
        cleanup(&config);
        cleanup(&source_config);
    });
}
