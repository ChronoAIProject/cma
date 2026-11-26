use super::*;
use crate::store::agent_log_segments::{audit, mongo_tests::fixtures, schema};
use layer2_core::sandbox::{store::AgentLogStore, AgentStatus, TypedFailure};
use mongodb::{
    bson,
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst},
    Arc,
};

const NOW: &str = "2026-11-01T00:00:00Z";

#[derive(Default)]
struct Trace {
    enabled: AtomicBool,
    bytes: AtomicUsize,
    writes: AtomicUsize,
    source: AtomicUsize,
    commands: AtomicUsize,
}
impl Trace {
    fn reset(&self) {
        for value in [&self.bytes, &self.writes, &self.source, &self.commands] {
            value.store(0, SeqCst);
        }
        self.enabled.store(true, SeqCst);
    }
    fn finish(&self) -> (usize, usize, usize, usize) {
        self.enabled.store(false, SeqCst);
        (
            self.bytes.load(SeqCst),
            self.writes.load(SeqCst),
            self.source.load(SeqCst),
            self.commands.load(SeqCst),
        )
    }
}

async fn store() -> (MongoSandboxStore, mongodb::Database, Arc<Trace>) {
    let trace = Arc::new(Trace::default());
    let seen = trace.clone();
    let name = format!("cma_preview_{}", uuid::Uuid::new_v4().simple());
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.app_name = Some(name.clone());
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if !seen.enabled.load(SeqCst) {
            return;
        }
        match event {
            CommandEvent::Started(e) => {
                seen.commands.fetch_add(1, SeqCst);
                if e.command.get_str("find").ok() == Some("agent_log") {
                    seen.source.fetch_add(1, SeqCst);
                }
                if matches!(
                    e.command_name.as_str(),
                    "insert"
                        | "update"
                        | "delete"
                        | "findAndModify"
                        | "createIndexes"
                        | "commitTransaction"
                ) {
                    seen.writes.fetch_add(1, SeqCst);
                }
            }
            CommandEvent::Succeeded(e) => {
                seen.bytes
                    .fetch_add(bson::serialize_to_vec(&e.reply).unwrap().len(), SeqCst);
            }
            _ => {}
        }
    }));
    let client = Client::with_options(options).unwrap();
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    (store, db, trace)
}

fn initial(agent: &Agent) -> Vec<AgentLogKind> {
    vec![
        AgentLogKind::Created {
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.worktree_id.clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: "preview-native".into(),
            runtime_generation: 1,
        },
        AgentLogKind::TurnStarted {
            turn_sequence: 3,
            origin_key: None,
            model: None,
        },
        AgentLogKind::OwnerMessage {
            content: "Synthetic owner input".into(),
        },
    ]
}

async fn install(
    store: &MongoSandboxStore,
    tail: Vec<AgentLogKind>,
    closed: i64,
    width: i64,
) -> Agent {
    let mut agent = fixtures::agent();
    agent.conversation.conversation_id = Some("preview-native".into());
    agent.conversation.runtime_generation = Some(1);
    let mut kinds = initial(&agent);
    kinds.extend(tail);
    agent.conversation.active_turn = if kinds.iter().any(|kind| {
        matches!(
            kind,
            AgentLogKind::TurnCompleted { .. }
                | AgentLogKind::TurnFailed { .. }
                | AgentLogKind::TurnInterrupted { .. }
        )
    }) {
        None
    } else {
        Some(3)
    };
    agent.status = if agent.conversation.active_turn.is_some() {
        AgentStatus::Working
    } else {
        AgentStatus::Ready
    };
    let entries = kinds
        .into_iter()
        .enumerate()
        .map(|(index, kind)| AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: index as u64 + 1,
            appended_at: NOW.into(),
            kind,
        })
        .collect();
    let (agent, _) = fixtures::install(store, agent, entries, closed, width).await;
    let db = store.client.database(&store.agents.namespace().db);
    for _ in 0..closed {
        assert_eq!(
            audit::run_unit(&store.client, &db, agent.id.as_str(), "preview-test", false)
                .await
                .unwrap(),
            "running"
        );
    }
    assert_eq!(
        audit::run_unit(&store.client, &db, agent.id.as_str(), "preview-test", false)
            .await
            .unwrap(),
        "complete"
    );
    assert_eq!(
        audit::install(&store.client, &db, agent.id.as_str(), "preview-test")
            .await
            .unwrap(),
        "installed"
    );
    agent
}

