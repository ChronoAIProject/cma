use super::*;
use axum::{body::Body, http::Request, routing::get, Router};
use layer3_adapter::runtime_bootstrap::RUNTIME_BOOTSTRAP_VERSION;
use tower::ServiceExt;

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        // macOS temporary directories leave little room in sockaddr_un.sun_path.
        Self(std::env::temp_dir().join(format!(
            "cmab-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..16]
        )))
    }
    fn socket(&self) -> PathBuf {
        self.0.join("control.sock")
    }
    fn request(&self) -> RuntimeBootstrapRequest {
        RuntimeBootstrapRequest {
            version: RUNTIME_BOOTSTRAP_VERSION,
            staging_startup_failure: None,
            staging_startup_confirmed: false,
            pod_uid: "original-pod".into(),
            environment: BTreeMap::from([
                ("CHRONO_SANDBOX_ID".into(), "sbx_claimed".into()),
                ("CHRONO_RUNTIME_GENERATION".into(), "3".into()),
                ("CHRONO_RUNTIME_HOME_IDENTITY".into(), "a".repeat(43)),
                ("CHRONO_RUNTIME_CONTROL_TOKEN".into(), "b".repeat(64)),
                (
                    "CHRONO_RUNTIME_HOME".into(),
                    self.0.join("home").to_string_lossy().into_owned(),
                ),
                (
                    "CHRONO_RUNTIME_STATE_ROOT".into(),
                    self.0.join("state").to_string_lossy().into_owned(),
                ),
                (
                    "TMPDIR".into(),
                    self.0.join("transfer").to_string_lossy().into_owned(),
                ),
                ("CHRONO_RUNTIME_BIND".into(), "127.0.0.1:0".into()),
            ]),
        }
    }
    async fn start(&self) -> (AssignmentServer, oneshot::Receiver<AssignedRuntime>) {
        AssignmentServer::start(self.socket(), "original-pod".into(), BTreeMap::new())
            .await
            .unwrap()
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn exact_pod_assignment_is_replayable_and_keeps_control_credentials_out_of_child_environments(
) {
    let directory = Directory::new();
    let (server, mut ready) = directory.start().await;
    assert!(matches!(
        ready.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    let request = directory.request();
    let first = assign(&directory.socket(), &request).await.unwrap();
    let assigned = ready.await.unwrap();
    assert_eq!(assigned.config.sandbox_id, "sbx_claimed");
    assert_eq!(assigned.config.generation, 3);
    assert_eq!(assigned.config.home_identity, Some("a".repeat(43)));
    assert_eq!(
        assigned.config.checkpoint_root,
        directory.0.join("transfer/cma-checkpoints")
    );
    assert!(!assigned
        .config
        .process_env
        .contains_key("CHRONO_RUNTIME_CONTROL_TOKEN"));
    assert!(!assigned
        .config
        .install_env
        .contains_key("CHRONO_RUNTIME_CONTROL_TOKEN"));
    assert_eq!(
        assigned.config.process_env[crate::bridge::STALE_TURN_SEAL_FLAG],
        "1"
    );
    assert_eq!(
        first,
        RuntimeBootstrapReceipt::for_request(&request).unwrap()
    );
    assert_eq!(first, assign(&directory.socket(), &request).await.unwrap());
    let app = Router::new()
        .route("/control", get(|| async { "private" }))
        .layer(axum::middleware::from_fn_with_state(
            assigned.auth,
            crate::control_auth::authenticate,
        ));
    for (token, expected) in [("wrong".into(), 401), ("b".repeat(64), 200)] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/control")
                    .header("x-cma-runtime-token", token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), expected);
    }
    let entries = std::fs::read_dir(&directory.0)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        entries.len(),
        1,
        "assignment must not persist token or configuration files"
    );
    assert_eq!(
        std::fs::metadata(directory.socket())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    drop(server);
    assert!(!directory.socket().exists());
}

#[tokio::test]
async fn assignment_preserves_seal_opt_out_and_machine_default() {
    let flag = crate::bridge::STALE_TURN_SEAL_FLAG;
    for (name, value, expected) in [(flag, "0", Some("0")), ("CMA_MACHINE_INCLUDED", "1", None)] {
        let directory = Directory::new();
        let (_server, ready) = directory.start().await;
        let mut request = directory.request();
        request.environment.insert(name.into(), value.into());
        assign(&directory.socket(), &request).await.unwrap();
        let assigned = ready.await.unwrap();
        assert_eq!(
            assigned.config.process_env.get(flag).map(String::as_str),
            expected
        );
    }
}

#[tokio::test]
async fn wrong_pod_invalid_configuration_and_conflicting_assignments_cannot_replace_the_winner() {
    let directory = Directory::new();
    let (_server, mut ready) = directory.start().await;
    let original = directory.request();
    let mut wrong = original.clone();
    wrong.pod_uid = "replacement-pod".into();
    assert!(assign(&directory.socket(), &wrong).await.is_err());
    wrong = original.clone();
    wrong
        .environment
        .insert("CHRONO_RUNTIME_BIND".into(), "invalid".into());
    assert!(assign(&directory.socket(), &wrong).await.is_err());
    assert!(matches!(
        ready.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    let mut other = original.clone();
    other
        .environment
        .insert("CHRONO_SANDBOX_ID".into(), "sbx_other".into());
    let path = directory.socket();
    let (first, second) = tokio::join!(assign(&path, &original), assign(&path, &other));
    assert_ne!(first.is_ok(), second.is_ok());
    let assigned = ready.await.unwrap();
    assert_eq!(
        assigned.config.sandbox_id,
        if first.is_ok() {
            "sbx_claimed"
        } else {
            "sbx_other"
        }
    );
    let winner = if first.is_ok() { &original } else { &other };
    assert!(assign(&path, winner).await.is_ok());
}

#[tokio::test]
async fn lost_assignment_response_replays_without_reinitializing_or_persisting_secrets() {
    let directory = Directory::new();
    let (_server, ready) = directory.start().await;
    let request = directory.request();
    let mut stream = UnixStream::connect(directory.socket()).await.unwrap();
    stream
        .write_all(&serde_json::to_vec(&request).unwrap())
        .await
        .unwrap();
    stream.shutdown().await.unwrap();
    drop(stream);
    let assigned = ready.await.unwrap();
    assert_eq!(assigned.config.sandbox_id, "sbx_claimed");
    assign(&directory.socket(), &request)
        .await
        .unwrap()
        .validate_for(&request)
        .unwrap();
}

#[tokio::test]
async fn oversized_assignment_is_refused_without_waiting_for_eof_and_does_not_consume_authority() {
    let directory = Directory::new();
    let (_server, mut ready) = directory.start().await;
    let mut stream = UnixStream::connect(directory.socket()).await.unwrap();
    stream
        .write_all(&vec![b'x'; MAX_RUNTIME_BOOTSTRAP_BYTES + 1])
        .await
        .unwrap();
    let bytes = tokio::time::timeout(Duration::from_secs(2), read_bounded(stream, 1024))
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        serde_json::from_slice::<Reply>(&bytes).unwrap(),
        Reply::Rejected
    ));
    assert!(matches!(
        ready.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    assign(&directory.socket(), &directory.request())
        .await
        .unwrap();
}

#[tokio::test]
async fn live_socket_regular_files_and_symlinks_cannot_be_replaced_and_stale_socket_can_recover() {
    let directory = Directory::new();
    let (server, _) = directory.start().await;
    assert!(
        AssignmentServer::start(directory.socket(), "original-pod".into(), BTreeMap::new())
            .await
            .is_err()
    );
    drop(server);
    std::fs::write(directory.socket(), "keep").unwrap();
    assert!(
        AssignmentServer::start(directory.socket(), "original-pod".into(), BTreeMap::new())
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(directory.socket()).unwrap(), b"keep");
    std::fs::remove_file(directory.socket()).unwrap();
    std::os::unix::fs::symlink(directory.0.join("target"), directory.socket()).unwrap();
    assert!(
        AssignmentServer::start(directory.socket(), "original-pod".into(), BTreeMap::new())
            .await
            .is_err()
    );
    std::fs::remove_file(directory.socket()).unwrap();
    drop(UnixListener::bind(directory.socket()).unwrap());
    let (_recovered, _) = directory.start().await;
}

#[tokio::test]
async fn warm_start_requires_downward_pod_identity_and_no_existing_tenant_or_control_token() {
    let directory = Directory::new();
    for (uid, environment) in [
        ("", BTreeMap::new()),
        (
            "original-pod",
            BTreeMap::from([("CHRONO_SANDBOX_ID".into(), "existing".into())]),
        ),
        (
            "original-pod",
            BTreeMap::from([("CHRONO_RUNTIME_CONTROL_TOKEN".into(), "b".repeat(64))]),
        ),
    ] {
        assert!(
            AssignmentServer::start(directory.socket(), uid.into(), environment)
                .await
                .is_err()
        );
        assert!(!directory.socket().exists());
    }
}
