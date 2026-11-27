use super::*;
use crate::store::agent_log_codec::log_document;
use layer2_core::sandbox::{AgentLogEntry, AgentLogKind};
use mongodb::bson::Bson;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn admission_rejects_mismatched_source_and_caller_evidence_without_writes() {
    let (client, name, store) = database().await;
    let (sandbox, agent) = fixture(&store, "sbx_activity_admission_mismatch").await;
    let initial = observe(&store, &sandbox.id).await;
    let id = doc! {"_id":sandbox.id.as_str()};
    let root = store.sandboxes.find_one(id.clone()).await.unwrap().unwrap();
    let agent_row = store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();

    for field in ["revision", "last_seq"] {
        let mut other = root.clone();
        other.insert(field, i64::MAX);
        assert_refused(&store, &other, std::slice::from_ref(&agent), &initial).await;
    }
    assert_refused(&store, &root, &[], &initial).await;

    let mut stale_agent = agent.clone();
    stale_agent.revision += 1;
    assert_refused(&store, &root, &[stale_agent], &initial).await;
    let mut stale_cursor = agent.clone();
    stale_cursor.conversation.last_seq += 1;
    assert_refused(&store, &root, &[stale_cursor], &initial).await;
    let mut foreign_agent = agent.clone();
    foreign_agent.id = layer2_core::AgentId("agt_other".into());
    assert_refused(&store, &root, &[foreign_agent], &initial).await;

    let mut other_process = initial.clone();
    other_process.source.process_incarnation = "previous-process".into();
    other_process.desktop.process_incarnation = "previous-process".into();
    assert_refused(&store, &root, std::slice::from_ref(&agent), &other_process).await;
    let mut other_database = initial.clone();
    other_database.source.database_id = "other-disposable-database".into();
    assert_refused(&store, &root, std::slice::from_ref(&agent), &other_database).await;
    let mut memory_coverage = initial.clone();
    memory_coverage.source.coverage = RuntimeActivityCoverage::MemoryFixtureV1;
    assert_refused(
        &store,
        &root,
        std::slice::from_ref(&agent),
        &memory_coverage,
    )
    .await;
    let mut old_desktop = initial.clone();
    old_desktop.desktop.tracking_incarnation = "previous-tracker".into();
    assert_refused(&store, &root, std::slice::from_ref(&agent), &old_desktop).await;
    assert_refused(
        &store.clone().with_renewable_work(true),
        &root,
        std::slice::from_ref(&agent),
        &initial,
    )
    .await;
    assert_eq!(store.sandboxes.find_one(id).await.unwrap(), Some(root));
    assert_eq!(
        store
            .agents
            .find_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap(),
        Some(agent_row)
    );
    client.database(&name).drop().await.unwrap();
}

