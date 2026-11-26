use super::*;
use crate::store::sandbox_maintenance::evidence::legacy_log_budget::{
    MAX_LEGACY_BYTES, MAX_LEGACY_ROWS,
};
use layer2_core::sandbox::store::{InactivityPauseCause, SandboxSessionStore};
use mongodb::{
    bson::{Bson, Document},
    event::{command::CommandEvent, EventHandler},
};
use std::sync::Mutex;

mod integrity;

struct TestStore {
    store: MongoSandboxStore,
    commands: Arc<Mutex<Vec<Document>>>,
    policy_revision: u64,
}

impl TestStore {
    async fn open() -> Self {
        let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        let commands = Arc::new(Mutex::new(Vec::new()));
        let observed = commands.clone();
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            if let CommandEvent::Started(event) = event {
                if event.command.get_str(&event.command_name).ok() == Some("agent_log") {
                    observed.lock().unwrap().push(event.command);
                }
            }
        }));
        let client = Client::with_options(options).unwrap();
        let name = format!("cma_log_budget_{}", uuid::Uuid::new_v4().simple());
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let (_, policy) = policy_tests::enable(&client, &name).await;
        Self {
            store: store.with_bounded_inactivity_legacy_log(true),
            commands,
            policy_revision: policy.revision,
        }
    }

    fn request(&self, sandbox: &Sandbox) -> SandboxInactivityPauseRequest {
        SandboxInactivityPauseRequest {
            expected_policy_revision: Some(self.policy_revision),
            ..policy_tests::global(&sandbox.id)
        }
    }

    fn commands(&self) -> Vec<Document> {
        std::mem::take(&mut *self.commands.lock().unwrap())
    }

    async fn cleanup(self) {
        self.store
            .client
            .database(&self.store.agent_log.namespace().db)
            .drop()
            .await
            .unwrap();
    }
}

fn log(agent: &Agent, seq: u64, kind: AgentLogKind) -> AgentLogEntry {
    AgentLogEntry {
        agent_id: agent.id.clone(),
        seq,
        appended_at: IDLE_AT.into(),
        kind,
    }
}

fn small_history(agent: &Agent, rows: u64) -> Vec<AgentLogEntry> {
    (1..=rows)
        .map(|seq| {
            let kind = match seq {
                1 => AgentLogKind::Created {
                    workspace_id: agent.workspace_id.clone(),
                    worktree_id: agent.worktree_id.clone(),
                },
                2 if agent.conversation.conversation_id.is_some() => {
                    AgentLogKind::ConversationOpened {
                        conversation_id: agent.conversation.conversation_id.clone().unwrap(),
                        runtime_generation: 1,
                    }
                }
                _ => AgentLogKind::ModeChanged {
                    mode: "code".into(),
                },
            };
            log(agent, seq, kind)
        })
        .collect()
}

async fn seed_legacy(store: &MongoSandboxStore, agent: &Agent, entries: &[AgentLogEntry]) {
    store
        .agents
        .insert_one(agent_document(agent).unwrap())
        .await
        .unwrap();
    if !entries.is_empty() {
        store
            .agent_log
            .insert_many(entries.iter().map(|entry| log_document(entry).unwrap()))
            .await
            .unwrap();
    }
}

fn assert_probe(command: &Document, ids: &[&str]) {
    assert_eq!(command.get_str("hint").unwrap(), "agent_log_sequence");
    assert_eq!(command.get_bool("autocommit").ok(), Some(false));
    let pipeline = command.get_array("pipeline").unwrap();
    assert_eq!(pipeline.len(), 3);
    assert_eq!(
        pipeline[0],
        Bson::Document(doc! {"$match": {"agent_id": {"$in": ids}}}),
    );
    assert_eq!(
        pipeline[1],
        Bson::Document(doc! {"$limit": MAX_LEGACY_ROWS + 1})
    );
    assert_eq!(
        pipeline[2],
        Bson::Document(doc! {"$group": {
            "_id": null,
            "rows": {"$sum": 1_i64},
            "bytes": {"$sum": {"$toLong": {"$bsonSize": "$$ROOT"}}},
        }})
    );
}

