//! Own and join only the test's loopback provider child, including panic paths.
use super::*;
use futures_util::{future::BoxFuture, FutureExt};
use ring::rand::SecureRandom;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};

mod cleanup;
mod directory;
mod receipt;
mod tests;
use directory::PrivateDirectory;

pub(super) struct Provider {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
    token: SecretValue,
    issuer: Option<Arc<KeyIssuer>>,
    directory: PrivateDirectory,
}

impl Provider {
    fn spawn() -> Self {
        let mut directory = PrivateDirectory::new();
        let mut entropy = [0_u8; 32];
        ring::rand::SystemRandom::new().fill(&mut entropy).unwrap();
        let token = SecretValue::new(
            entropy
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
        );
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../.github/scripts/work-recovery/provider-cli.mjs");
        let mut child = tokio::process::Command::new("node")
            .arg(script)
            .arg("--state-root")
            .arg(directory.path())
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("CMA_WORK_FIXTURE_BEARER", token.expose())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .expect("start counting provider with Node.js");
        // Register ownership before any fallible pipe extraction. Drop may
        // request a kill, but only an awaited result can confirm this exit.
        directory.spawned();
        Self {
            input: child.stdin.take(),
            output: BufReader::new(child.stdout.take().unwrap()),
            child,
            token,
            issuer: None,
            directory,
        }
    }

    async fn initialize(&mut self) {
        let ready = self.read().await;
        assert_eq!(ready["type"], "ready");
        let origin = ready["origin"].as_str().expect("loopback fixture origin");
        let url = url::Url::parse(origin).unwrap();
        assert_eq!(url.scheme(), "http");
        assert_eq!(url.host_str(), Some("127.0.0.1"));
        assert!(url.port().is_some());
        // Reuse the actual AuthState constructor's bounded dependency client.
        let auth = cma_am_auth::auth::test_support::contract_auth(origin);
        self.issuer = Some(Arc::new(KeyIssuer::new(
            auth.fixture_http().clone(),
            origin,
            "work-recovery-fixture",
        )));
    }

    async fn read(&mut self) -> Value {
        let mut line = String::new();
        let mut bounded = (&mut self.output).take(256 * 1024);
        tokio::time::timeout(Duration::from_secs(5), bounded.read_line(&mut line))
            .await
            .expect("provider control response deadline")
            .expect("provider control response");
        assert!(line.ends_with('\n'), "bounded provider control response");
        serde_json::from_str(&line).expect("provider control JSON")
    }

    async fn command(&mut self, command: Value) -> Value {
        let mut line = serde_json::to_vec(&command).unwrap();
        line.push(b'\n');
        tokio::time::timeout(
            Duration::from_secs(5),
            self.input.as_mut().unwrap().write_all(&line),
        )
        .await
        .expect("provider control write deadline")
        .expect("provider control write");
        let response = self.read().await;
        assert_eq!(response["type"], "snapshot");
        response["ledger"].clone()
    }

    pub fn issuer(&self) -> &Arc<KeyIssuer> {
        self.issuer.as_ref().expect("initialized fixture issuer")
    }
    pub fn token(&self) -> &SecretValue {
        &self.token
    }
    pub async fn snapshot(&mut self) -> Value {
        self.command(json!({"command":"snapshot"})).await
    }
    pub async fn configure(&mut self, modes: Value) {
        self.command(json!({"command":"configure","modes":modes}))
            .await;
    }
    pub async fn release(&mut self, request: u64, action: &str) {
        self.command(json!({"command":"release","request":request,"action":action}))
            .await;
    }
    pub async fn barrier(&mut self, phase: &str) -> u64 {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let state = tokio::time::timeout_at(deadline, self.snapshot())
                .await
                .expect("source-bound provider barrier deadline");
            if let Some(request) = state["pending"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["phase"] == phase)
            {
                return request["request"].as_u64().unwrap();
            }
            tokio::task::yield_now().await;
        }
    }

    async fn close(&mut self) -> Result<(), cleanup::Failure> {
        self.input.take();
        cleanup::close(&mut self.child, &mut self.output, &mut self.directory).await
    }
}

pub(super) async fn with_provider(
    test: impl for<'a> FnOnce(&'a mut Provider) -> BoxFuture<'a, ()>,
) {
    let mut provider = Provider::spawn();
    let result = AssertUnwindSafe(async {
        provider.initialize().await;
        tokio::time::timeout(Duration::from_secs(25), test(&mut provider))
            .await
            .expect("bounded production-client fixture test");
    })
    .catch_unwind()
    .await;
    let cleanup = AssertUnwindSafe(provider.close()).catch_unwind().await;
    cleanup::finish_test(result, cleanup);
}
