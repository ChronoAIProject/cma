use crate::store::{
    agent_log::log_document, agents::agent_document, sandbox_mongo::MongoSandboxStore,
    sandbox_mongo_integration_tests::resource_access_tests::fixture,
};
use layer2_core::{
    resource_access::ResourceAccessStore,
    sandbox::{
        Agent, AgentLogEntry, AgentLogKind, AgentStatus, AgentWorkStep, HierarchyReadService,
    },
    Authorization, Caller, Capability, SubjectId,
};
use mongodb::{bson::doc, Client};
use std::sync::Arc;

use super::super::{batch::pipeline, read_workspace};

const YESTERDAY: &str = "2026-11-19T17:00:00Z";
const TODAY: &str = "2026-11-20T09:00:00Z";

async fn append(store: &MongoSandboxStore, agent: &mut Agent, at: &str, kind: AgentLogKind) {
    agent.conversation.last_seq += 1;
    store
        .agent_log
        .insert_one(
            log_document(&AgentLogEntry {
                agent_id: agent.id.clone(),
                seq: agent.conversation.last_seq,
                appended_at: at.into(),
                kind,
            })
            .unwrap(),
        )
        .await
        .unwrap();
    agent.revision += 1;
    agent.updated_at = at.into();
    store
        .agents
        .replace_one(
            doc! {"_id": agent.id.as_str()},
            agent_document(agent).unwrap(),
        )
        .await
        .unwrap();
}

async fn turn(store: &MongoSandboxStore, agent: &mut Agent, at: &str) {
    append(
        store,
        agent,
        at,
        AgentLogKind::OwnerMessage {
            content: "synthetic task".into(),
        },
    )
    .await;
    let turn_sequence = agent.conversation.last_seq + 1;
    append(
        store,
        agent,
        at,
        AgentLogKind::TurnStarted {
            turn_sequence,
            model: None,
            origin_key: None,
        },
    )
    .await;
    append(
        store,
        agent,
        at,
        AgentLogKind::TurnCompleted { turn_sequence },
    )
    .await;
}

