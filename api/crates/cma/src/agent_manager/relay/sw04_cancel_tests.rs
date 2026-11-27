use super::*;
use axum::http::StatusCode;
use futures_util::FutureExt;
use layer2_core::{
    agent_ownership::OwnershipStore,
    ids::{AgentCommandId, AgentResponseId},
    RuntimeCapabilities, RuntimeCapability,
};
use mongodb::bson::{doc, Document};
use mongodb::options::{ClientOptions, ServerAddress};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{io, net::SocketAddr, panic::AssertUnwindSafe, process::Stdio};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
    time::{sleep, timeout, timeout_at, Instant},
};

#[path = "sw04_cancel_tests/child.rs"]
mod child;

const DATABASE_ENV: &str = "CMA_SW04_CANCEL_CHILD_DATABASE";
const MONGO_ENV: &str = "CHRONO_TEST_MONGODB_URI";
const READY: &str = "SW04_LISTENER=";
const PROCESS_HEADER: &str = "x-sw04-serving-process";
const SOURCE_KEY: &str = "sw04-source";
const CANCEL_KEY: &str = "sw04-cancel";
const LIMIT: Duration = Duration::from_secs(10);
const DATABASE_CLEANUP_LIMIT: Duration = Duration::from_secs(75);

fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value))
}

fn capabilities() -> RuntimeCapabilities {
    RuntimeCapabilities::new([
        RuntimeCapability::Conversation,
        RuntimeCapability::Interrupt,
    ])
}

async fn mongo() -> Client {
    let uri = std::env::var(MONGO_ENV).expect("disposable Mongo fixture required");
    assert!(uri.starts_with("mongodb://"), "direct Mongo URI required");
    let mut options = ClientOptions::parse(uri)
        .await
        .expect("invalid Mongo fixture URI");
    assert!(
        options.repl_set_name.is_some()
            && options.credential.is_none()
            && matches!(options.hosts.as_slice(), [ServerAddress::Tcp { host, .. }] if host == "127.0.0.1"),
        "unauthenticated loopback replica set required"
    );
    // Prevent server-advertised addresses from widening the loopback boundary.
    options.direct_connection = Some(true);
    options.server_selection_timeout = Some(LIMIT);
    Client::with_options(options).unwrap()
}

#[tokio::test]
#[ignore = "requires a disposable loopback Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn sw04_cancel_admitted_by_non_owner_executes_once() {
    let mongo = mongo().await;
    if let Ok(name) = std::env::var(DATABASE_ENV) {
        let suffix = name
            .strip_prefix("cma_sw04_cancel_")
            .expect("owned fixture database required");
        assert!(uuid::Uuid::parse_str(suffix).is_ok());
        timeout(Duration::from_secs(90), child::serve(&mongo, &name))
            .await
            .expect("fixture replica timed out");
        mongo.shutdown().await;
        return;
    }

    let name = format!("cma_sw04_cancel_{}", uuid::Uuid::new_v4().simple());
    let database = mongo.database(&name);
    let mut processes = Vec::with_capacity(2);
    let result = AssertUnwindSafe(timeout(Duration::from_secs(60), async {
        let store = MongoSandboxStore::open(&mongo, &name).await.unwrap();
        let mut agent = fixture::seed_named(&store, &database, "sw04-cancel").await;
        let revision = agent.revision;
        agent.revision += 1;
        agent.capabilities = capabilities();
        store.save_agent(revision, agent.clone()).await.unwrap();
        let mut replicas = Vec::with_capacity(2);
        for _ in 0..2 {
            processes.push(spawn(&name));
            replicas.push(listener(processes.last_mut().unwrap()).await);
        }
        assert_ne!(replicas[0].address, replicas[1].address);
        assert_ne!(replicas[0].instance, replicas[1].instance);
        let ownership = crate::store::agent_ownership::MongoAgentOwnership::open(&mongo, &name)
            .await
            .unwrap();
        observe(&database, &store, &ownership, &replicas, &agent.id).await
    }))
    .catch_unwind()
    .await;

    let mut stopped = true;
    for process in &mut processes {
        if !matches!(timeout(LIMIT, reap(process)).await, Ok(Ok(()))) {
            stopped = false;
        }
    }
    let removed = stopped && drop_owned_database(&mongo, &database).await;
    if !removed {
        eprintln!(
            "SW04 fixture cleanup failed: children_reaped={stopped} database_removed={removed}"
        );
    }
    let observation = match result {
        Ok(result) => result.expect("SW04 observation timed out"),
        Err(error) => std::panic::resume_unwind(error),
    };
    assert!(removed, "SW04 fixture cleanup incomplete");
    println!("SW04_CANCEL_OBSERVATION={observation}");
}

