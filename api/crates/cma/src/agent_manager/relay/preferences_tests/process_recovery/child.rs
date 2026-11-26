use super::*;
use mongodb::options::{ClientOptions, ServerAddress};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    io,
    net::SocketAddr,
    process::{ExitStatus, Stdio},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};

const CHILD: &str =
    "agent_manager::relay::tests::mongo::preferences::process_recovery::child::preferences_replica_child";
const DATABASE: &str = "CMA_PREFERENCES_REPLICA_CHILD_DATABASE";
const MONGO_URI: &str = "CHRONO_TEST_MONGODB_URI";
const REPLY: &str = "CMA_PREFERENCES_REPLICA_REPLY ";
const MAX_LINE: u64 = 4096;
const CONTROL_TIMEOUT: Duration = Duration::from_secs(4);

#[derive(Serialize, Deserialize)]
enum Control {
    Acquire,
    TakeScopes,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ready {
    pid: u32,
    address: SocketAddr,
    instance: String,
}

pub(super) async fn require_local_mongo() {
    let uri = std::env::var(MONGO_URI).expect("explicit disposable Mongo fixture required");
    assert!(
        uri.starts_with("mongodb://"),
        "a direct Mongo fixture URI is required"
    );
    let options = ClientOptions::parse(&uri)
        .await
        .unwrap_or_else(|_| panic!("invalid disposable Mongo fixture"));
    assert!(options.repl_set_name.is_some());
    assert_eq!(options.direct_connection, Some(true));
    assert!(
        matches!(
            options.hosts.as_slice(),
            [ServerAddress::Tcp { host, .. }] if host == "127.0.0.1"
        ),
        "only a directly connected loopback replica set is supported"
    );
}

pub(super) struct ReplicaProcess {
    process: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    ready: Option<Ready>,
}

impl ReplicaProcess {
    pub(super) fn spawn(database: &str) -> Self {
        let mut process = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CHILD, "--ignored", "--nocapture"])
            .env_clear()
            .env(DATABASE, database)
            .env(MONGO_URI, std::env::var_os(MONGO_URI).unwrap())
            .env("RUST_MIN_STACK", "33554432")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("start CMA test replica");
        let input = process.stdin.take().unwrap();
        let output = BufReader::new(process.stdout.take().unwrap());
        Self {
            process,
            input,
            output,
            ready: None,
        }
    }

    pub(super) async fn read_ready(&mut self) {
        let ready: Ready = self.receive(Duration::from_secs(15)).await;
        assert_eq!(Some(ready.pid), self.process.id());
        assert_ne!(ready.pid, std::process::id());
        assert!(ready.address.ip().is_loopback());
        assert!(uuid::Uuid::parse_str(&ready.instance).is_ok());
        self.ready = Some(ready);
    }

    pub(super) fn instance(&self) -> &str {
        &self.ready.as_ref().unwrap().instance
    }

    pub(super) fn address(&self) -> SocketAddr {
        self.ready.as_ref().unwrap().address
    }

    pub(super) fn pid(&self) -> u32 {
        self.ready.as_ref().unwrap().pid
    }

    pub(super) async fn acquire(&mut self) -> Option<OwnershipToken> {
        self.command(Control::Acquire).await
    }

    pub(super) async fn take_scopes(&mut self) -> Vec<Option<OwnershipToken>> {
        self.command(Control::TakeScopes).await
    }

    async fn command<T: DeserializeOwned>(&mut self, control: Control) -> T {
        let bytes = format!("{}\n", serde_json::to_string(&control).unwrap());
        tokio::time::timeout(CONTROL_TIMEOUT, self.input.write_all(bytes.as_bytes()))
            .await
            .expect("CMA test replica control write timed out")
            .expect("CMA test replica control write failed");
        self.receive(CONTROL_TIMEOUT).await
    }

    async fn receive<T: DeserializeOwned>(&mut self, wait: Duration) -> T {
        tokio::time::timeout(wait, async {
            // The test harness may print its own startup lines before the reply.
            for _ in 0..8 {
                let mut line = String::new();
                let length = (&mut self.output)
                    .take(MAX_LINE)
                    .read_line(&mut line)
                    .await
                    .unwrap();
                assert!(
                    length > 0 && line.ends_with('\n'),
                    "invalid CMA test replica reply"
                );
                if let Some((_, value)) = line.split_once(REPLY) {
                    return serde_json::from_str(value).expect("invalid CMA test replica evidence");
                }
            }
            panic!("CMA test replica did not emit its control reply");
        })
        .await
        .expect("CMA test replica response timed out")
    }

    pub(super) async fn terminate(&mut self) -> io::Result<ExitStatus> {
        tokio::time::timeout(CONTROL_TIMEOUT, async {
            if let Some(status) = self.process.try_wait()? {
                return Ok(status);
            }
            self.process.start_kill()?;
            self.process.wait().await
        })
        .await
        .map_err(io::Error::other)?
    }
}

#[tokio::test]
#[ignore = "helper invoked only by rename_replays_after_owner_process_loss"]
async fn preferences_replica_child() {
    let database = std::env::var(DATABASE).expect("explicit preferences child role required");
    let suffix = database
        .strip_prefix("cma_812_maintenance_")
        .expect("an owned maintenance fixture database is required");
    assert!(uuid::Uuid::parse_str(suffix).is_ok());
    require_local_mongo().await;
    tokio::time::timeout(Duration::from_secs(75), async {
        let replica = Replica::open(&database, true).await;
        let coordinator = replica.coordinator.as_ref().unwrap();
        let agent = AgentId(AGENT.into());
        reply(&Ready {
            pid: std::process::id(),
            address: replica
                .address
                .strip_prefix("http://")
                .unwrap()
                .parse()
                .unwrap(),
            instance: coordinator.instance_id.clone(),
        });
        let mut input = BufReader::new(tokio::io::stdin());
        loop {
            let mut line = String::new();
            let length = (&mut input)
                .take(MAX_LINE)
                .read_line(&mut line)
                .await
                .unwrap();
            if length == 0 {
                break;
            }
            assert!(line.ends_with('\n'));
            match serde_json::from_str(&line).expect("invalid parent control") {
                Control::Acquire => {
                    let token = coordinator
                        .acquire(&agent)
                        .await
                        .unwrap()
                        .map(|owner| owner.token.clone());
                    reply(&token);
                }
                Control::TakeScopes => {
                    let scopes = std::mem::take(&mut *replica.scopes.lock().unwrap());
                    reply(&scopes);
                }
            }
        }
        replica.close().await;
    })
    .await
    .expect("CMA test replica exceeded its runtime bound");
}

fn reply(value: &impl Serialize) {
    println!("{REPLY}{}", serde_json::to_string(value).unwrap());
}
