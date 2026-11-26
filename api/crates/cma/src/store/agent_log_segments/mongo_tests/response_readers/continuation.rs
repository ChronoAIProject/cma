use super::*;
use crate::store::{
    agent_log_segments::{publication, writer_control, writer_gate},
    sandbox_maintenance::mongo_integration_tests::{active_sandbox, ready_agent},
    sandbox_mongo_integration_tests::agent_transaction_tests::capacity_tests::mongo::verify_full_log_cleanup,
};
use futures_util::FutureExt as _;
use layer2_core::sandbox::{
    store::{
        AgentConversationMutation, AgentConversationStore, AgentLogPublicationIdentity,
        SandboxMutationGuard, SandboxStore, SandboxStoreError,
    },
    AgentStatus, AgentTurnOutcome,
};
use mongodb::Database;
use std::{future::Future, panic::AssertUnwindSafe};

const MIN_EVENTS: i64 = 100_000;
const MIN_BYTES: i64 = 640 * 1024 * 1024;
const INITIAL_SEALS: i64 = 20;
const SEALED_WIDTH: i64 = 4_500;
const PAD: usize = 8 * 1024;

#[tokio::test]
#[ignore = "requires an opt-in disposable Mongo replica set and at least 640 MiB of logical history"]
async fn same_agent_continues_after_100000_events_and_640_mib() {
    disposable("continuation", |client, db| async move {
        continuation(&client, &db).await;
    })
    .await;
    disposable("corrupt", |client, db| async move {
        refuses_changed_original(&client, &db).await;
    })
    .await;
    // The disabled writer retains both its count/byte ceilings and cleanup reserves.
    verify_full_log_cleanup(false, None).await;
    verify_full_log_cleanup(true, None).await;
}

async fn disposable<Fut: Future<Output = ()>>(
    prefix: &str,
    run: impl FnOnce(Client, Database) -> Fut,
) {
    let (client, name) = database(prefix).await;
    let db = client.database(&name);
    let result = AssertUnwindSafe(run(client, db.clone()))
        .catch_unwind()
        .await;
    let cleanup = db.drop().await;
    if let Err(original) = result {
        if let Err(error) = cleanup {
            eprintln!("disposable database cleanup after failure: {error}");
        }
        std::panic::resume_unwind(original);
    }
    cleanup.expect("drop disposable database");
}

fn row(agent: &Agent, seq: i64, kind: AgentLogKind) -> AgentLogEntry {
    AgentLogEntry {
        agent_id: agent.id.clone(),
        seq: seq as u64,
        appended_at: NOW.into(),
        kind,
    }
}

fn mode(n: usize) -> AgentLogKind {
    AgentLogKind::ModeChanged {
        mode: "x".repeat(n),
    }
}

fn initial_rows(agent: &Agent) -> impl Iterator<Item = AgentLogEntry> + '_ {
    [
        AgentLogKind::Created {
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.worktree_id.clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: agent.conversation.conversation_id.clone().unwrap(),
            runtime_generation: 1,
        },
        AgentLogKind::TurnStarted {
            turn_sequence: 3,
            origin_key: None,
            model: Some("fixture-model".into()),
        },
        AgentLogKind::OwnerMessage {
            content: "first turn".into(),
        },
        AgentLogKind::AgentMessage {
            content: "first answer".into(),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
    ]
    .into_iter()
    .enumerate()
    .map(|(offset, kind)| row(agent, offset as i64 + 1, kind))
}

fn seed_rows(agent: &Agent) -> impl Iterator<Item = AgentLogEntry> + '_ {
    let initial = initial_rows(agent).collect::<Vec<_>>();
    let initial_bytes: i64 = initial.iter().map(encoded_size).sum();
    let fixed = initial.len() as i64;
    let empty_mode_bytes = encoded_size(&row(agent, MIN_EVENTS - 1, mode(0)));
    let sealed = INITIAL_SEALS * SEALED_WIDTH;
    let active = MIN_EVENTS - 1 - sealed;
    let wanted = MIN_BYTES - (PAD / 2) as i64;
    let sealed_payload =
        wanted - initial_bytes - (MIN_EVENTS - 1 - fixed) * empty_mode_bytes - active * 32;
    let ordinary = sealed_payload / (sealed - fixed);
    let extra = sealed_payload % (sealed - fixed);
    initial
        .into_iter()
        .chain((fixed + 1..MIN_EVENTS).map(move |seq| {
            let size = if seq <= sealed {
                ordinary + i64::from(seq <= fixed + extra)
            } else {
                32
            };
            row(agent, seq, mode(size as usize))
        }))
}

fn encoded_size(entry: &AgentLogEntry) -> i64 {
    bson::serialize_to_vec(&log_document(entry).unwrap())
        .unwrap()
        .len() as i64
}