async fn drop_owned_database(mongo: &Client, database: &mongodb::Database) -> bool {
    // Mongo can retain an unfinished transaction after its client exits.
    // Allow one default transaction lifetime plus time for removal and readback.
    let deadline = Instant::now() + DATABASE_CLEANUP_LIMIT;
    let filter = doc! {"name": database.name()};
    let mut last_drop = None;
    let removed = timeout_at(deadline, async {
        loop {
            // A server-side deadline lets a subsequent attempt make progress.
            // Reconcile the exact database before repeating an unconfirmed drop.
            last_drop = Some(
                timeout(LIMIT, async {
                    database
                        .run_command(doc! {"dropDatabase": 1, "maxTimeMS": 8_000_i64})
                        .await
                })
                .await,
            );
            let visible = timeout(LIMIT, async {
                mongo.list_database_names().filter(filter.clone()).await
            })
            .await;
            if matches!(visible, Ok(Ok(names)) if names.is_empty()) {
                return true;
            }
            sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .unwrap_or(false);
    if !removed {
        eprintln!("SW04 fixture database removal unconfirmed: last_drop={last_drop:?}");
    }
    removed
}

async fn reap(process: &mut Child) -> io::Result<()> {
    // Closing stdin lets the replica stop accepting requests and finish its
    // current MongoDB work before the parent drops the fixture database.
    drop(process.stdin.take());
    let status = process.wait().await?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "fixture replica exited with {status}"
        )));
    }
    Ok(())
}

fn spawn(database: &str) -> Child {
    let name = module_path!().split_once("::").unwrap().1.to_owned()
        + "::sw04_cancel_admitted_by_non_owner_executes_once";
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &name, "--ignored", "--nocapture"])
        .env_clear()
        .env(DATABASE_ENV, database)
        .env(MONGO_ENV, std::env::var_os(MONGO_ENV).unwrap())
        .env("RUST_MIN_STACK", "33554432")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("start SW04 test process")
}

#[derive(serde::Deserialize)]
struct Listener {
    pid: u32,
    address: SocketAddr,
    instance: String,
}

impl Listener {
    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.address)
    }
}

async fn listener(process: &mut Child) -> Listener {
    let mut output = BufReader::new(process.stdout.as_mut().unwrap()).lines();
    let ready: Listener = timeout(LIMIT, async {
        loop {
            let line = output
                .next_line()
                .await
                .unwrap()
                .expect("fixture exited before readiness");
            if let Some(value) = line.strip_prefix(READY) {
                break serde_json::from_str(value).unwrap();
            }
        }
    })
    .await
    .expect("SW04 listener startup timed out");
    assert_eq!(Some(ready.pid), process.id());
    assert_ne!(ready.pid, std::process::id());
    assert!(ready.address.ip().is_loopback());
    assert!(uuid::Uuid::parse_str(&ready.instance).is_ok());
    ready
}

