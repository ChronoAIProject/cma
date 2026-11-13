//! Own and join only the test's loopback provider child, including panic paths.
use super::*;
use futures_util::{future::BoxFuture, FutureExt};
use ring::rand::SecureRandom;
use std::os::unix::fs::DirBuilderExt;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};

struct PrivateDirectory(PathBuf, bool);

impl PrivateDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "cma-key-provider-contract-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .unwrap();
        Self(path, false)
    }

    fn remove(&mut self) {
        std::fs::remove_dir_all(&self.0).expect("remove joined fixture state");
        self.1 = true;
    }
}

impl Drop for PrivateDirectory {
    fn drop(&mut self) {
        if self.1 {
            return;
        }
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            eprintln!("provider fixture directory cleanup failed: {error}");
        }
    }
}

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
        let directory = PrivateDirectory::new();
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
            .arg(&directory.0)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("CMA_WORK_FIXTURE_BEARER", token.expose())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .expect("start counting provider with Node.js");
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
        let auth = crate::auth::nyxid_contract_tests::auth(origin);
        self.issuer = Some(Arc::new(KeyIssuer::new(
            auth.http.clone(),
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

    async fn close(&mut self) {
        // EOF closes the fixture even if the test left a held response. Join
        // that exact child before removing its private durable journal.
        self.input.take();
        let status = match tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await {
            Ok(status) => status.expect("join fixture child"),
            Err(_) => {
                self.child
                    .kill()
                    .await
                    .expect("kill and join fixture child");
                panic!("provider fixture did not close after stdin EOF");
            }
        };
        assert!(status.success());
        assert!(!self.directory.0.join("provider.lock").exists());
        self.directory.remove();
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
    provider.close().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