fn message(text: &str) -> AgentLogKind {
    AgentLogKind::AgentMessage {
        content: text.into(),
    }
}
fn failure(turn_sequence: u64) -> AgentLogKind {
    AgentLogKind::TurnFailed {
        turn_sequence,
        failure: TypedFailure {
            class: "synthetic_failure".into(),
            detail: "Synthetic failure".into(),
        },
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn latest_reply_in_active_completed_failed_and_interrupted_turns_is_read_only() {
    for terminal in [
        None,
        Some(AgentLogKind::TurnCompleted { turn_sequence: 3 }),
        Some(failure(3)),
        Some(AgentLogKind::TurnInterrupted { turn_sequence: 3 }),
    ] {
        let (store, db, trace) = store().await;
        let mut tail = vec![message("old reply"), message("  newest\n回复 🦀 ")];
        tail.extend(terminal);
        let agent = install(&store, tail, 1, 4).await;
        trace.reset();
        let result = store.agent_log_reply_preview(&agent).await.unwrap();
        let AgentReplyPreview::Ready {
            source_through_seq,
            reply,
        } = result
        else {
            panic!("{result:?}");
        };
        assert_eq!(
            (
                source_through_seq,
                reply.message_sequence,
                reply.turn_sequence
            ),
            (agent.conversation.last_seq, 6, 3)
        );
        assert_eq!(reply.text, "newest 回复 🦀");
        assert_eq!(reply.appended_at, NOW);
        let (bytes, writes, source, commands) = trace.finish();
        assert_eq!(writes, 0);
        assert!(bytes < 256 * 1024 && source > 0 && commands > source);
        eprintln!("preview source_reads={source} all_commands={commands} reply_bytes={bytes} writes={writes}");
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn newer_failure_without_a_reply_preserves_the_actual_previous_message() {
    let (store, db, _) = store().await;
    let agent = install(
        &store,
        vec![
            message("actual reply"),
            AgentLogKind::TurnCompleted { turn_sequence: 3 },
            AgentLogKind::TurnStarted {
                turn_sequence: 7,
                origin_key: None,
                model: None,
            },
            AgentLogKind::OwnerMessage {
                content: "next".into(),
            },
            failure(7),
        ],
        1,
        4,
    )
    .await;
    let AgentReplyPreview::Ready { reply, .. } = read(&store, &agent).await.unwrap() else {
        panic!("expected original reply");
    };
    assert_eq!(
        (
            reply.text.as_str(),
            reply.message_sequence,
            reply.turn_sequence
        ),
        ("actual reply", 5, 3)
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn only_certified_beginning_proves_none_and_tail_budget_never_skips_newer_rows() {
    for (text, padding, expected) in [
        (" \n\u{0000} ", 0, "none"),
        (" \n\u{0000} ", 150, "unavailable"),
        ("boundary reply", 31, "ready"),
        ("boundary reply", 32, "ready"),
        ("boundary reply", 127, "ready"),
        ("boundary reply", 128, "unavailable"),
    ] {
        let (store, db, trace) = store().await;
        let mut tail = vec![message(text)];
        tail.extend((0..padding).map(|_| AgentLogKind::ModeChanged {
            mode: "code".into(),
        }));
        let agent = install(&store, tail, 1, 4).await;
        trace.reset();
        let result = read(&store, &agent).await.unwrap();
        match (expected, result) {
            ("ready", AgentReplyPreview::Ready { reply, .. }) => {
                assert_eq!(reply.message_sequence, 5);
                assert_eq!(reply.text, text);
            }
            ("none", AgentReplyPreview::None { source_through_seq }) => {
                assert_eq!(source_through_seq, 5)
            }
            ("unavailable", AgentReplyPreview::Unavailable) => {}
            (_, actual) => panic!("padding={padding} expected={expected} actual={actual:?}"),
        }
        let (bytes, writes, source, commands) = trace.finish();
        assert_eq!(writes, 0);
        eprintln!("boundary padding={padding} source_reads={source} all_commands={commands} reply_bytes={bytes}");
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn legacy_source_refuses_before_any_log_or_projection_query() {
    let (store, db, trace) = store().await;
    let agent = fixtures::agent();
    store
        .agents
        .insert_one(crate::store::agents::agent_document(&agent).unwrap())
        .await
        .unwrap();
    trace.reset();
    assert_eq!(
        read(&store, &agent).await.unwrap(),
        AgentReplyPreview::Unavailable
    );
    let (bytes, writes, source, commands) = trace.finish();
    assert_eq!((writes, source, commands), (0, 0, 1));
    assert!(bytes < 16 * 1024);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn missing_newer_source_and_corrupt_message_membership_are_errors() {
    let (store, db, _) = store().await;
    let agent = install(&store, vec![message("old"), message("new")], 1, 4).await;
    let filter = doc! {"agent_id":agent.id.as_str(),"kind":"message","entity_key":"6"};
    store
        .segment_logs
        .evidence
        .update_one(filter, doc! {"$set":{"record.role":"owner"}})
        .await
        .unwrap();
    assert!(matches!(
        read(&store, &agent).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    store
        .agent_log
        .delete_one(doc! {"agent_id":agent.id.as_str(),"seq":6_i64})
        .await
        .unwrap();
    assert!(read(&store, &agent).await.is_err());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn large_active_unit_and_oversized_sealed_point_refuse_before_payload_download() {
    for sealed in [false, true] {
        let (store, db, trace) = store().await;
        let agent = install(
            &store,
            vec![message(&"x".repeat(9 * 1024 * 1024))],
            if sealed { 1 } else { 0 },
            5,
        )
        .await;
        trace.reset();
        assert_eq!(
            read(&store, &agent).await.unwrap(),
            AgentReplyPreview::Unavailable
        );
        let (bytes, writes, _, _) = trace.finish();
        assert_eq!(writes, 0);
        assert!(
            bytes < 64 * 1024,
            "oversized source was downloaded: {bytes}"
        );
        db.drop().await.unwrap();
    }
    let (store, db, trace) = store().await;
    let message = message(&"x".repeat(3 * 1024 * 1024));
    let agent = install(&store, vec![message.clone(), message], 1, 4).await;
    trace.reset();
    assert_eq!(
        read(&store, &agent).await.unwrap(),
        AgentReplyPreview::Unavailable
    );
    let (bytes, writes, source, commands) = trace.finish();
    assert_eq!(writes, 0);
    assert!(
        bytes < capture::preview_budget::MAX_SOURCE_BYTES,
        "aggregate download exceeded budget: {bytes}"
    );
    eprintln!("cumulative source_reads={source} all_commands={commands} reply_bytes={bytes}");
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set with enableTestCommands"]
async fn deadline_cancels_source_work_without_followup_queries_or_writes() {
    let (store, db, trace) = store().await;
    let agent = install(&store, vec![message("reply")], 1, 4).await;
    let control = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    control.database("admin").run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":1},"data":{"failCommands":["find"],"appName":db.name(),"blockConnection":true,"blockTimeMS":2500}}).await.unwrap();
    trace.reset();
    let before = std::time::Instant::now();
    assert_eq!(
        read(&store, &agent).await.unwrap(),
        AgentReplyPreview::Unavailable
    );
    assert!(before.elapsed() < std::time::Duration::from_secs(3));
    let commands = trace.commands.load(SeqCst);
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    assert_eq!(trace.commands.load(SeqCst), commands);
    assert_eq!(trace.finish().1, 0);
    control
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    db.drop().await.unwrap();
}