async fn observe(
    database: &mongodb::Database,
    store: &MongoSandboxStore,
    ownership: &dyn OwnershipStore,
    replicas: &[Listener],
    agent: &AgentId,
) -> Value {
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(LIMIT)
        .build()
        .unwrap();
    let members = ownership.membership().await.unwrap();
    assert_eq!(members.members.len(), 2);
    let preferred = &candidates(agent, &members, None).unwrap()[0].instance_id;
    let index = replicas
        .iter()
        .position(|replica| &replica.instance == preferred)
        .unwrap();
    let (owner, peer) = (&replicas[index], &replicas[1 - index]);
    let acquired = client
        .post(owner.url(&format!("/fixture/acquire/{agent}")))
        .send()
        .await
        .unwrap();
    assert_eq!(acquired.status(), StatusCode::NO_CONTENT);
    let source_path = format!("/api/v2/agents/{agent}/responses");
    let source = client
        .post(owner.url(&source_path))
        .header("idempotency-key", SOURCE_KEY)
        .json(&json!({"input":"Synthetic active turn", "stream":true}))
        .send()
        .await
        .unwrap();
    assert_eq!(source.status(), StatusCode::OK);
    let mut stream = SseReader {
        response: source,
        buffered: String::new(),
    };
    let created = stream.event("response.created").await;
    let response = AgentResponseId(created.body["response"]["id"].as_str().unwrap().into());
    let source = store
        .get_response_command(&response)
        .await
        .unwrap()
        .unwrap();
    drive(&client, owner, &source.receipt.envelope.command_id).await;
    let target_path = format!("{source_path}/{response}/controls");
    let inspection = client.get(peer.url(&target_path)).send().await.unwrap();
    assert_eq!(inspection.status(), StatusCode::OK);
    let inspection = inspection.json::<Value>().await.unwrap();
    assert_eq!(inspection["target_active"], true);
    assert_eq!(inspection["target"]["response_id"], response.as_str());
    let intent = json!({"turn_sequence": inspection["target"]["turn_sequence"],
        "expected_agent_revision": inspection["agent_revision"], "command":{"type":"cancel"}});

    let original = ownership.owner(agent).await.unwrap().unwrap().token;
    assert_eq!(original.instance_id, owner.instance);
    let admission = cancel(&client, peer, &target_path, &intent).await;
    assert_eq!(admission.body["target"]["response_id"], response.as_str());
    let id = command_id(&admission.body);
    let admitted = store.get_command(&id).await.unwrap().unwrap();
    assert_eq!(admitted.status, AgentResponseStatus::Queued);
    drive(&client, peer, &id).await;
    assert_eq!(store.get_command(&id).await.unwrap(), Some(admitted));
    drive(&client, owner, &id).await;
    let before = effects(&client, replicas).await;

    let replay = cancel(&client, owner, &target_path, &intent).await;
    assert_eq!(replay.body["target"], admission.body["target"]);
    assert_eq!(command_id(&replay.body), id);
    for replica in replicas {
        drive(&client, replica, &id).await;
    }
    let after = effects(&client, replicas).await;
    let retained = store.get_command(&id).await.unwrap().unwrap();
    assert_eq!(retained.receipt.envelope.command, AgentCommand::Interrupt);
    assert_eq!(retained.status, AgentResponseStatus::Completed);
    let dispatch = retained.work.dispatch.unwrap();
    assert_eq!(dispatch.claim.holder, original.instance_id);
    let commands = database.collection::<Document>("agent_commands");
    let match_key = doc! {"agent_id": agent.as_str(), "key_sha256": retained.key_sha256};
    assert_eq!(
        commands.count_documents(match_key.clone()).await.unwrap(),
        1
    );
    let record = commands.find_one(match_key).await.unwrap().unwrap();
    let binding = record.get_document("explicit_control").unwrap();
    assert_eq!(binding.get_str("response_id").unwrap(), response.as_str());
    assert_eq!(
        ownership.owner(agent).await.unwrap().unwrap().token,
        original
    );
    let observation = json!({
        "schema": "cma-sw04-cancel-v1",
        "replicas": replicas.iter().map(|replica| hash(&replica.instance)).collect::<Vec<_>>(),
        "owner": {"processHash": hash(&original.instance_id), "epoch": original.epoch},
        "admission": admission.evidence(),
        "replay": replay.evidence(),
        "durable": {"responseHash": hash(binding.get_str("response_id").unwrap()),
            "commandHash": hash(retained.receipt.envelope.command_id.as_str()),
            "status": retained.status, "holderHash": hash(&dispatch.claim.holder),
            "runtimeGeneration": dispatch.runtime_generation},
        "effectsBeforeReplay": before,
        "effectsAfterReplay": after,
    });
    assert_eq!(before, after);
    assert_eq!(before.len(), 1);
    assert_eq!(before[0]["processHash"], hash(&original.instance_id));
    assert_eq!(before[0]["ownerProcessHash"], hash(&original.instance_id));
    assert_eq!(before[0]["commandHash"], hash(id.as_str()));
    assert_eq!(before[0]["runtimeGeneration"], dispatch.runtime_generation);
    assert_eq!(before[0]["epoch"], original.epoch);
    assert_eq!(admission.process_hash, hash(&peer.instance));
    assert_eq!(replay.process_hash, hash(&owner.instance));
    observation
}

fn command_id(body: &Value) -> AgentCommandId {
    AgentCommandId(
        body["control"]["receipt"]["command_id"]
            .as_str()
            .unwrap()
            .into(),
    )
}

struct Cancel {
    process_hash: String,
    status: StatusCode,
    body: Value,
}

impl Cancel {
    fn evidence(&self) -> Value {
        json!({"processHash": self.process_hash, "status": self.status.as_u16(),
            "responseHash": hash(self.body["target"]["response_id"].as_str().unwrap()),
            "commandHash": hash(command_id(&self.body).as_str())})
    }
}

async fn cancel(
    client: &reqwest::Client,
    replica: &Listener,
    path: &str,
    intent: &Value,
) -> Cancel {
    let response = client
        .post(replica.url(path))
        .header("idempotency-key", CANCEL_KEY)
        .json(intent)
        .send()
        .await
        .unwrap();
    let status = response.status();
    assert_eq!(status, StatusCode::ACCEPTED);
    let process_hash = response.headers()[PROCESS_HEADER]
        .to_str()
        .unwrap()
        .to_owned();
    Cancel {
        process_hash,
        status,
        body: response.json().await.unwrap(),
    }
}

async fn drive(client: &reqwest::Client, replica: &Listener, id: &AgentCommandId) {
    let response = client
        .post(replica.url(&format!("/fixture/drive/{id}")))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

async fn effects(client: &reqwest::Client, replicas: &[Listener]) -> Vec<Value> {
    let mut all = Vec::new();
    for replica in replicas {
        all.extend(
            client
                .get(replica.url("/fixture/effects"))
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json::<Vec<Value>>()
                .await
                .unwrap(),
        );
    }
    all
}
