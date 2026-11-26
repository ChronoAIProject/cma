use super::*;
use layer3_adapter::{
    RuntimeLedgerObservation, RuntimeLedgerStatus, RuntimeStartupPolicyObservation,
    RuntimeStatusRequest, RUNTIME_LEDGER_STATUS_MEDIA_TYPE,
    RUNTIME_STARTUP_POLICY_STATUS_MEDIA_TYPE,
};
use sha2::{Digest, Sha256};

struct Fixture {
    root: std::path::PathBuf,
    config: RuntimeConfig,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("cma-ledger-observe-{}", uuid::Uuid::new_v4()));
        let mut config = RuntimeConfig::for_test(root.join("home"));
        config.home_identity = Some("a".repeat(43));
        config.state_root = root.join("state");
        config.checkpoint_root = root.join("transfer/cma-checkpoints");
        config.process_env.insert(
            "TMPDIR".into(),
            root.join("transfer").to_str().unwrap().into(),
        );
        for path in [&config.home, &config.state_root, &root.join("transfer")] {
            std::fs::create_dir_all(path).unwrap();
        }
        Self { root, config }
    }
    fn request(&self) -> RuntimeStatusRequest {
        RuntimeStatusRequest {
            sandbox_id: self.config.sandbox_id.clone(),
            generation: self.config.generation,
            home_identity: self.config.home_identity.clone().unwrap(),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

#[tokio::test]
async fn startup_policy_status_returns_effective_policy_only_when_requested() {
    for enabled in [false, true] {
        let mut fixture = Fixture::new();
        fixture.config.legacy_journal_recovery_enabled = enabled;
        let supervisor = Supervisor::start(fixture.config.clone(), SupervisorBoundary::default())
            .await
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = crate::link::router(Arc::clone(&supervisor));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let url = format!("http://{address}/runtime/control");
        let request = fixture.request();
        let frame = RequestFrame::RuntimeStatus(request.clone());
        let baseline = client.post(&url).json(&frame).send().await.unwrap();
        assert_eq!(baseline.headers()["content-type"], "application/json");
        let baseline = baseline.bytes().await.unwrap();
        assert_eq!(
            RuntimeStartupPolicyObservation::decode(&baseline, &request).unwrap(),
            None
        );
        let ResponseFrame::RuntimeStatus(baseline) = serde_json::from_slice(&baseline).unwrap()
        else {
            panic!("expected ordinary runtime status");
        };

        let observation = client
            .post(&url)
            .header("accept", RUNTIME_STARTUP_POLICY_STATUS_MEDIA_TYPE)
            .json(&frame)
            .send()
            .await
            .unwrap();
        assert_eq!(
            observation.headers()["content-type"],
            RUNTIME_STARTUP_POLICY_STATUS_MEDIA_TYPE
        );
        let observation = observation.bytes().await.unwrap();
        let readback = RuntimeStartupPolicyObservation::decode(&observation, &request)
            .unwrap()
            .unwrap();
        assert_eq!(readback.runtime, baseline);
        assert_eq!(readback.legacy_journal_recovery_enabled, enabled);

        let mut wrong = request;
        wrong.generation += 1;
        assert!(RuntimeStartupPolicyObservation::decode(&observation, &wrong).is_err());
        let refused: ResponseFrame = client
            .post(&url)
            .header("accept", RUNTIME_STARTUP_POLICY_STATUS_MEDIA_TYPE)
            .json(&RequestFrame::RuntimeStatus(wrong))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(matches!(refused, ResponseFrame::Failure(_)));
        server.abort();
        let _ = server.await;
    }
}

#[tokio::test]
async fn observation_is_negotiated_read_only_and_does_not_wait_for_the_effect_gate() {
    let fixture = Fixture::new();
    let supervisor = Supervisor::start(fixture.config.clone(), SupervisorBoundary::default())
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = crate::link::router(Arc::clone(&supervisor));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let held = supervisor.operation_gate.lock().await;
    let database = fixture
        .config
        .state_root
        .join("mutation-ledger-v2/ledger.sqlite3");
    let before = Sha256::digest(std::fs::read(&database).unwrap());
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .unwrap();
    let mut baseline = None;
    for accept in [
        "application/json",
        RUNTIME_LEDGER_STATUS_MEDIA_TYPE,
        "application/vnd.cma.runtime-ledger-status.v1+json;q=0",
        "application/json",
    ] {
        let response = client
            .post(format!("http://{address}/runtime/control"))
            .header("accept", accept)
            .json(&RequestFrame::RuntimeStatus(fixture.request()))
            .send()
            .await
            .unwrap();
        let supported = accept == RUNTIME_LEDGER_STATUS_MEDIA_TYPE;
        assert_eq!(
            response.headers()["content-type"],
            if supported {
                RUNTIME_LEDGER_STATUS_MEDIA_TYPE
            } else {
                "application/json"
            }
        );
        let bytes = response.bytes().await.unwrap();
        if let Some(observation) =
            RuntimeLedgerObservation::decode(&bytes, &fixture.request()).unwrap()
        {
            assert!(supported);
            let RuntimeLedgerStatus::Indexed { counters } = observation.ledger else {
                panic!("expected indexed counters")
            };
            assert_eq!(counters.operations, 0);
            assert_eq!(counters.maximum_bytes, 1024 * 1024 * 1024);
            assert_eq!(counters.emergency_remaining_bytes, 16 * 1024 * 1024);
            assert_eq!(counters.normal_outstanding_limit, 1024);
        } else {
            assert!(!supported);
            if let Some(original) = &baseline {
                assert_eq!(original, &bytes);
            } else {
                baseline = Some(bytes);
            }
        }
        assert_eq!(Sha256::digest(std::fs::read(&database).unwrap()), before);
    }
    // Corrupt format evidence is an explicit hold, never invented zero counters.
    std::fs::remove_file(
        fixture
            .config
            .checkpoint_root
            .join("runtime-state/mutation-ledger-v2.required.json"),
    )
    .unwrap();
    let value: ResponseFrame = client
        .post(format!("http://{address}/runtime/control"))
        .header("accept", RUNTIME_LEDGER_STATUS_MEDIA_TYPE)
        .json(&RequestFrame::RuntimeStatus(fixture.request()))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        matches!(value, ResponseFrame::Failure(ref f) if f.code == "mutation_ledger_format_hold")
    );
    drop(held);
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn legacy_observation_reports_migration_required_without_recovering_a_temporary() {
    let fixture = Fixture::new();
    let supervisor =
        Supervisor::new(fixture.config.clone(), SupervisorBoundary::default()).unwrap();
    let path = fixture.config.state_root.join("runtime-mutations");
    std::fs::create_dir(&path).unwrap();
    let temporary = path.join(".unpublished.tmp");
    std::fs::write(&temporary, b"unpublished bytes").unwrap();
    let status = supervisor.runtime_status(fixture.request()).await.unwrap();
    let held = supervisor.operation_gate.lock().await;
    let observation = supervisor.ledger_observation(status).await.unwrap();
    assert_eq!(
        observation.ledger,
        RuntimeLedgerStatus::LegacyMigrationRequired
    );
    assert_eq!(std::fs::read(temporary).unwrap(), b"unpublished bytes");
    drop(held);
}
