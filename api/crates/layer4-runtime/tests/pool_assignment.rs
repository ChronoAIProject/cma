//! Exercise the shipped binary and stdio protocol, including retained-home restart.
use std::{collections::BTreeMap, path::PathBuf, process::Stdio, time::Duration};

use layer3_adapter::{
    runtime_bootstrap::{
        RuntimeBootstrapReceipt, RuntimeBootstrapRequest, RUNTIME_BOOTSTRAP_VERSION,
    },
    wire::{RequestFrame, ResponseFrame},
    InstallReceipt, InstallRequest, RuntimeStatus, RuntimeStatusRequest,
};
use tokio::{io::AsyncWriteExt, process::Command};

struct Fixture {
    root: PathBuf,
    address: std::net::SocketAddr,
}

impl Fixture {
    fn new() -> Self {
        // Model the real retained home/state/transfer mounts with canonical
        // roots. Indexed startup refuses symlink ancestors and absent mounts.
        let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "cmac-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..16]
        ));
        std::fs::create_dir_all(root.join("home")).unwrap();
        std::fs::create_dir_all(root.join("transfer")).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        Self {
            root,
            address: listener.local_addr().unwrap(),
        }
    }

    fn socket(&self) -> PathBuf {
        self.root.join("assign.sock")
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_layer4-runtime"));
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("CHRONO_RUNTIME_BOOTSTRAP_SOCKET", self.socket())
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    async fn waiting(&self, uid: &str) -> tokio::process::Child {
        let ambient = self.root.join(format!("ambient-{uid}"));
        let directory = ambient.join("cma-checkpoints/runtime-state");
        std::fs::create_dir_all(&directory).unwrap();
        let request = InstallRequest::new("image-build-install", vec![]);
        let receipt = InstallReceipt {
            operation_id: request.operation_id.clone(),
            request_sha256: request.request_sha256.clone(),
            commands: vec![],
        };
        // The parent starts with another runtime's completed image-local state.
        // Assignment must select retained transfer storage before reading it.
        std::fs::write(
            directory.join("install-v2.json"),
            serde_json::to_vec(&serde_json::json!({
                "state": "completed", "version": 2, "sandbox_id": "native-tools-fixture",
                "request": request, "receipt": receipt,
            }))
            .unwrap(),
        )
        .unwrap();
        let mut child = self
            .command()
            .arg("--wait-for-assignment")
            .env("CHRONO_RUNTIME_POD_UID", uid)
            .env("CHRONO_RUNTIME_BIND", self.address.to_string())
            .env("TMPDIR", ambient)
            .spawn()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            while tokio::net::UnixStream::connect(self.socket())
                .await
                .is_err()
            {
                assert!(
                    child.try_wait().unwrap().is_none(),
                    "waiting runtime exited"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        child
    }

    fn request(&self) -> RuntimeBootstrapRequest {
        RuntimeBootstrapRequest {
            version: RUNTIME_BOOTSTRAP_VERSION,
            staging_startup_failure: None,
            staging_startup_confirmed: false,
            pod_uid: "original-pod".into(),
            environment: BTreeMap::from([
                ("CHRONO_SANDBOX_ID".into(), "sbx_cli".into()),
                ("CHRONO_RUNTIME_GENERATION".into(), "1".into()),
                ("CHRONO_RUNTIME_HOME_IDENTITY".into(), "a".repeat(43)),
                ("CHRONO_RUNTIME_CONTROL_TOKEN".into(), "b".repeat(64)),
                ("CHRONO_RUNTIME_BIND".into(), self.address.to_string()),
                (
                    "CHRONO_RUNTIME_HOME".into(),
                    self.root.join("home").to_string_lossy().into_owned(),
                ),
                (
                    "CHRONO_RUNTIME_STATE_ROOT".into(),
                    self.root.join("state").to_string_lossy().into_owned(),
                ),
                (
                    "TMPDIR".into(),
                    self.root.join("transfer").to_string_lossy().into_owned(),
                ),
            ]),
        }
    }

    async fn assign(&self, request: &RuntimeBootstrapRequest, success: bool) {
        let mut child = self
            .command()
            .arg("--assign-runtime")
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(request).unwrap())
            .await
            .unwrap();
        let output = tokio::time::timeout(Duration::from_secs(15), child.wait_with_output())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(output.status.success(), success);
        assert_private(&output, request);
        if success {
            serde_json::from_slice::<RuntimeBootstrapReceipt>(&output.stdout)
                .unwrap()
                .validate_for(request)
                .unwrap();
        } else {
            assert!(output.stdout.is_empty());
        }
    }

    async fn status(&self, request: &RuntimeBootstrapRequest) -> RuntimeStatus {
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let root = format!("http://{}", self.address);
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if client.get(format!("{root}/livez")).send().await.is_ok() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        let frame = RequestFrame::RuntimeStatus(RuntimeStatusRequest {
            sandbox_id: request.environment["CHRONO_SANDBOX_ID"].clone(),
            generation: request.environment["CHRONO_RUNTIME_GENERATION"]
                .parse()
                .unwrap(),
            home_identity: request.environment["CHRONO_RUNTIME_HOME_IDENTITY"].clone(),
        });
        for token in [None, Some("wrong")] {
            let mut call = client.post(format!("{root}/runtime/control")).json(&frame);
            if let Some(token) = token {
                call = call.header("x-cma-runtime-token", token);
            }
            assert_eq!(call.send().await.unwrap().status().as_u16(), 401);
        }
        let response = client
            .post(format!("{root}/runtime/control"))
            .header(
                "x-cma-runtime-token",
                &request.environment["CHRONO_RUNTIME_CONTROL_TOKEN"],
            )
            .json(&frame)
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        let ResponseFrame::RuntimeStatus(status) = response.json().await.unwrap() else {
            panic!("expected authenticated runtime status");
        };
        status
    }

    async fn install(
        &self,
        assignment: &RuntimeBootstrapRequest,
        request: &InstallRequest,
    ) -> InstallReceipt {
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap();
        let response = client
            .post(format!("http://{}/runtime/control", self.address))
            .header(
                "x-cma-runtime-token",
                &assignment.environment["CHRONO_RUNTIME_CONTROL_TOKEN"],
            )
            .json(&RequestFrame::Install(request.clone()))
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        let ResponseFrame::Install(receipt) = response.json().await.unwrap() else {
            panic!("expected installation receipt");
        };
        receipt
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn assert_private(output: &std::process::Output, request: &RuntimeBootstrapRequest) {
    let token = &request.environment["CHRONO_RUNTIME_CONTROL_TOKEN"];
    for bytes in [&output.stdout, &output.stderr] {
        assert!(!String::from_utf8_lossy(bytes).contains(token));
        assert!(!String::from_utf8_lossy(bytes).contains("CHRONO_RUNTIME_CONTROL_TOKEN"));
    }
}

#[tokio::test]
async fn assignment_cli_gates_network_authenticates_and_rebinds_retained_home_after_restart() {
    let fixture = Fixture::new();
    let mut runtime = fixture.waiting("original-pod").await;
    assert!(tokio::net::TcpStream::connect(fixture.address)
        .await
        .is_err());
    assert!(!fixture.root.join("state").exists());
    let mut request = fixture.request();
    let mut wrong = request.clone();
    wrong.pod_uid = "replacement-pod".into();
    fixture.assign(&wrong, false).await;
    assert!(tokio::net::TcpStream::connect(fixture.address)
        .await
        .is_err());
    fixture.assign(&request, true).await;
    let original = fixture.status(&request).await;
    assert!(!original.installation_ready);
    let installation = InstallRequest::new(
        "install-once",
        vec!["printf 'installed\\n' >> installation-count".into()],
    );
    let installed = fixture.install(&request, &installation).await;
    assert!(fixture.status(&request).await.installation_ready);
    let persistent_journal = fixture
        .root
        .join("transfer/cma-checkpoints/runtime-state/install-v2.json");
    let journal_before = std::fs::read(&persistent_journal).unwrap();
    let authority: serde_json::Value = serde_json::from_slice(&journal_before).unwrap();
    assert_eq!(authority["sandbox_id"], "sbx_cli");
    assert_eq!(authority["state"], "completed");
    fixture.assign(&request, true).await;
    assert_eq!(fixture.status(&request).await.boot_id, original.boot_id);
    wrong = request.clone();
    wrong
        .environment
        .insert("CHRONO_SANDBOX_ID".into(), "other".into());
    fixture.assign(&wrong, false).await;
    std::fs::write(fixture.root.join("home/retained.txt"), "retained").unwrap();
    runtime.kill().await.unwrap();
    assert_private(&runtime.wait_with_output().await.unwrap(), &request);

    // A killed Pod leaves a stale socket. A new Pod must receive a fresh UID
    // and generation while keeping the existing home and process boot fence.
    let mut resumed = fixture.waiting("replacement-pod").await;
    fixture.assign(&request, false).await;
    request.pod_uid = "replacement-pod".into();
    request
        .environment
        .insert("CHRONO_RUNTIME_GENERATION".into(), "2".into());
    fixture.assign(&request, true).await;
    let restored = fixture.status(&request).await;
    assert_ne!(restored.boot_id, original.boot_id);
    assert!(restored.installation_ready);
    assert_eq!(fixture.install(&request, &installation).await, installed);
    assert_eq!(std::fs::read(persistent_journal).unwrap(), journal_before);
    assert_eq!(
        std::fs::read(fixture.root.join("home/installation-count")).unwrap(),
        b"installed\n"
    );
    for uid in ["original-pod", "replacement-pod"] {
        let ambient: serde_json::Value = serde_json::from_slice(
            &std::fs::read(fixture.root.join(format!(
                "ambient-{uid}/cma-checkpoints/runtime-state/install-v2.json"
            )))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(ambient["sandbox_id"], "native-tools-fixture");
        assert_eq!(ambient["state"], "completed");
    }
    assert_eq!(
        std::fs::read(fixture.root.join("home/retained.txt")).unwrap(),
        b"retained"
    );
    resumed.kill().await.unwrap();
    assert_private(&resumed.wait_with_output().await.unwrap(), &request);
}

fn faulty_request(fixture: &Fixture, incarnation: &str) -> RuntimeBootstrapRequest {
    use layer3_adapter::runtime_bootstrap::StagingStartupFailure;
    use std::time::{SystemTime, UNIX_EPOCH};

    let mut request = fixture.request();
    let sandbox_id = "sbx_00000000000000000000000000000001";
    request
        .environment
        .insert("CHRONO_SANDBOX_ID".into(), sandbox_id.into());
    let now = SystemTime::now();
    let expiry = now.duration_since(UNIX_EPOCH).unwrap().as_secs() + 300;
    request.staging_startup_failure = Some(
        StagingStartupFailure::from_setting(
            Some("staging"),
            &format!("{sandbox_id}:{expiry}"),
            now,
        )
        .unwrap()
        .bind_incarnation(incarnation)
        .unwrap(),
    );
    request
}

#[tokio::test]
async fn staging_startup_failure_requires_exact_confirmation_and_clears_on_the_same_home() {
    let fixture = Fixture::new();
    let mut request = faulty_request(&fixture, "original-incarnation");
    let runtime = fixture.waiting("original-pod").await;
    let mut confirm = request.clone();
    confirm.staging_startup_confirmed = true;
    fixture.assign(&confirm, false).await;

    let mut wrong = request.clone();
    wrong
        .environment
        .insert("CHRONO_SANDBOX_ID".into(), "sbx_untargeted".into());
    fixture.assign(&wrong, false).await;
    fixture.assign(&request, true).await;
    assert!(tokio::net::TcpStream::connect(fixture.address)
        .await
        .is_err());

    let mut obsolete = faulty_request(&fixture, "obsolete-incarnation");
    obsolete.staging_startup_confirmed = true;
    fixture.assign(&obsolete, false).await;
    fixture.assign(&confirm, true).await;
    let mut ordinary = request.clone();
    ordinary.staging_startup_failure = None;
    fixture.assign(&ordinary, false).await;
    let output = tokio::time::timeout(Duration::from_secs(10), runtime.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_private(&output, &request);
    let logs = [output.stdout, output.stderr].concat();
    assert!(String::from_utf8_lossy(&logs).contains("runtime startup interrupted after assignment"));
    assert!(!fixture.root.join("state").exists());

    request.staging_startup_failure = None;
    let mut recovered = fixture.waiting("original-pod").await;
    fixture.assign(&request, true).await;
    assert!(!fixture.status(&request).await.installation_ready);
    recovered.kill().await.unwrap();
}

#[tokio::test]
async fn ordinary_reassignment_before_confirmation_disarms_the_running_process() {
    let fixture = Fixture::new();
    let request = faulty_request(&fixture, "disarmed-incarnation");
    let mut runtime = fixture.waiting("original-pod").await;
    fixture.assign(&request, true).await;

    let mut ordinary = request.clone();
    ordinary.staging_startup_failure = None;
    fixture.assign(&ordinary, true).await;
    let mut stale_confirmation = request;
    stale_confirmation.staging_startup_confirmed = true;
    fixture.assign(&stale_confirmation, false).await;
    fixture.assign(&ordinary, true).await;
    assert!(!fixture.status(&ordinary).await.installation_ready);
    assert!(runtime.try_wait().unwrap().is_none());
    runtime.kill().await.unwrap();
}

#[tokio::test]
async fn staging_startup_failure_without_confirmation_permits_disarmed_reassignment() {
    use layer3_adapter::runtime_bootstrap::NATIVE_ASSIGNMENT_DEADLINE;

    let fixture = Fixture::new();
    let request = faulty_request(&fixture, "unconfirmed-incarnation");
    let mut runtime = fixture.waiting("original-pod").await;
    fixture.assign(&request, true).await;
    // Simulate an accepted request whose receipt or post-delivery proof the
    // manager could not use. There is no confirmation for this incarnation.
    tokio::time::timeout(
        NATIVE_ASSIGNMENT_DEADLINE + Duration::from_secs(20),
        async {
            while tokio::net::TcpStream::connect(fixture.address)
                .await
                .is_err()
            {
                assert!(
                    runtime.try_wait().unwrap().is_none(),
                    "unconfirmed runtime exited"
                );
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        },
    )
    .await
    .unwrap();
    let mut ordinary = request.clone();
    ordinary.staging_startup_failure = None;
    let mut changed = ordinary.clone();
    changed
        .environment
        .insert("CHRONO_RUNTIME_GENERATION".into(), "2".into());
    fixture.assign(&changed, false).await;
    fixture.assign(&ordinary, true).await;
    fixture.assign(&ordinary, true).await;
    let status = fixture.status(&ordinary).await;
    assert!(!status.installation_ready);

    let mut stale_confirmation = request.clone();
    stale_confirmation.staging_startup_confirmed = true;
    fixture.assign(&stale_confirmation, false).await;
    fixture.assign(&request, false).await;
    assert_eq!(
        fixture.status(&ordinary).await.installation_ready,
        status.installation_ready
    );
    runtime.kill().await.unwrap();
    let output = runtime.wait_with_output().await.unwrap();
    let logs = [output.stdout, output.stderr].concat();
    assert!(
        !String::from_utf8_lossy(&logs).contains("runtime startup interrupted after assignment")
    );
}
