//! Real executable, signed HTTP callback and process-signal evidence for runner composition.
use super::{
    fake_provider::{FakeProvider, FakeProviderConfig},
    support::{Envelope, Fixture},
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::process::{Child, Command};
struct Process {
    child: Child,
    dir: PathBuf,
    base: String,
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
        let _ = fs::remove_dir_all(&self.dir);
    }
}
impl Process {
    async fn start(fixture: &mut Fixture, provider: &FakeProvider, budget_ms: u64) -> Self {
        let resolved = fixture
            .state
            .resolver
            .resolve(fixture.id.as_str())
            .await
            .unwrap();
        let cma = reqwest::Url::parse(&resolved.pointer.record_url)
            .unwrap()
            .origin()
            .ascii_serialization();
        let dir = std::env::temp_dir().join(format!("cmaeg-runner-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::write(dir.join("keys"), format!("1:{}", STANDARD.encode([1; 32]))).unwrap();
        fs::write(dir.join("service"), "test-service-bearer").unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let base = format!("http://{address}");
        let log = fs::File::create(dir.join("trace.log")).unwrap();
        let child=Command::new(env!("CARGO_BIN_EXE_cmaeg")).kill_on_drop(true)
            .env_clear().env("RUST_LOG","cmaeg=info")
            .env("CMAEG_LISTEN",address.to_string()).env("CMAEG_PUBLIC_URL",&base)
            .env("CMAEG_CMA_BASE_URL",cma).env("CMAEG_NYXID_BASE_URL","https://nyx.example")
            .env("CMAEG_MASTER_KEYS_FILE",dir.join("keys")).env("CMAEG_CMA_SERVICE_TOKEN_FILE",dir.join("service"))
            .env("CMAEG_ACCEPT_BUDGET_MS",budget_ms.to_string()).env("CMAEG_DRAIN_SECONDS","15")
            .env("CMAEG_PROVIDERS",json!([{"slug":"cma_codex","base_url":provider.base_url(),"kind":"responses_http","paths":{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}","conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"},"max_inflight":1}]).to_string())
            .stdout(Stdio::from(log.try_clone().unwrap())).stderr(Stdio::from(log)).spawn().unwrap();
        let mut process = Self { child, dir, base };
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                assert!(
                    process.child.try_wait().unwrap().is_none(),
                    "{}",
                    process.logs()
                );
                if reqwest::get(format!("{}/readyz", process.base))
                    .await
                    .is_ok_and(|r| r.status() == 200)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        fixture.url = format!("{}/v1/callbacks/{}", process.base, fixture.id);
        process
    }
    fn logs(&self) -> String {
        fs::read_to_string(self.dir.join("trace.log")).unwrap()
    }
    async fn terminate(&mut self) {
        #[cfg(unix)]
        {
            assert!(Command::new("kill")
                .args(["-TERM", &self.child.id().unwrap().to_string()])
                .status()
                .await
                .unwrap()
                .success());
        }
        #[cfg(not(unix))]
        {
            self.child.start_kill().unwrap();
        }
    }
    fn assert_safe_logs(&self) {
        let logs = self.logs();
        for secret in [
            "runtime-test-provider-bearer",
            "test-service-bearer",
            "cmaeg1.ev.",
        ] {
            assert!(!logs.contains(secret), "secret appeared in process trace");
        }
    }
}
fn credential(value: &mut Value) {
    value["secrets"]["provider_credential"] = "runtime-test-provider-bearer".into();
}

#[tokio::test]
async fn real_signed_callback_accepts_at_two_seconds_and_defers_at_twenty() {
    for (delay, budget, expected) in [(2, 20000, 2), (30, 20000, 20)] {
        let fake = FakeProvider::start(FakeProviderConfig {
            response_delay: Duration::from_secs(delay),
            ..Default::default()
        })
        .await;
        let mut fixture = Fixture::new(credential, Default::default()).await;
        let mut process = Process::start(&mut fixture, &fake, budget).await;
        let start = Instant::now();
        let response = fixture.send(&Envelope::new()).await;
        assert_eq!(response.status(), 202);
        assert!(start.elapsed() >= Duration::from_secs(expected));
        assert!(start.elapsed() < Duration::from_secs(expected + 2));
        assert_eq!(
            response.json::<Value>().await.unwrap()["status"],
            "accepted"
        );
        tokio::time::timeout(Duration::from_secs(12), async {
            while !process.logs().contains("outcome=\"completed\"") {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            fake.requests()
                .iter()
                .filter(|r| r.method == "POST")
                .count(),
            1
        );
        process.assert_safe_logs();
        process.terminate().await;
        assert!(
            tokio::time::timeout(Duration::from_secs(2), process.child.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
        fake.close().await;
    }
}
#[cfg(unix)]
#[tokio::test]
async fn sigterm_keeps_process_alive_until_ten_second_turn_finishes() {
    let fake = FakeProvider::start(FakeProviderConfig {
        terminal_delay: Duration::from_secs(10),
        ..Default::default()
    })
    .await;
    let mut fixture = Fixture::new(credential, Default::default()).await;
    let mut process = Process::start(&mut fixture, &fake, 20000).await;
    assert_eq!(fixture.send(&Envelope::new()).await.status(), 202);
    let busy = fixture.send(&Envelope::new()).await;
    assert_eq!(busy.status(), 503);
    assert_eq!(busy.headers()["retry-after"], "2");
    assert_eq!(busy.json::<Value>().await.unwrap()["code"], "provider_busy");
    let start = Instant::now();
    process.terminate().await;
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let ready = reqwest::get(format!("{}/readyz", process.base))
                .await
                .unwrap();
            if ready.status() == 503 {
                assert!(ready.json::<Value>().await.unwrap()["reasons"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("draining")));
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let refused = fixture.send(&Envelope::new()).await;
    assert_eq!(refused.status(), 503);
    assert_eq!(refused.json::<Value>().await.unwrap()["code"], "draining");
    assert!(process.child.try_wait().unwrap().is_none());
    assert!(
        tokio::time::timeout(Duration::from_secs(12), process.child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    assert!(start.elapsed() >= Duration::from_secs(9));
    assert!(process.logs().contains("outcome=\"completed\""));
    process.assert_safe_logs();
    fake.close().await;
}
