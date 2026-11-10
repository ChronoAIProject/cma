use super::*;
use crate::store::agent_log_reads::{
    integrity::{LegacyIntegrity, LogReadIntegrity, ReadHead},
    read_with_integrity, Window,
};
use crate::store::mongo_read_telemetry as telemetry;
use mongodb::{event::cmap::CmapEvent, ClientSession};

// This exercises the consumer contract, not #1591's metadata parser/proof.
// The fixture explicitly supplies trust after reading its head in the same
// snapshot. End-to-end metadata activation is a separate integration gate.
struct FixtureProof {
    append_after_head: bool,
}

#[async_trait::async_trait]
impl LogReadIntegrity for FixtureProof {
    async fn read_head(
        &self,
        store: &MongoSandboxStore,
        id: &AgentId,
        session: &mut ClientSession,
    ) -> Result<Option<ReadHead>, SandboxStoreError> {
        let mut head = LegacyIntegrity.read_head(store, id, session).await?;
        if let Some(head) = &mut head {
            if self.append_after_head {
                let entry = entry(id, head.last_seq + 1);
                // A separate writer advances after the proof snapshot began.
                store
                    .agent_log
                    .insert_one(log_document(&entry).unwrap())
                    .await
                    .unwrap();
                store.agents.update_one(doc! {"_id":id.as_str()}, doc! {"$set": {
                    "last_seq":entry.seq as i64, "record.conversation.last_seq":entry.seq as i64
                }}).await.unwrap();
            }
            head.trusted = true;
        }
        Ok(head)
    }
}

struct RejectedProof;
#[async_trait::async_trait]
impl LogReadIntegrity for RejectedProof {
    async fn read_head(
        &self,
        _: &MongoSandboxStore,
        _: &AgentId,
        _: &mut ClientSession,
    ) -> Result<Option<ReadHead>, SandboxStoreError> {
        Err(SandboxStoreError::Corrupt("fixture quarantine".into()))
    }
}

fn entry(id: &AgentId, seq: u64) -> AgentLogEntry {
    AgentLogEntry {
        agent_id: id.clone(),
        seq,
        appended_at: NOW.into(),
        kind: AgentLogKind::AgentMessage {
            content: "x".repeat(4096),
        },
    }
}

struct Fixture {
    client: Client,
    name: String,
    store: MongoSandboxStore,
    id: AgentId,
    checkouts: Arc<AtomicU64>,
    commands: Arc<Mutex<Vec<Document>>>,
}

