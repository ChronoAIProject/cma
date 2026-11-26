//! Parent for developer-13's explicitly configured actual receiver test binary.
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

const SELECTOR: &str = "link::desktop::tests::child_fixture::desktop_receiver_child";

pub(super) struct ReceiverRoot {
    pub(super) path: PathBuf,
    token: String,
}

impl ReceiverRoot {
    pub(super) fn new() -> Self {
        let parent = PathBuf::from(
            std::env::var_os("CMA_DESKTOP_RECEIVER_ROOT")
                .expect("dedicated fixture runner must supply its owned receiver root"),
        );
        assert!(parent.is_absolute() && parent.is_dir());
        let path = parent.join(format!("receiver-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir(&path).unwrap();
        let mut bytes = [0_u8; 32];
        ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut bytes).unwrap();
        let token = bytes.iter().map(|value| format!("{value:02x}")).collect();
        let root = Self { path, token };
        root.record("created");
        root
    }

    pub(super) fn identity(&self) -> &str {
        self.path.file_name().unwrap().to_str().unwrap()
    }

    fn record(&self, event: &str) {
        println!(
            "DESKTOP_FIXTURE_ROOT={}",
            serde_json::json!({"schema": 1, "event": event, "id": self.identity()})
        );
    }

    pub(super) fn token(&self) -> &str {
        &self.token
    }

    pub(super) fn floor(&self) -> serde_json::Value {
        serde_json::from_slice(
            &std::fs::read(self.path.join("state/desktop-owner/floor.json")).unwrap(),
        )
        .unwrap()
    }

    // Explicit cleanup follows confirmed child exit. Keep failed fixture roots
    // for the runner's exact-PID cleanup instead of removing an active floor.
    pub(super) fn cleanup(self) {
        std::fs::remove_dir_all(&self.path).unwrap();
        self.record("removed");
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Ready {
    protocol: u64,
    address: std::net::SocketAddr,
    sandbox_id: String,
    generation: u64,
    desktop_path: String,
    control_path: String,
    floor_path: String,
    activation_path: String,
}

pub(super) struct ReceiverChild {
    pid: u32,
    process: tokio::process::Child,
    output: BufReader<tokio::process::ChildStdout>,
    pub(super) address: std::net::SocketAddr,
}

impl ReceiverChild {
    pub(super) async fn start(root: &ReceiverRoot, sandbox_id: &str) -> Self {
        let binary = std::env::var_os("CMA_DESKTOP_RECEIVER_TEST_BINARY")
            .expect("dedicated fixture runner must supply the exact runtime test binary");
        assert!(Path::new(&binary).is_absolute());
        let mut process = tokio::process::Command::new(binary)
            .args([
                "--exact",
                SELECTOR,
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("CMA_DESKTOP_RECEIVER_CHILD", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let pid = process.id().unwrap();
        println!(
            "DESKTOP_FIXTURE_CHILD={}",
            serde_json::json!({"schema": 1, "event": "started", "id": root.identity(), "pid": pid})
        );
        let input = serde_json::json!({
            "root": root.path, "sandbox_id": sandbox_id, "control_token": root.token,
        });
        process
            .stdin
            .as_mut()
            .unwrap()
            .write_all(format!("{input}\n").as_bytes())
            .await
            .unwrap();
        let mut output = BufReader::new(process.stdout.take().unwrap());
        let ready = tokio::time::timeout(Duration::from_secs(10), async {
            let mut consumed = 0;
            loop {
                let mut line = String::new();
                let read = (&mut output).take(8193).read_line(&mut line).await.unwrap();
                consumed += read;
                assert!(read > 0 && read <= 8192 && consumed <= 32768);
                assert!(!line.contains(root.token()));
                if let Some((_, json)) = line.split_once("CMA_DESKTOP_RECEIVER_READY ") {
                    break serde_json::from_str::<Ready>(json).unwrap();
                }
            }
        })
        .await
        .expect("bounded receiver child readiness");
        assert_eq!(ready.protocol, 1);
        assert!(ready.address.ip().is_loopback());
        assert_eq!(ready.sandbox_id, sandbox_id);
        assert_eq!(ready.generation, 1);
        assert_eq!(ready.desktop_path, "/runtime/desktop");
        assert_eq!(ready.control_path, "/runtime/desktop/ownership");
        assert_eq!(ready.floor_path, "state/desktop-owner/floor.json");
        assert_eq!(ready.activation_path, "state/desktop-owner.activated");
        Self {
            pid,
            process,
            output,
            address: ready.address,
        }
    }

    pub(super) async fn stop(mut self, root: &ReceiverRoot) {
        drop(self.process.stdin.take());
        let status = tokio::time::timeout(Duration::from_secs(5), self.process.wait())
            .await
            .expect("receiver must drain after EOF")
            .unwrap();
        self.record_exit(root, "success", status);
        assert!(status.success());
        self.check_output(root).await;
    }

    pub(super) async fn kill(mut self, root: &ReceiverRoot) {
        self.process.kill().await.unwrap();
        let status = self.process.wait().await.unwrap();
        self.record_exit(root, "forced", status);
        assert!(!status.success());
        self.check_output(root).await;
    }

    fn record_exit(&self, root: &ReceiverRoot, expected: &str, status: std::process::ExitStatus) {
        println!(
            "DESKTOP_FIXTURE_CHILD={}",
            serde_json::json!({"schema": 1, "event": "exited", "id": root.identity(),
                "pid": self.pid, "expected": expected, "success": status.success(), "code": status.code()})
        );
    }

    async fn check_output(&mut self, root: &ReceiverRoot) {
        let mut stdout = String::new();
        (&mut self.output)
            .take(65537)
            .read_to_string(&mut stdout)
            .await
            .unwrap();
        let mut stderr = String::new();
        self.process
            .stderr
            .take()
            .unwrap()
            .take(65537)
            .read_to_string(&mut stderr)
            .await
            .unwrap();
        assert!(stdout.len() <= 65536 && stderr.len() <= 65536);
        assert!(!stdout.contains(root.token()) && !stderr.contains(root.token()));
    }
}
