//! The real gateway executable, started against test fixtures, for the runner tests.
use super::{fake_provider::FakeProvider, support::Fixture};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::json;
use std::{fs, path::PathBuf, process::Stdio, sync::Arc, time::Duration};
use tokio::process::{Child, Command};
#[path = "replica_capture.rs"]
pub mod capture;
#[path = "../support/database.rs"]
pub mod database;
pub struct Process {
    _database: Option<Arc<database::DatabaseFixture>>,
    capture: Option<capture::Capture>,
    pub child: Child,
    pub dir: PathBuf,
    pub base: String,
    /// The private Prometheus scrape URL.
    pub metrics: String,
}
impl Drop for Process {
    fn drop(&mut self) {
        if let Err(_error) = self.child.start_kill() {
            eprintln!("host_child_signal_failed");
        }
        // The owned parent collects logs only after group absence is confirmed.
        if self.capture.is_none() {
            let _ = fs::remove_dir_all(&self.dir);
        }
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
        let database = if std::env::var_os("CMA_SERVICE_AUTH_TEST_MONGODB_URI").is_some() {
            Some(Arc::new(
                database::DatabaseFixture::new(std::path::Path::new(env!("CARGO_BIN_EXE_cmaeg")))
                    .await,
            ))
        } else {
            None
        };
        Self::with_database(fixture, provider, budget_ms, nyxid, env, database).await
    }
    /// Independent executables share only durable storage, never a process-local registry.
    pub async fn with_database(
        fixture: &mut Fixture,
        provider: &FakeProvider,
        budget_ms: u64,
        nyxid: &str,
        env: &[(&str, &str)],
        database: Option<Arc<database::DatabaseFixture>>,
    ) -> Self {
        let providers = json!([{"slug":"cma_codex","base_url":provider.base_url(),"kind":"responses_http","paths":{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}","conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"},"max_inflight":1}]);
        Self::with_providers(fixture, providers, budget_ms, nyxid, env, database).await
    }
    /// Inject the provider protocol fixture without coupling the process runner to its implementation.
    pub async fn with_providers(
        fixture: &mut Fixture,
        providers: serde_json::Value,
        budget_ms: u64,
        nyxid: &str,
        env: &[(&str, &str)],
        database: Option<Arc<database::DatabaseFixture>>,
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
        let mut command = Command::new(env!("CARGO_BIN_EXE_cmaeg"));
        command.env_clear();
        if let Some(database) = &database {
            database.admin.database(&database.name).collection::<mongodb::bson::Document>("eg_channel_records")
                .update_one(mongodb::bson::doc!{"_id":resolved.pointer.record_id.as_str()},
                    mongodb::bson::doc!{"$setOnInsert":{"owner":&resolved.definition.owner.subject,"blob":mongodb::bson::Binary{subtype:mongodb::bson::spec::BinarySubtype::Generic,bytes:fixture.backend.blob.lock().unwrap().clone()},"version":resolved.version as i64,"schema_version":1_i32}})
                .upsert(true).await.unwrap();
            database.configure(&mut command);
        }
        capture::verify_application();
        let child = command
            .kill_on_drop(true)
            .env("RUST_LOG", "cmaeg=info")
            // The unoptimized executable needs the same stack budget as backend tests.
            .env("RUST_MIN_STACK", "33554432")
            .env("CMAEG_LISTEN", address.to_string())
            .env("CMAEG_PUBLIC_URL", &base)
            .env("CMAEG_REPLICA_ADDRESS", &base)
            .env("CMAEG_METRICS_LISTEN", metrics.to_string())
            .envs(env.iter().copied())
            .env("CMAEG_CMA_BASE_URL", cma)
            .env("CMAEG_NYXID_BASE_URL", nyxid)
            .env(
                "CMAEG_MASTER_KEYS",
                format!("1:{}", STANDARD.encode([1; 32])),
            )
            .env("CMAEG_CMA_SERVICE_TOKEN", "test-service-bearer")
            .env("CMAEG_ACCEPT_BUDGET_MS", budget_ms.to_string())
            .env("CMAEG_DRAIN_SECONDS", "15")
            .env("CMAEG_PROVIDERS", providers.to_string())
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log))
            .spawn()
            .unwrap();
        let mut process = Self {
            _database: database,
            capture: capture::Capture::new(&dir, child.id().unwrap(), env),
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
    pub fn mark_joined(&mut self) -> Result<(), &'static str> {
        match &mut self.capture {
            Some(capture) => capture.joined(),
            None => Ok(()),
        }
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