async fn summary(store: &MongoSandboxStore, agent: &Agent) -> serde_json::Value {
    let caller = Caller {
        subject: agent.owner.clone(),
        authorization: Authorization::new([Capability::UseOwnSandboxes]),
    };
    let view = HierarchyReadService::new(Arc::new(store.clone()))
        .workspace(&caller, &agent.workspace_id)
        .await
        .unwrap();
    serde_json::to_value(&view.agents[0]).unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn task_activity_hierarchy_ignores_maintenance_and_tracks_later_turns() {
    let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let name = format!("cma_task_activity_{}", uuid::Uuid::new_v4().simple());
    let (store, target) = fixture(&client, &name).await;
    let mut agent = store
        .resource_access_snapshot(&target)
        .await
        .unwrap()
        .lineage
        .agent
        .unwrap();
    assert!(summary(&store, &agent)
        .await
        .get("task_activity_at")
        .is_none());
    turn(&store, &mut agent, YESTERDAY).await;
    let revision = agent.revision;
    for _ in 0..32 {
        append(
            &store,
            &mut agent,
            TODAY,
            AgentLogKind::WorkStepCompleted {
                step: AgentWorkStep::PublishConnection,
            },
        )
        .await;
    }
    let stored_before = store
        .agents
        .find_one(doc! {"_id": agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let value = summary(&store, &agent).await;
    assert_eq!(
        store
            .agents
            .find_one(doc! {"_id": agent.id.as_str()})
            .await
            .unwrap()
            .unwrap(),
        stored_before
    );
    assert_eq!(value["task_activity_at"], YESTERDAY);
    assert_eq!(value["updated_at"], TODAY);
    assert_eq!(value["task_status"], "completed");
    assert_eq!(agent.revision, revision + 32);
    turn(&store, &mut agent, TODAY).await;
    assert_eq!(summary(&store, &agent).await["task_activity_at"], TODAY);
    agent.status = AgentStatus::Paused;
    store
        .agents
        .replace_one(
            doc! {"_id": agent.id.as_str()},
            agent_document(&agent).unwrap(),
        )
        .await
        .unwrap();
    let paused = summary(&store, &agent).await;
    assert_eq!(paused["task_activity_at"], TODAY);
    assert_eq!(paused["task_status"], "paused");
    let before = store
        .agents
        .find_one(doc! {"_id": agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let foreign = Caller {
        subject: SubjectId::new("other-owner"),
        authorization: Authorization::new([Capability::UseOwnSandboxes]),
    };
    assert!(matches!(
        HierarchyReadService::new(Arc::new(store.clone()))
            .workspace(&foreign, &agent.workspace_id)
            .await,
        Err(layer2_core::sandbox::HierarchyReadError::NotFound)
    ));
    assert_eq!(
        store
            .agents
            .find_one(doc! {"_id": agent.id.as_str()})
            .await
            .unwrap()
            .unwrap(),
        before
    );
    assert!(!before
        .get_document("record")
        .unwrap()
        .contains_key("task_activity_at"));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn task_activity_batches_project_headers_and_use_existing_index_with_head_fences() {
    use mongodb::{
        event::{command::CommandEvent, EventHandler},
        options::ClientOptions,
    };
    use std::sync::Mutex;
    let trace = Arc::new(Mutex::new(Vec::new()));
    let observed = trace.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            observed
                .lock()
                .unwrap()
                .push((event.command_name, event.command));
        }
    }));
    let client = Client::with_options(options).unwrap();
    let name = format!("cma_activity_batch_{}", uuid::Uuid::new_v4().simple());
    let (store, target) = fixture(&client, &name).await;
    let mut original = store
        .resource_access_snapshot(&target)
        .await
        .unwrap()
        .lineage
        .agent
        .unwrap();
    turn(&store, &mut original, YESTERDAY).await;
    trace.lock().unwrap().clear();
    let row = summary(&store, &original).await;
    assert_eq!(row["task_activity_at"], YESTERDAY);
    assert_eq!(row["task_status"], "completed");
    let workspace_aggregates = trace
        .lock()
        .unwrap()
        .iter()
        .filter(|(operation, command)| {
            operation == "aggregate" && matches!(command.get_str("aggregate"), Ok("agents"))
        })
        .count();
    assert_eq!(workspace_aggregates, 1);
    let mut agents = Vec::new();
    for index in 0..101 {
        let mut agent = original.clone();
        agent.id = layer2_core::AgentId(format!("agt_activity_{index:03}"));
        agent.creation_key = format!("activity_{index}");
        agent.worktree_id = layer2_core::WorktreeId(format!("wkt_activity_{index}"));
        agent.conversation.last_seq = 33;
        store
            .agents
            .insert_one(agent_document(&agent).unwrap())
            .await
            .unwrap();
        let mut logs = Vec::new();
        for seq in 1..=34 {
            let kind = if seq == 1 {
                AgentLogKind::ToolCallUpdated {
                    call_id: "call".into(),
                    status: "completed".into(),
                    title: Some("x".repeat(70 * 1024)),
                    output: None,
                }
            } else if seq == 34 {
                AgentLogKind::AgentMessage {
                    content: "above the snapshot head".into(),
                }
            } else {
                AgentLogKind::WorkStepCompleted {
                    step: AgentWorkStep::PublishConnection,
                }
            };
            logs.push(
                log_document(&AgentLogEntry {
                    agent_id: agent.id.clone(),
                    seq,
                    appended_at: if seq == 1 { YESTERDAY } else { TODAY }.into(),
                    kind,
                })
                .unwrap(),
            );
        }
        store.agent_log.insert_many(logs).await.unwrap();
        agents.push(agent);
    }
    // An unrelated Agent's newer eligible event cannot affect the requested batch.
    let mut foreign = original.clone();
    append(
        &store,
        &mut foreign,
        TODAY,
        AgentLogKind::OwnerMessage {
            content: "other task".into(),
        },
    )
    .await;
    let mut session = client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    trace.lock().unwrap().clear();
    let actual = read_workspace(&store, &agents, &mut session).await.unwrap();
    assert_eq!(actual.len(), 101);
    assert!(actual
        .values()
        .all(|row| row.activity_at.as_deref() == Some(YESTERDAY)));
    let commands = trace.lock().unwrap().clone();
    assert_eq!(commands.len(), 2);
    for (name, command) in &commands {
        assert_eq!(name, "aggregate");
        assert_eq!(command.get_i32("maxTimeMS").unwrap(), 10_000);
        let pipeline = command.get_array("pipeline").unwrap();
        let activity_lookups = pipeline
            .iter()
            .filter_map(|stage| stage.as_document()?.get_document("$lookup").ok())
            .filter(|lookup| {
                lookup
                    .get_str("as")
                    .is_ok_and(|name| name.starts_with("activity"))
            })
            .collect::<Vec<_>>();
        assert_eq!(activity_lookups.len(), 3);
        for lookup in activity_lookups {
            let stages = lookup.get_array("pipeline").unwrap();
            assert_eq!(
                stages[2].as_document().unwrap().get_i32("$limit").unwrap(),
                1
            );
            let projection = stages[3]
                .as_document()
                .unwrap()
                .get_document("$project")
                .unwrap();
            assert!(!projection.contains_key("record.kind.content"));
            assert!(!projection.contains_key("record.kind.output"));
            assert!(!projection.contains_key("record.kind.title"));
        }
    }
    // The same transaction keeps its original head even after an external update.
    let mut changed = agents[0].clone();
    changed.conversation.last_seq = 34;
    store
        .agents
        .replace_one(
            doc! {"_id": changed.id.as_str()},
            agent_document(&changed).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        read_workspace(&store, &agents[..1], &mut session)
            .await
            .unwrap()[&changed.id]
            .activity_at
            .as_deref(),
        Some(YESTERDAY)
    );
    session.abort_transaction().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    assert_eq!(
        read_workspace(&store, std::slice::from_ref(&changed), &mut session)
            .await
            .unwrap()[&changed.id]
            .activity_at
            .as_deref(),
        Some(TODAY)
    );
    session.abort_transaction().await.unwrap();
    let explain = client.database(&name).run_command(doc! {
        "explain": {"aggregate": "agents", "pipeline": pipeline(&[&agents[1]], true), "cursor": {}},
        "verbosity": "executionStats",
    }).await.unwrap();
    let lookups: Vec<_> = explain
        .get_array("stages")
        .unwrap()
        .iter()
        .filter_map(|value| value.as_document())
        .filter(|stage| {
            stage.get_document("$lookup").is_ok_and(|lookup| {
                lookup
                    .get_str("as")
                    .is_ok_and(|name| name.starts_with("activity"))
            })
        })
        .collect();
    assert_eq!(lookups.len(), 3, "{explain:?}");
    for lookup in lookups {
        assert!(
            lookup
                .get_array("indexesUsed")
                .unwrap()
                .contains(&"agent_log_task_status".into()),
            "{explain:?}"
        );
        let count = lookup
            .get("totalDocsExamined")
            .and_then(|v| v.as_i64().or_else(|| v.as_i32().map(i64::from)))
            .unwrap();
        assert!(count <= 2, "{explain:?}");
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn task_activity_missing_gap_and_inconsistent_evidence_stay_unknown() {
    let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let name = format!("cma_activity_unknown_{}", uuid::Uuid::new_v4().simple());
    let (store, target) = fixture(&client, &name).await;
    let mut agent = store
        .resource_access_snapshot(&target)
        .await
        .unwrap()
        .lineage
        .agent
        .unwrap();
    append(
        &store,
        &mut agent,
        TODAY,
        AgentLogKind::WorkStepCompleted {
            step: AgentWorkStep::PublishConnection,
        },
    )
    .await;
    assert!(summary(&store, &agent)
        .await
        .get("task_activity_at")
        .is_none());
    turn(&store, &mut agent, YESTERDAY).await;
    let started = store
        .agent_log
        .find_one(
            doc! {"agent_id": agent.id.as_str(), "seq": (agent.conversation.last_seq - 1) as i64},
        )
        .await
        .unwrap()
        .unwrap();
    store
        .agent_log
        .delete_one(doc! {"_id": started.get_str("_id").unwrap()})
        .await
        .unwrap();
    assert!(summary(&store, &agent)
        .await
        .get("task_activity_at")
        .is_none());
    store.agent_log.insert_one(started).await.unwrap();

    append(
        &store,
        &mut agent,
        TODAY,
        AgentLogKind::ConversationGap { frames: 1 },
    )
    .await;
    assert!(summary(&store, &agent)
        .await
        .get("task_activity_at")
        .is_none());
    let turn_sequence = agent.conversation.last_seq + 1;
    append(
        &store,
        &mut agent,
        TODAY,
        AgentLogKind::TurnStarted {
            turn_sequence,
            model: None,
            origin_key: None,
        },
    )
    .await;
    append(
        &store,
        &mut agent,
        TODAY,
        AgentLogKind::ConversationGap { frames: 1 },
    )
    .await;
    append(
        &store,
        &mut agent,
        TODAY,
        AgentLogKind::TurnCompleted { turn_sequence },
    )
    .await;
    assert!(summary(&store, &agent)
        .await
        .get("task_activity_at")
        .is_none());
    append(
        &store,
        &mut agent,
        TODAY,
        AgentLogKind::ApprovalRequested {
            approval_id: "approval".into(),
            action: None,
            detail: None,
            runtime_locator: None,
        },
    )
    .await;
    assert_eq!(summary(&store, &agent).await["task_activity_at"], TODAY);
    let seq = agent.conversation.last_seq as i64;
    for (field, value) in [
        (
            "record.agent_id",
            mongodb::bson::Bson::from("foreign-agent"),
        ),
        ("record.seq", mongodb::bson::Bson::from(seq + 1)),
        (
            "record.appended_at",
            mongodb::bson::Bson::from("not-a-time"),
        ),
        (
            "record.appended_at",
            mongodb::bson::Bson::from("x".repeat(8192)),
        ),
    ] {
        let original = store
            .agent_log
            .find_one(doc! {"agent_id": agent.id.as_str(), "seq": seq})
            .await
            .unwrap()
            .unwrap();
        store
            .agent_log
            .update_one(
                doc! {"_id": original.get_str("_id").unwrap()},
                doc! {"$set": {field: value}},
            )
            .await
            .unwrap();
        assert!(
            summary(&store, &agent)
                .await
                .get("task_activity_at")
                .is_none(),
            "{field}"
        );
        store
            .agent_log
            .replace_one(
                doc! {"_id": original.get_str("_id").unwrap()},
                original.clone(),
            )
            .await
            .unwrap();
    }
    client.database(&name).drop().await.unwrap();
}
