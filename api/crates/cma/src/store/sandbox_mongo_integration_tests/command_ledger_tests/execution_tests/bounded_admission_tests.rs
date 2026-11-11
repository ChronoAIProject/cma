use super::bounded_fixtures::prepare_history;
use super::*;
use crate::store::agent_log_integrity::{parse_integrity, INTEGRITY_FIELD, QUARANTINE_FIELD};
use crate::store::agent_log_telemetry::{isolated_test as isolated, snapshot, ScanCaller};
use crate::store::agents::agent_replacement_fields;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn zero_head_probes_all_rows_including_malformed_sequences() {
    for orphan in [
        None,
        Some(Bson::Int64(1)),
        Some(Bson::Int64(2)),
        Some(Bson::Int64(0)),
        Some(Bson::Int64(-1)),
        Some(Bson::String("malformed".into())),
    ] {
        let (client, name) = database("bounded_zero_head").await;
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let mut agent = ready_agent(&active_sandbox("sbx_zero"), "agt_zero");
        agent.conversation.last_seq = 0;
        agent.conversation.conversation_id = None;
        agent.conversation.runtime_generation = None;
        store.create_agent(agent.clone()).await.unwrap();
        if let Some(seq) = &orphan {
            store
                .agent_log
                .insert_one(doc! { "_id": "orphan", "document_schema_version": 1_i64,
                "agent_id": agent.id.as_str(), "seq": seq.clone(), "record": {} })
                .bypass_document_validation(true)
                .await
                .unwrap();
        }
        let result = store
            .append_agent_log(
                &agent.id,
                NOW.into(),
                AgentLogKind::Created {
                    workspace_id: agent.workspace_id.clone(),
                    worktree_id: agent.worktree_id.clone(),
                },
            )
            .await;
        assert_eq!(result.is_ok(), orphan.is_none(), "orphan={orphan:?}");
        let current = store.get_agent(&agent.id).await.unwrap().unwrap();
        assert_eq!(current.conversation.last_seq, u64::from(orphan.is_none()));
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn audit_detects_middle_damage_and_quarantines_across_restart() {
    let (client, name) = database("bounded_middle_audit").await;
    let (legacy, agent, _) = fixture(&name, &client).await;
    let (store, agent) = prepare_history(legacy, agent, 1000).await;
    store
        .agent_log
        .delete_one(doc! { "agent_id": agent.id.as_str(), "seq": 500_i64 })
        .await
        .unwrap();
    // The tail proof is intentionally not advertised as a proof of middle bytes.
    assert!(store
        .clone()
        .prepare_bounded_admission(std::slice::from_ref(&agent.id))
        .await
        .is_err());
    let restarted = MongoSandboxStore::open(&client, &name).await.unwrap();
    let row = restarted
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert!(row.contains_key(QUARANTINE_FIELD));
    assert!(restarted
        .append_agent_log(
            &agent.id,
            NOW.into(),
            AgentLogKind::WorkStepCompleted {
                step: AgentWorkStep::None
            }
        )
        .await
        .is_err());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn abort_and_concurrent_appends_keep_metadata_atomic() {
    use crate::store::sandbox_mongo::start_transaction;
    let (client, name) = database("bounded_atomicity").await;
    let (legacy, agent, _) = fixture(&name, &client).await;
    let (store, agent) = prepare_history(legacy, agent, 1000).await;
    let filter = doc! { "_id": agent.id.as_str() };
    let before = store
        .agents
        .find_one(filter.clone())
        .await
        .unwrap()
        .unwrap();
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let kind = AgentLogKind::WorkStepCompleted {
        step: AgentWorkStep::None,
    };
    assert!(crate::store::agent_log::append_in_transaction(
        &store,
        &mut session,
        &agent.id,
        NOW.into(),
        kind.clone()
    )
    .await
    .is_ok());
    session.abort_transaction().await.unwrap();
    assert_eq!(
        store
            .agents
            .find_one(filter.clone())
            .await
            .unwrap()
            .unwrap(),
        before
    );
    assert_eq!(
        store
            .agent_log
            .count_documents(doc! { "agent_id": agent.id.as_str() })
            .await
            .unwrap(),
        1000
    );
    let (a, b) = tokio::join!(
        store.append_agent_log(&agent.id, NOW.into(), kind.clone()),
        store.append_agent_log(&agent.id, NOW.into(), kind)
    );
    let mut sequences = [a.unwrap().seq, b.unwrap().seq];
    sequences.sort();
    assert_eq!(sequences, [1001, 1002]);
    let raw: Vec<mongodb::bson::RawDocumentBuf> = store
        .agent_log
        .clone_with_type()
        .find(doc! { "agent_id": agent.id.as_str() })
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let after = store.agents.find_one(filter).await.unwrap().unwrap();
    let integrity = parse_integrity(&after).unwrap().unwrap();
    assert_eq!(integrity.entry_count, 1002);
    assert_eq!(
        integrity.encoded_bytes,
        raw.iter().map(|row| row.as_bytes().len()).sum::<usize>()
    );
    // Restart/audit proves the exact raw tail hash and persisted reducer too.
    store
        .prepare_bounded_admission(std::slice::from_ref(&agent.id))
        .await
        .unwrap();
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn successful_last_normal_append_and_full_capacity_refusal_stay_bounded() {
    let (client, name) = database("bounded_capacity").await;
    let (legacy, agent, _) = fixture(&name, &client).await;
    let (store, agent) = prepare_history(legacy, agent, 9999).await;
    isolated(async {
        let kind = AgentLogKind::WorkStepCompleted {
            step: AgentWorkStep::None,
        };
        assert_eq!(
            store
                .append_agent_log(&agent.id, NOW.into(), kind.clone())
                .await
                .unwrap()
                .seq,
            10000
        );
        assert_eq!(
            store.append_agent_log(&agent.id, NOW.into(), kind).await,
            Err(SandboxStoreError::AgentLogCapacityExceeded)
        );
        assert_eq!(snapshot(ScanCaller::Other, true)[0], 0);
    })
    .await;
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn tail_tamper_cannot_be_repaired_by_a_successful_structural_scan() {
    let (client, name) = database("bounded_tail_tamper").await;
    let (legacy, agent, _) = fixture(&name, &client).await;
    let (store, agent) = prepare_history(legacy, agent, 1000).await;
    let before = store
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    store
        .agent_log
        .update_one(
            doc! { "agent_id": agent.id.as_str(), "seq": 1000_i64 },
            doc! { "$set": { "record.appended_at": TWO_MINUTES } },
        )
        .await
        .unwrap();
    for _ in 0..2 {
        assert!(store
            .append_agent_log(
                &agent.id,
                NOW.into(),
                AgentLogKind::WorkStepCompleted {
                    step: AgentWorkStep::None,
                }
            )
            .await
            .is_err());
    }
    let row = store
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert!(row.contains_key(QUARANTINE_FIELD));
    assert_eq!(row.get_i64("last_seq").unwrap(), 1000);
    assert_eq!(row.get(INTEGRITY_FIELD), before.get(INTEGRITY_FIELD));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn revision_carry_never_promotes_an_old_writer_checkpoint() {
    let (client, name) = database("bounded_revision").await;
    let (legacy, agent, _) = fixture(&name, &client).await;
    let (store, mut agent) = prepare_history(legacy, agent, 1000).await;
    let revision = agent.revision;
    agent.revision += 1;
    store.save_agent(revision, agent.clone()).await.unwrap();
    let row = store
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        parse_integrity(&row).unwrap().unwrap().bound_agent_revision,
        agent.revision
    );
    // Generation-173-shaped revision-only $set leaves the block bound to the old revision.
    let mut next = agent.clone();
    next.revision += 1;
    store
        .agents
        .update_one(
            doc! { "_id": agent.id.as_str() },
            doc! { "$set": agent_replacement_fields(&next).unwrap() },
        )
        .await
        .unwrap();
    let revision = next.revision;
    next.revision += 1;
    store.save_agent(revision, next.clone()).await.unwrap();
    let row = store
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        parse_integrity(&row).unwrap().unwrap().bound_agent_revision,
        agent.revision
    );
    isolated(async {
        store
            .append_agent_log(
                &agent.id,
                NOW.into(),
                AgentLogKind::WorkStepCompleted {
                    step: AgentWorkStep::None,
                },
            )
            .await
            .unwrap();
        assert_eq!(snapshot(ScanCaller::Other, true)[0], 1);
        store
            .append_agent_log(
                &agent.id,
                NOW.into(),
                AgentLogKind::WorkStepCompleted {
                    step: AgentWorkStep::None,
                },
            )
            .await
            .unwrap();
        assert_eq!(snapshot(ScanCaller::Other, true)[0], 1);
    })
    .await;
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn insert_failure_after_staged_metadata_aborts_the_entire_append() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let app_name = format!("cma-admission-atomic-{suffix}");
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    let client = Client::with_options(options).unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_admission_atomic_{suffix}");
    let (legacy, agent, _) = fixture(&name, &client).await;
    let (store, agent) = prepare_history(legacy, agent, 1000).await;
    let filter = doc! { "_id": agent.id.as_str() };
    let before = store
        .agents
        .find_one(filter.clone())
        .await
        .unwrap()
        .unwrap();
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": { "times": 1_i32 },
            "data": { "failCommands": ["insert"], "appName": app_name, "errorCode": 13_i32 }
        })
        .await
        .unwrap();
    let kind = AgentLogKind::WorkStepCompleted {
        step: AgentWorkStep::None,
    };
    let result = store
        .append_agent_log(&agent.id, NOW.into(), kind.clone())
        .await;
    admin
        .database("admin")
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await
        .unwrap();
    assert!(result.is_err());
    assert_eq!(
        store.agents.find_one(filter).await.unwrap().unwrap(),
        before
    );
    assert_eq!(
        store
            .agent_log
            .count_documents(doc! { "agent_id": agent.id.as_str() })
            .await
            .unwrap(),
        1000
    );
    assert_eq!(
        store
            .append_agent_log(&agent.id, NOW.into(), kind)
            .await
            .unwrap()
            .seq,
        1001
    );
    client.database(&name).drop().await.unwrap();
}