async fn assert_unchanged(store: &MongoSandboxStore, sandbox: &Sandbox) {
    assert_eq!(
        store.get_sandbox(&sandbox.id).await.unwrap(),
        Some(sandbox.clone())
    );
    assert!(store
        .sandbox_events(&sandbox.id, 0, 10)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn bounded_legacy_and_strict_session_evidence_gate_the_same_idle_pause() {
    let f = TestStore::open().await;
    let store = f
        .store
        .clone()
        .with_strict_inactivity_session_evidence(true);
    let (sandbox, agent) = fixture(&store, "sbx_combined_inactivity_evidence").await;
    let request = f.request(&sandbox);

    store
        .open_sandbox_session(&sandbox.id, "desktop", "2026-09-05T01:00:00Z")
        .await
        .unwrap();
    f.commands();
    assert_eq!(
        store
            .preview_inactivity_pause(request.clone())
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    let commands = f.commands();
    let [probe] = commands.as_slice() else {
        panic!("expected the budget probe to fit and the session to prevent the log payload read")
    };
    assert_probe(probe, &[agent.id.as_str()]);
    assert_eq!(
        store
            .schedule_inactivity_pause(request.clone())
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    assert_unchanged(&store, &sandbox).await;

    store
        .close_sandbox_session(&sandbox.id, "desktop")
        .await
        .unwrap();
    f.commands();
    assert_eq!(
        store
            .preview_inactivity_pause(request.clone())
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    let commands = f.commands();
    assert!(commands
        .iter()
        .any(|command| command.contains_key("aggregate")));
    assert!(commands.iter().any(|command| command.contains_key("find")));
    assert_unchanged(&store, &sandbox).await;

    assert_eq!(
        store
            .schedule_inactivity_pause(request.clone())
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    assert_eq!(
        store.schedule_inactivity_pause(request).await.unwrap(),
        SandboxMaintenanceOutcome::AlreadyScheduled
    );
    let paused = store.get_sandbox(&sandbox.id).await.unwrap().unwrap();
    assert_eq!(paused.status, SandboxStatus::Pausing);
    let events = store.sandbox_events(&sandbox.id, 0, 10).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].kind,
        SandboxEventKind::StatusChanged {
            from: SandboxStatus::Active,
            to: SandboxStatus::Pausing,
        }
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn legacy_budget_counts_all_live_histories_before_loading_payloads() {
    let f = TestStore::open().await;
    let sandbox = active_sandbox("sbx_legacy_rows");
    f.store.create_sandbox(sandbox.clone()).await.unwrap();
    let mut first = ready_agent(&sandbox, "agt_legacy_a");
    let mut second = ready_agent(&sandbox, "agt_legacy_b");
    first.conversation.last_seq = (MAX_LEGACY_ROWS / 2) as u64;
    second.conversation.last_seq = first.conversation.last_seq;
    seed_legacy(
        &f.store,
        &first,
        &small_history(&first, first.conversation.last_seq),
    )
    .await;
    seed_legacy(
        &f.store,
        &second,
        &small_history(&second, second.conversation.last_seq),
    )
    .await;
    f.commands();
    let request = f.request(&sandbox);
    assert_eq!(
        f.store
            .preview_inactivity_pause(request.clone())
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    let commands = f.commands();
    let aggregate = commands
        .iter()
        .find(|cmd| cmd.contains_key("aggregate"))
        .unwrap();
    let ids = [first.id.as_str(), second.id.as_str()];
    assert_probe(aggregate, &ids);
    let scan = commands
        .iter()
        .find(|cmd| cmd.contains_key("find"))
        .unwrap();
    assert_eq!(aggregate.get("lsid"), scan.get("lsid"));
    assert_eq!(aggregate.get("txnNumber"), scan.get("txnNumber"));
    assert_unchanged(&f.store, &sandbox).await;

    let extra = second.conversation.last_seq + 1;
    f.store
        .agent_log
        .insert_one(
            log_document(&log(
                &second,
                extra,
                AgentLogKind::ModeChanged {
                    mode: "code".into(),
                },
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    f.commands();
    assert_eq!(
        f.store
            .preview_inactivity_pause(request.clone())
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    let commands = f.commands();
    let [probe] = commands.as_slice() else {
        panic!("expected only the budget probe")
    };
    assert_probe(probe, &ids);
    assert_eq!(
        f.store
            .schedule_inactivity_pause(SandboxInactivityPauseRequest {
                cause: InactivityPauseCause::OperatorInactivity,
                ..request.clone()
            })
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    let commands = f.commands();
    let [probe] = commands.as_slice() else {
        panic!("expected only the budget probe")
    };
    assert_probe(probe, &ids);

    f.store
        .agents
        .update_one(
            doc! {"_id": second.id.as_str()},
            doc! {"$set": {"last_seq": extra as i64, "record.conversation.last_seq": extra as i64}},
        )
        .await
        .unwrap();
    assert_eq!(
        f.store.schedule_inactivity_pause(request).await.unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    assert!(f.commands().is_empty());
    assert_unchanged(&f.store, &sandbox).await;
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn legacy_budget_limits_byte_heavy_proofs_only_when_enabled_for_inactivity() {
    let mut f = TestStore::open().await;
    let sandbox = active_sandbox("sbx_legacy_bytes");
    f.store.create_sandbox(sandbox.clone()).await.unwrap();
    let agent = ready_agent(&sandbox, "agt_legacy_bytes");
    let oversized = log(
        &agent,
        1,
        AgentLogKind::AgentMessage {
            content: "x".repeat(MAX_LEGACY_BYTES as usize),
        },
    );
    seed_legacy(&f.store, &agent, &[oversized]).await;
    f.commands();
    let request = f.request(&sandbox);
    assert_eq!(
        f.store
            .schedule_inactivity_pause(request.clone())
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    let commands = f.commands();
    let [probe] = commands.as_slice() else {
        panic!("expected only the budget probe")
    };
    assert_probe(probe, &[agent.id.as_str()]);
    assert_unchanged(&f.store, &sandbox).await;

    assert_eq!(
        f.store
            .clone()
            .with_bounded_inactivity_legacy_log(false)
            .preview_inactivity_pause(request)
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    let commands = f.commands();
    assert!(commands.iter().any(|cmd| cmd.contains_key("find")));
    assert!(!commands.iter().any(|cmd| cmd.contains_key("aggregate")));
    platform_stop_integration_tests::install_stop(&f.store).await;
    assert_eq!(
        f.store
            .preview_inactivity_pause(SandboxInactivityPauseRequest {
                cause: InactivityPauseCause::PlatformSpendStop,
                pause_after_seconds: 0,
                ..f.request(&sandbox)
            })
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    let commands = f.commands();
    assert!(commands.iter().any(|cmd| cmd.contains_key("find")));
    assert!(!commands.iter().any(|cmd| cmd.contains_key("aggregate")));
    assert_unchanged(&f.store, &sandbox).await;

    // The spend-stop fixture installs a fresh settings record.
    let (_, policy) =
        policy_tests::enable(&f.store.client, &f.store.agent_log.namespace().db).await;
    f.policy_revision = policy.revision;
    let size_of_record = log_document(&log(
        &agent,
        1,
        AgentLogKind::AgentMessage {
            content: String::new(),
        },
    ))
    .unwrap()
    .to_vec()
    .unwrap()
    .len();
    let exactly_at_limit = log_document(&log(
        &agent,
        1,
        AgentLogKind::AgentMessage {
            content: "x".repeat(MAX_LEGACY_BYTES as usize - size_of_record),
        },
    ))
    .unwrap();
    f.store
        .agent_log
        .replace_one(doc! {"agent_id": agent.id.as_str()}, exactly_at_limit)
        .await
        .unwrap();
    assert_eq!(
        f.store
            .preview_inactivity_pause(f.request(&sandbox))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    assert_unchanged(&f.store, &sandbox).await;

    // The required hint is missing, so the budget cannot establish eligibility.
    f.store
        .agent_log
        .drop_index("agent_log_sequence")
        .await
        .unwrap();
    f.commands();
    assert!(matches!(
        f.store.schedule_inactivity_pause(f.request(&sandbox)).await,
        Err(SandboxStoreError::Unavailable(_))
    ));
    let commands = f.commands();
    assert_eq!(commands.len(), 1);
    assert_probe(&commands[0], &[agent.id.as_str()]);
    assert_unchanged(&f.store, &sandbox).await;
    f.cleanup().await;
}