impl Fixture {
    async fn new(head: u64) -> Self {
        let name = format!("cma_window_{}", uuid::Uuid::new_v4().simple());
        let checkouts = Arc::new(AtomicU64::new(0));
        let captured = checkouts.clone();
        let commands = Arc::new(Mutex::new(Vec::new()));
        let captured_commands = commands.clone();
        let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        options.cmap_event_handler = Some(EventHandler::callback(move |event| {
            if matches!(event, CmapEvent::ConnectionCheckedOut(_)) {
                captured.fetch_add(1, Ordering::SeqCst);
            }
        }));
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            if let CommandEvent::Started(event) = event {
                captured_commands.lock().unwrap().push(event.command);
            }
        }));
        let client = Client::with_options(options).unwrap();
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let parent = sandbox("sbx_window");
        store.create_sandbox(parent.clone()).await.unwrap();
        let (_, agent) = worktree_and_agent(&parent.id);
        store.create_agent(agent.clone()).await.unwrap();
        for start in (1..=head).step_by(200) {
            let documents = (start..=(start + 199).min(head))
                .map(|seq| log_document(&entry(&agent.id, seq)).unwrap());
            store.agent_log.insert_many(documents).await.unwrap();
        }
        store
            .agents
            .update_one(
                doc! {"_id":agent.id.as_str()},
                doc! {"$set": {
                    "last_seq":head as i64, "record.conversation.last_seq":head as i64
                }},
            )
            .await
            .unwrap();
        Self {
            client,
            name,
            store,
            id: agent.id,
            checkouts,
            commands,
        }
    }

    async fn read<P: LogReadIntegrity>(
        &self,
        window: Window,
        proof: &P,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        read_with_integrity(&self.store, &self.id, window, proof).await
    }

    async fn cleanup(self) {
        self.client.database(&self.name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn public_window_consumer_bounds_reads_and_checkouts_with_injected_proof() {
    for history in [1_000, 10_000] {
        let fixture = Fixture::new(history).await;
        telemetry::isolated_test(async {
            let window = Window::Page { after: history - 200, limit: 200 };
            let before = fixture.checkouts.load(Ordering::SeqCst);
            let legacy = fixture.read(window, &LegacyIntegrity).await.unwrap();
            let legacy_checkouts = fixture.checkouts.load(Ordering::SeqCst) - before;
            assert_eq!(telemetry::snapshot(AGENT_LOG)[2], history);
            fixture.commands.lock().unwrap().clear();
            let before = fixture.checkouts.load(Ordering::SeqCst);
            let bounded = fixture.read(window, &FixtureProof { append_after_head: false }).await.unwrap();
            let bounded_checkouts = fixture.checkouts.load(Ordering::SeqCst) - before;
            assert_eq!(bounded, legacy);
            assert_eq!(telemetry::snapshot(AGENT_LOG)[2], history + 200);
            assert_eq!(bounded_checkouts, 2, "head plus one bounded window, proof stub excluded");
            assert!(bounded_checkouts < legacy_checkouts);
            let commands = fixture.commands.lock().unwrap().clone();
            assert_eq!(commands.len(), 2);
            assert_eq!(commands[0].get_document("lsid").unwrap(), commands[1].get_document("lsid").unwrap());
            assert!(command_uses_snapshot_read(&commands[0]));
            assert!(command_uses_snapshot_read(&commands[1]));
            assert_eq!(commands[1].get_i64("limit").unwrap(), 200);
            assert_eq!(commands[1].get_document("filter").unwrap().get_document("seq").unwrap(), &doc! {"$gt":(history-200) as i64,"$lte":history as i64});
            eprintln!("public_window_consumer history={history} page=200 legacy_checkouts={legacy_checkouts} injected_proof_checkouts={bounded_checkouts} legacy_documents={history} bounded_documents=200; real metadata proof overhead not included");
            let explain = fixture.client.database(&fixture.name).run_command(doc! {
                "explain": { "find": AGENT_LOG,
                    "filter": { "agent_id": fixture.id.as_str(), "seq": { "$gt": (history - 200) as i64, "$lte": history as i64 } },
                    "sort": { "seq": 1 }, "limit": 200_i64 },
                "verbosity": "executionStats"
            }).await.unwrap();
            let stats = explain.get_document("executionStats").unwrap();
            let examined = match stats.get("totalDocsExamined").unwrap() {
                Bson::Int32(value) => i64::from(*value),
                Bson::Int64(value) => *value,
                _ => panic!("invalid explain document count"),
            };
            assert_eq!(examined, 200, "window query must use the Agent/sequence index");
            let empty = fixture.read(Window::Page { after: history, limit: 200 }, &FixtureProof { append_after_head: false }).await.unwrap();
            assert!(empty.is_empty());
            assert_eq!(telemetry::snapshot(AGENT_LOG)[2], history + 200);
        }).await;
        fixture.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn public_window_refuses_gaps_corruption_and_failed_proofs() {
    let fixture = Fixture::new(405).await;
    let window = Window::Range {
        after: 400,
        through: 405,
        limit: 5,
    };
    let proof = FixtureProof {
        append_after_head: false,
    };
    assert_eq!(fixture.read(window, &proof).await.unwrap().len(), 5);
    fixture
        .store
        .agent_log
        .delete_one(doc! {"agent_id":fixture.id.as_str(),"seq":403_i64})
        .await
        .unwrap();
    assert_eq!(
        fixture.read(window, &proof).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    // Missing or stale trust selects the full scanner, including rows outside
    // the requested prefix and even for an empty page.
    assert_eq!(
        fixture
            .read(Window::Prefix { through: 200 }, &LegacyIntegrity)
            .await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert_eq!(
        fixture
            .read(
                Window::Page {
                    after: 405,
                    limit: 1
                },
                &LegacyIntegrity
            )
            .await,
        Err(SandboxStoreError::SequenceConflict)
    );
    fixture
        .store
        .agent_log
        .insert_one(log_document(&entry(&fixture.id, 403)).unwrap())
        .await
        .unwrap();
    fixture
        .store
        .agent_log
        .update_one(
            doc! {"agent_id":fixture.id.as_str(),"seq":403_i64},
            doc! {"$set":{"record.agent_id":"foreign"}},
        )
        .await
        .unwrap();
    assert!(matches!(
        fixture.read(window, &proof).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    let before = fixture.checkouts.load(Ordering::SeqCst);
    assert_eq!(
        fixture.read(window, &RejectedProof).await,
        Err(SandboxStoreError::Corrupt("fixture quarantine".into()))
    );
    assert_eq!(
        fixture.checkouts.load(Ordering::SeqCst),
        before,
        "proof failure must not fall back or read a window"
    );
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn public_window_uses_the_proof_snapshot_across_concurrent_append() {
    let fixture = Fixture::new(200).await;
    let proof = FixtureProof {
        append_after_head: true,
    };
    let window = Window::Page {
        after: 195,
        limit: 10,
    };
    let bounded = fixture.read(window, &proof).await.unwrap();
    assert_eq!(bounded.len(), 5);
    assert_eq!(bounded.last().unwrap().seq, 200);
    let next = fixture.read(window, &LegacyIntegrity).await.unwrap();
    assert_eq!(next.len(), 6);
    assert_eq!(next.last().unwrap().seq, 201);
    fixture.cleanup().await;
}

mod prepared;
