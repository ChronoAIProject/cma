//! The real gateway executable, started against test fixtures, for the runner tests.
use super::{fake_provider::FakeProvider, support::Fixture};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::json;
use std::{fs, path::PathBuf, process::Stdio, time::Duration};
use tokio::process::{Child, Command};
pub struct Process {
    pub child: Child,
    pub dir: PathBuf,
    pub base: String,
    /// The private Prometheus scrape URL.
    pub metrics: String,
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
        let _ = fs::remove_dir_all(&self.dir);
    }
}
impl Process {
    pub async fn start(fixture: &mut Fixture, provider: &FakeProvider, budget_ms: u64) -> Self {
        Self::with_reply(fixture, provider, budget_ms, "https://nyx.example").await
    }
    pub async fn with_reply(
        fixture: &mut Fixture,
        provider: &FakeProvider,
        budget_ms: u64,
        nyxid: &str,
    ) -> Self {
        Self::with_env(fixture, provider, budget_ms, nyxid, &[]).await
    }
    /// Start the executable with `env` added to its fixed environment.
    pub async fn with_env(
        fixture: &mut Fixture,
        provider: &FakeProvider,
        budget_ms: u64,
        nyxid: &str,
        env: &[(&str, &str)],
    ) -> Self {
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
        let free = || {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap()
        };
        let (address, metrics) = (free(), free());
        let base = format!("http://{address}");
        let log = fs::File::create(dir.join("trace.log")).unwrap();
        let child=Command::new(env!("CARGO_BIN_EXE_cmaeg")).kill_on_drop(true)
            .env_clear().env("RUST_LOG","cmaeg=info")
            .env("CMAEG_LISTEN",address.to_string()).env("CMAEG_PUBLIC_URL",&base)
            .env("CMAEG_METRICS_LISTEN", metrics.to_string()).envs(env.iter().copied())
            .env("CMAEG_CMA_BASE_URL",cma).env("CMAEG_NYXID_BASE_URL",nyxid)
            .env("CMAEG_MASTER_KEYS",format!("1:{}", STANDARD.encode([1; 32])))
            .env("CMAEG_CMA_SERVICE_TOKEN","test-service-bearer")
            .env("CMAEG_ACCEPT_BUDGET_MS",budget_ms.to_string()).env("CMAEG_DRAIN_SECONDS","15")
            .env("CMAEG_PROVIDERS",json!([{"slug":"cma_codex","base_url":provider.base_url(),"kind":"responses_http","paths":{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}","conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"},"max_inflight":1}]).to_string())
            .stdout(Stdio::from(log.try_clone().unwrap())).stderr(Stdio::from(log)).spawn().unwrap();
        let mut process = Self {
            child,
            dir,
            base,
            metrics: format!("http://{metrics}/metrics"),
        };
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
    pub fn logs(&self) -> String {
        fs::read_to_string(self.dir.join("trace.log")).unwrap()
    }
    pub async fn terminate(&mut self) {
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
    pub fn assert_safe_logs(&self) {
        let logs = self.logs();
        for secret in [
            "runtime-test-provider-bearer",
            "test-service-bearer",
            "test-route-credential",
            "cmaeg1.ev.",
        ] {
            assert!(!logs.contains(secret), "secret appeared in process trace");
        }
    }
}