async fn persisted_size(store: &MongoSandboxStore, agent: &Agent) -> (i64, i64) {
    let mut rows = store
        .agent_log
        .aggregate([
            doc! {"$match":{"agent_id":agent.id.as_str()}},
            doc! {"$group":{"_id":null,"events":{"$sum":1},"bytes":{"$sum":{"$bsonSize":"$$ROOT"}}}},
        ])
        .await
        .unwrap();
    let result = rows.try_next().await.unwrap().unwrap();
    let number = |key| {
        result
            .get_i64(key)
            .or_else(|_| result.get_i32(key).map(i64::from))
            .unwrap()
    };
    (number("events"), number("bytes"))
}

async fn open_writer(client: &Client, db: &Database) -> MongoSandboxStore {
    MongoSandboxStore::open(client, db.name())
        .await
        .unwrap()
        .with_segment_adoption_enabled(true)
}

fn identity(key: &str, kind: &AgentLogKind) -> AgentLogPublicationIdentity {
    AgentLogPublicationIdentity {
        key: key.into(),
        appended_at: NOW.into(),
        fingerprint: publication::request_digest(kind)
            .unwrap()
            .bytes
            .try_into()
            .unwrap(),
    }
}

async fn continuation(client: &Client, db: &Database) {
    schema::migrate(db).await.unwrap();
    let store = open_writer(client, db).await;
    let parent = active_sandbox("sbx_long_history");
    store.create_sandbox(parent.clone()).await.unwrap();
    let seed = ready_agent(&parent, "agt_long_history");
    let agent = super::super::fixtures::install_bounded(
        &store,
        seed.clone(),
        seed_rows(&seed),
        MIN_EVENTS - 1,
        INITIAL_SEALS,
        SEALED_WIDTH,
    )
    .await;
    fixtures::certify(client, db, &agent, INITIAL_SEALS).await;
    writer_control::execute(
        client,
        db,
        writer_gate::GateName::Segments,
        writer_control::Action::Replace {
            expected_revision: 0,
            policy: writer_gate::Policy {
                enabled: true,
                agents: vec![agent.id.to_string()],
                ..Default::default()
            },
        },
    )
    .await
    .unwrap();

    let before = persisted_size(&store, &agent).await;
    assert_eq!(before.0, MIN_EVENTS - 1);
    assert!(before.1 < MIN_BYTES, "seed must precede both thresholds");
    let crosses = mode(PAD);
    let crossing = store
        .append_agent_log_keyed(&agent.id, &identity("cross-thresholds", &crosses), crosses)
        .await
        .expect("normal writer crosses both history thresholds");
    let crossed = persisted_size(&store, &agent).await;
    assert_eq!(crossed, (MIN_EVENTS, before.1 + encoded_size(&crossing)));
    assert!(crossed.1 >= MIN_BYTES);
    assert_eq!(crossing.seq, MIN_EVENTS as u64);

    let expected = store.get_agent(&agent.id).await.unwrap().unwrap();
    drop(store);
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let fresh_client = Client::with_uri_str(&uri).await.unwrap();
    let restarted = open_writer(&fresh_client, db).await;
    let current = restarted.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(current, expected);
    let turn = current.conversation.last_seq + 1;
    let mut working = current.clone();
    working
        .transition(AgentStatus::Working, NOW.into(), None)
        .unwrap();
    working.conversation.active_turn = Some(turn);
    let start = publish_turn(
        &restarted,
        &current,
        working,
        Some(SandboxMutationGuard::capture(&parent)),
        vec![
            AgentLogKind::TurnStarted {
                turn_sequence: turn,
                origin_key: None,
                model: Some("fixture-model".into()),
            },
            AgentLogKind::OwnerMessage {
                content: "continue this conversation".into(),
            },
        ],
    )
    .await;
    assert_eq!(start[0].seq, turn);
    let head = Head::read(
        collection(&restarted, HEADS)
            .find_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(head.active_ordinal, INITIAL_SEALS + 1);
    assert_eq!(head.active_start_seq, turn as i64);

    let output = AgentLogKind::AgentMessage {
        content: "answer after restart".into(),
    };
    let key = identity("continued-output", &output);
    let appended = restarted
        .append_agent_log_keyed(&agent.id, &key, output.clone())
        .await
        .unwrap();
    let mut current = restarted.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(current.conversation.active_turn, Some(turn));
    let mut ready = current.clone();
    ready
        .transition(AgentStatus::Ready, NOW.into(), None)
        .unwrap();
    ready.conversation.active_turn = None;
    let terminal = publish_turn(
        &restarted,
        &current,
        ready,
        None,
        vec![AgentLogKind::TurnCompleted {
            turn_sequence: turn,
        }],
    )
    .await;
    assert_eq!(terminal.len(), 1);
    let after = terminal[0].seq;
    let replay_client = Client::with_uri_str(uri).await.unwrap();
    let replay = open_writer(&replay_client, db).await;
    assert_eq!(
        replay
            .append_agent_log_keyed(&agent.id, &key, output)
            .await
            .unwrap(),
        appended
    );
    current = replay.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(current.id, agent.id);
    assert_eq!(
        current.conversation.conversation_id,
        agent.conversation.conversation_id
    );
    assert_eq!(
        current.conversation.runtime_generation,
        agent.conversation.runtime_generation
    );
    assert_eq!(current.conversation.last_seq, after);
    assert_eq!(persisted_size(&replay, &agent).await.0, after as i64);

    let receipt = collection(&replay, PUBLICATIONS)
        .find(doc! {"agent_id":agent.id.as_str(),"publication_key":key.key})
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    assert_eq!(receipt.len(), 1);
    let receipt = publication::Publication::read(receipt.into_iter().next().unwrap()).unwrap();
    assert_eq!(
        (receipt.first_seq, receipt.last_seq),
        (appended.seq as i64, appended.seq as i64)
    );
    assert_eq!(
        receipt.result_digest,
        publication::result_digest(&appended).unwrap()
    );
    let terminal_receipt = collection(&replay, PUBLICATIONS)
        .find_one(
            doc! {"agent_id":agent.id.as_str(),"first_seq":after as i64,"last_seq":after as i64},
        )
        .await
        .unwrap()
        .expect("terminal publication has its own receipt");
    let terminal_receipt = publication::Publication::read(terminal_receipt).unwrap();
    assert_eq!(
        terminal_receipt.agent_digest,
        Some(publication::agent_digest(&current).unwrap())
    );

    let mut cursor = crossing.seq - 2;
    let mut pages = Vec::new();
    while cursor < after {
        let page = replay
            .agent_log_range(&agent.id, cursor, after, 3)
            .await
            .unwrap();
        assert!(!page.is_empty());
        for event in page {
            cursor += 1;
            assert_eq!((event.seq, &event.agent_id), (cursor, &agent.id));
            pages.push(event);
        }
    }
    assert_eq!(
        &pages[1..],
        &[vec![crossing], start, vec![appended], terminal.clone()].concat()
    );
    let mut reader = replay
        .agent_output_reader(&current, Some(turn))
        .await
        .unwrap()
        .unwrap();
    let facts = reader.metadata().turn.clone().unwrap();
    assert_eq!(facts.terminal_sequence, Some(after));
    assert!(matches!(
        facts.facts.outcome,
        AgentTurnOutcome::Completed { .. }
    ));
    assert_eq!(reader.points(&[after]).await.unwrap(), terminal);
    eprintln!(
        "retained events={} BSON bytes={} new turn={} terminal={}",
        crossed.0, crossed.1, turn, after
    );
}

async fn publish_turn(
    store: &MongoSandboxStore,
    current: &Agent,
    mut next: Agent,
    admission_guard: Option<SandboxMutationGuard>,
    agent_logs: Vec<AgentLogKind>,
) -> Vec<AgentLogEntry> {
    next.revision = current.revision + 1;
    next.conversation.last_seq = current.conversation.last_seq + agent_logs.len() as u64;
    store
        .commit_agent_conversation(AgentConversationMutation {
            settings_fence: None,
            command_claim: None,
            execution_fence: None,
            trigger_execution_fence: None,
            admission_guard,
            expected_agent_revision: current.revision,
            expected_last_seq: current.conversation.last_seq,
            runtime_generation: current.conversation.runtime_generation.unwrap(),
            agent: next,
            appended_at: NOW.into(),
            agent_logs,
        })
        .await
        .unwrap()
}

async fn refuses_changed_original(client: &Client, db: &Database) {
    schema::migrate(db).await.unwrap();
    let store = open_writer(client, db).await;
    let (agent, _) = fixtures::source(&store, 2, false).await;
    fixtures::certify(client, db, &agent, 2).await;
    let filter = doc! {"agent_id":agent.id.as_str(),"seq":3_i64};
    let mut reader = store
        .agent_output_reader(&agent, Some(3))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reader.points(&[3]).await.unwrap().len(), 1);
    drop(reader);
    let changed = store
        .agent_log
        .update_one(
            filter,
            doc! {"$set":{"record.appended_at":"2026-10-01T00:00:00Z"}},
        )
        .await
        .unwrap();
    assert_eq!(changed.modified_count, 1);
    assert!(
        matches!(
            store.agent_output_reader(&agent, Some(3)).await,
            Err(SandboxStoreError::Corrupt(_))
        ),
        "the reader must reject a changed original at capture"
    );
}
