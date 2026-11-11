use super::bounded_compatibility_tests::old_writer_append;
use super::bounded_fixtures::prepare_history;
use super::*;
use crate::store::agent_log_integrity::{parse_integrity, INTEGRITY_FIELD, QUARANTINE_FIELD};
use crate::store::agent_log_telemetry::{isolated_test as isolated, snapshot, ScanCaller};

fn kind() -> AgentLogKind {
    AgentLogKind::WorkStepCompleted {
        step: AgentWorkStep::None,
    }
}

async fn remove_metadata(store: &MongoSandboxStore, id: &AgentId) {
    store
        .agents
        .update_one(
            doc! { "_id": id.as_str() },
            doc! { "$unset": { INTEGRITY_FIELD: "" }},
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn fleet_first_append_prepares_lazily_and_restart_reuses_durable_proof() {
    let (client, name) = database("fleet_lazy").await;
    let (legacy, agent, _) = fixture(&name, &client).await;
    let (mut store, agent) = prepare_history(legacy, agent, 1_000).await;
    remove_metadata(&store, &agent.id).await;
    store.bounded_admission_agents.clear();
    let store = store.with_fleet_log_percentage(100);
    let before = store
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert!(!before.contains_key(INTEGRITY_FIELD));
    isolated(async {
        assert_eq!(snapshot(ScanCaller::RecoveryProof, true)[0], 0);
        assert_eq!(
            store
                .append_agent_log(&agent.id, NOW.into(), kind())
                .await
                .unwrap()
                .seq,
            1_001
        );
        assert_eq!(
            snapshot(ScanCaller::RecoveryProof, true)[0],
            1,
            "first append audits once"
        );
        assert_eq!(snapshot(ScanCaller::Other, true)[0], 0);
        store
            .append_agent_log(&agent.id, NOW.into(), kind())
            .await
            .unwrap();
        assert_eq!(snapshot(ScanCaller::RecoveryProof, true)[0], 1);
        assert_eq!(snapshot(ScanCaller::Other, true)[0], 0);
    })
    .await;
    let restarted = MongoSandboxStore::open(&client, &name)
        .await
        .unwrap()
        .with_fleet_log_percentage(100);
    isolated(async {
        restarted
            .append_agent_log(&agent.id, NOW.into(), kind())
            .await
            .unwrap();
        assert_eq!(
            snapshot(ScanCaller::RecoveryProof, true)[0],
            0,
            "restart revalidates durable proof"
        );
        assert_eq!(snapshot(ScanCaller::Other, true)[0], 0);
    })
    .await;
    let row = restarted
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(parse_integrity(&row).unwrap().unwrap().entry_count, 1_003);
    assert!(!row.contains_key(QUARANTINE_FIELD));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn fleet_quarantine_and_failed_preparation_are_isolated_per_agent() {
    let (client, name) = database("fleet_isolation").await;
    let (legacy, bad, parent) = fixture(&name, &client).await;
    let (store, bad) = prepare_history(legacy, bad, 1_000).await;
    let healthy = ready_agent(&parent, "agt_fleet_healthy");
    insert_agent(&store, &healthy, NOW).await;
    let (mut store, healthy) = prepare_history(store, healthy, 1_000).await;
    store.bounded_admission_agents.clear();
    let store = store.with_fleet_log_percentage(100);
    for id in [&bad.id, &healthy.id] {
        remove_metadata(&store, id).await;
    }
    store
        .agent_log
        .delete_one(doc! { "agent_id": bad.id.as_str(), "seq": 500_i64 })
        .await
        .unwrap();
    assert!(store
        .append_agent_log(&bad.id, NOW.into(), kind())
        .await
        .is_err());
    let marker = store
        .agents
        .find_one(doc! { "_id": bad.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert!(marker.contains_key(QUARANTINE_FIELD));
    assert_eq!(
        store
            .append_agent_log(&healthy.id, NOW.into(), kind())
            .await
            .unwrap()
            .seq,
        1_001
    );
    let second = store
        .agents
        .find_one(doc! { "_id": healthy.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert!(parse_integrity(&second).unwrap().unwrap().semantic.valid);
    assert!(!second.contains_key(QUARANTINE_FIELD));
    assert!(matches!(
        store.append_agent_log(&bad.id, NOW.into(), kind()).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    assert_eq!(
        store
            .agent_log
            .count_documents(doc! { "agent_id": bad.id.as_str() })
            .await
            .unwrap(),
        999
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn fleet_mixed_writers_and_kill_switch_preserve_history_and_repair_stale_proof() {
    let (client, name) = database("fleet_mixed").await;
    let (legacy, agent, _) = fixture(&name, &client).await;
    let (mut store, agent) = prepare_history(legacy, agent, 1_000).await;
    store.bounded_admission_agents.clear();
    let store = store.with_fleet_log_percentage(100);
    store.prepare_fleet_log(&agent.id).await;
    let (old, new) = tokio::join!(
        old_writer_append(&store, &agent.id),
        store.append_agent_log(&agent.id, NOW.into(), kind())
    );
    let mut sequences = [old, new.unwrap().seq];
    sequences.sort();
    assert_eq!(sequences, [1_001, 1_002]);
    old_writer_append(&store, &agent.id).await;
    isolated(async {
        store
            .append_agent_log(&agent.id, NOW.into(), kind())
            .await
            .unwrap();
        assert_eq!(
            snapshot(ScanCaller::Other, true)[0],
            1,
            "cached scheduling cannot grant stale trust"
        );
        store
            .append_agent_log(&agent.id, NOW.into(), kind())
            .await
            .unwrap();
        assert_eq!(snapshot(ScanCaller::Other, true)[0], 1);
    })
    .await;
    let disabled = MongoSandboxStore::open(&client, &name).await.unwrap();
    isolated(async {
        disabled
            .append_agent_log(&agent.id, NOW.into(), kind())
            .await
            .unwrap();
        assert_eq!(snapshot(ScanCaller::Other, true)[0], 1);
        assert_eq!(snapshot(ScanCaller::RecoveryProof, true)[0], 0);
    })
    .await;
    let resumed = disabled.with_fleet_log_percentage(100);
    isolated(async {
        resumed
            .append_agent_log(&agent.id, NOW.into(), kind())
            .await
            .unwrap();
        assert_eq!(snapshot(ScanCaller::Other, true)[0], 0);
        assert_eq!(snapshot(ScanCaller::RecoveryProof, true)[0], 0);
    })
    .await;
    assert_eq!(
        resumed
            .agent_log
            .count_documents(doc! { "agent_id": agent.id.as_str() })
            .await
            .unwrap(),
        1_007
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn fleet_message_reservation_keeps_exact_replay_and_authority_deadlines() {
    let (client, name) = database("fleet_command").await;
    let (legacy, agent, parent) = fixture(&name, &client).await;
    let (mut store, agent) = prepare_history(legacy, agent, 1_000).await;
    store.bounded_admission_agents.clear();
    remove_metadata(&store, &agent.id).await;
    let store = store.with_fleet_log_percentage(100);
    let mut expired = request(&agent, &parent, "expired");
    expired.authority_deadline = Some(tokio::time::Instant::now());
    assert!(store.reserve_command(expired).await.is_err());
    let before = store
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert!(
        !before.contains_key(INTEGRITY_FIELD),
        "expired authority cannot initiate preparation"
    );
    let request = request(&agent, &parent, "fleet-message");
    let first = store.reserve_command(request.clone()).await.unwrap();
    let first = record(first);
    let retry = record(store.reserve_command(request).await.unwrap());
    assert_eq!(first, retry);
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn failed_fleet_preparation_keeps_the_existing_append_fallback_usable() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let app = format!("fleet-preparation-{}", uuid::Uuid::new_v4().simple());
    let name = format!("cma_fleet_failure_{}", uuid::Uuid::new_v4().simple());
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app.clone());
    let client = Client::with_options(options).unwrap();
    let (legacy, agent, _) = fixture(&name, &client).await;
    let (mut store, agent) = prepare_history(legacy, agent, 1_000).await;
    store.bounded_admission_agents.clear();
    remove_metadata(&store, &agent.id).await;
    let store = store.with_fleet_log_percentage(100);
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": { "times": 1_i32 },
            "data": { "failCommands": ["update"], "appName": app,
                "errorCode": 91_i32, "errorLabels": ["TransientTransactionError"] }
        })
        .await
        .unwrap();
    store.prepare_fleet_log(&agent.id).await;
    admin
        .database("admin")
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await
        .unwrap();
    let row = store
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert!(!row.contains_key(INTEGRITY_FIELD));
    assert!(!row.contains_key(QUARANTINE_FIELD));
    isolated(async {
        assert_eq!(
            store
                .append_agent_log(&agent.id, NOW.into(), kind())
                .await
                .unwrap()
                .seq,
            1_001
        );
        assert_eq!(
            snapshot(ScanCaller::RecoveryProof, true)[0],
            0,
            "failed preparation is cooling down"
        );
        assert_eq!(
            snapshot(ScanCaller::Other, true)[0],
            1,
            "ordinary append repairs metadata"
        );
        store
            .append_agent_log(&agent.id, NOW.into(), kind())
            .await
            .unwrap();
        assert_eq!(snapshot(ScanCaller::Other, true)[0], 1);
    })
    .await;
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn fleet_legacy_semantic_ineligibility_keeps_the_complete_append_contract() {
    let (client, name) = database("fleet_legacy_fallback").await;
    // This retained fixture intentionally lacks a complete semantic prefix.
    // Structural appends remain valid under the existing compatibility contract.
    let (legacy, agent, _) = fixture(&name, &client).await;
    let store = legacy.with_fleet_log_percentage(100);
    isolated(async {
        let first = store
            .append_agent_log(&agent.id, NOW.into(), kind())
            .await
            .unwrap();
        assert_eq!(first.seq, agent.conversation.last_seq + 1);
        assert_eq!(snapshot(ScanCaller::Other, true)[0], 1);
        store
            .append_agent_log(&agent.id, NOW.into(), kind())
            .await
            .unwrap();
        assert_eq!(snapshot(ScanCaller::Other, true)[0], 2);
    })
    .await;
    let row = store
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert!(!row.contains_key(QUARANTINE_FIELD));
    assert!(!parse_integrity(&row).unwrap().unwrap().semantic.valid);
    client.database(&name).drop().await.unwrap();
}