async fn assert_refused(
    store: &MongoSandboxStore,
    root: &Document,
    agents: &[layer2_core::sandbox::Agent],
    snapshot: &RuntimeActivitySnapshot,
) {
    let proof = fixture_proof(snapshot.clone());
    let now = now_ms().unwrap();
    proof
        .validate_at(now)
        .expect("refused at the source recheck");
    let mut session = transaction(store, &proof).await;
    let result = validate_for_admission(store, root, agents, &proof, now, &mut session).await;
    assert!(
        matches!(
            result,
            Err(ApplyError::Domain(SandboxStoreError::InvalidTransaction(_)))
        ),
        "{result:?}"
    );
    session.abort_transaction().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn observer_refuses_a_missing_or_mismatched_segmented_log_head() {
    let (client, name, store) = database().await;
    let (sandbox, mut agent) = fixture(&store, "sbx_activity_segmented_mismatch").await;
    let created = AgentLogEntry {
        agent_id: agent.id.clone(),
        seq: 1,
        appended_at: agent.created_at.clone(),
        kind: AgentLogKind::Created {
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.worktree_id.clone(),
        },
    };
    store
        .agent_log
        .replace_one(
            doc! {"agent_id":agent.id.as_str()},
            log_document(&created).unwrap(),
        )
        .await
        .unwrap();
    let opened = AgentLogEntry {
        agent_id: agent.id.clone(),
        seq: 2,
        appended_at: agent.ready_at.clone().unwrap(),
        kind: AgentLogKind::ConversationOpened {
            conversation_id: agent.conversation.conversation_id.clone().unwrap(),
            runtime_generation: agent.conversation.runtime_generation.unwrap(),
        },
    };
    store
        .agent_log
        .insert_one(log_document(&opened).unwrap())
        .await
        .unwrap();
    agent.conversation.last_seq = 2;
    store
        .agents
        .replace_one(
            doc! {"_id":agent.id.as_str()},
            crate::store::agents::agent_document(&agent).unwrap(),
        )
        .await
        .unwrap();
    store.install_segmented_control_fixture(&agent).await;
    assert_eq!(observe(&store, &sandbox.id).await.vector.log_heads.len(), 1);
    let heads = &store.segment_logs.heads;
    let id = doc! {"_id":agent.id.as_str()};
    let saved = heads.find_one(id.clone()).await.unwrap().unwrap();
    for (field, value) in [
        ("incarnation", Bson::String("b".repeat(64))),
        (
            "revision",
            Bson::Int64(saved.get_i64("revision").unwrap() + 1),
        ),
    ] {
        let mut row = saved.clone();
        row.insert(field, value);
        heads.replace_one(id.clone(), row).await.unwrap();
        assert_unknown(&store, &sandbox.id).await;
    }
    let mut ahead = saved.clone();
    for field in [
        "last_seq",
        "active_count",
        "lifetime_count",
        "projection_through",
    ] {
        ahead.insert(field, ahead.get_i64(field).unwrap() + 1);
    }
    crate::store::agent_log_segments::Head::read(ahead.clone()).unwrap();
    heads.replace_one(id.clone(), ahead).await.unwrap();
    assert_unknown(&store, &sandbox.id).await;

    let mut foreign = saved.clone();
    foreign.insert("incarnation", "b".repeat(64));
    heads.replace_one(id.clone(), foreign).await.unwrap();
    let agents = &store.agents;
    let agent_before = agents.find_one(id.clone()).await.unwrap().unwrap();
    agents
        .update_one(
            id.clone(),
            doc! {"$set":{"agent_log_incarnation":"b".repeat(64)}},
        )
        .await
        .unwrap();
    assert_unknown(&store, &sandbox.id).await;
    agents.replace_one(id.clone(), agent_before).await.unwrap();

    heads.delete_one(id).await.unwrap();
    assert_unknown(&store, &sandbox.id).await;
    heads.insert_one(saved).await.unwrap();
    assert_eq!(observe(&store, &sandbox.id).await.vector.log_heads.len(), 1);
    client.database(&name).drop().await.unwrap();
}

async fn assert_unknown(store: &MongoSandboxStore, sandbox: &SandboxId) {
    assert!(matches!(
        store
            .observe_runtime_activity(sandbox, &context(store))
            .await
            .unwrap(),
        RuntimeActivityObservation::Unknown(RuntimeActivityUnknownReason::IncompleteCoverage)
    ));
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn capture_rejects_missing_or_mismatched_command_lane_and_log_tail() {
    use crate::store::sandbox_mongo_integration_tests::command_ledger_tests::{record, request};
    use layer2_core::sandbox::agent_io::AgentCommandStore;

    let (client, name, store) = database().await;
    let (sandbox, agent) = fixture(&store, "sbx_activity_command_mismatch").await;
    let admitted = record(
        store
            .reserve_command(request(&agent, &sandbox, "activity-proof"))
            .await
            .unwrap(),
    );
    let expected = observe(&store, &sandbox.id).await;
    assert!(expected.has_activity());
    assert_eq!(expected.vector.commands.len(), 1);

    let lane_id = doc! {"_id":agent.id.as_str()};
    let command_id = doc! {"_id":admitted.receipt.envelope.command_id.as_str()};
    let lanes = &store.agent_command_orders;
    let commands = &store.agent_commands;
    let lane = lanes.find_one(lane_id.clone()).await.unwrap().unwrap();
    let command = commands
        .find_one(command_id.clone())
        .await
        .unwrap()
        .unwrap();

    lanes.delete_one(lane_id.clone()).await.unwrap();
    assert_unknown(&store, &sandbox.id).await;
    lanes.insert_one(lane.clone()).await.unwrap();
    lanes
        .update_one(lane_id.clone(), doc! {"$set":{"last_order":2_i64}})
        .await
        .unwrap();
    assert_unknown(&store, &sandbox.id).await;
    lanes.replace_one(lane_id, lane).await.unwrap();

    let mut wrong_owner = command.clone();
    wrong_owner.insert("agent_id", "agt_other");
    commands
        .replace_one(command_id.clone(), wrong_owner)
        .await
        .unwrap();
    assert_unknown(&store, &sandbox.id).await;
    let mut later = command.clone();
    later.insert("_id", "cmd_later");
    later.insert("agent_order", 2_i64);
    later.insert("key_sha256", "c".repeat(64));
    later.insert("response_id", "rsp_later");
    commands
        .replace_one(command_id.clone(), command)
        .await
        .unwrap();
    commands.insert_one(later).await.unwrap();
    assert_unknown(&store, &sandbox.id).await;
    commands.delete_one(doc! {"_id":"cmd_later"}).await.unwrap();

    let last_seq = i64::try_from(expected.vector.agents[0].last_seq.unwrap()).unwrap();
    let log_id = doc! {"agent_id":agent.id.as_str(), "seq":last_seq};
    let log = store
        .agent_log
        .find_one(log_id.clone())
        .await
        .unwrap()
        .unwrap();
    store.agent_log.delete_one(log_id.clone()).await.unwrap();
    assert_unknown(&store, &sandbox.id).await;
    let mut future = log.clone();
    future.insert("seq", last_seq + 1);
    future
        .get_document_mut("record")
        .unwrap()
        .insert("seq", last_seq + 1);
    store.agent_log.insert_one(future).await.unwrap();
    assert_unknown(&store, &sandbox.id).await;
    store
        .agent_log
        .replace_one(doc! {"_id":log.get_str("_id").unwrap()}, log.clone())
        .await
        .unwrap();

    let restored = observe(&store, &sandbox.id).await;
    assert_eq!(restored.vector, expected.vector);
    client.database(&name).drop().await.unwrap();
}
