use super::bounded_fixtures::prepare_history;
use super::*;
use crate::store::agent_log_integrity::{parse_integrity, INTEGRITY_FIELD, QUARANTINE_FIELD};
use crate::store::agent_log_telemetry::{isolated_test as isolated, snapshot, ScanCaller};
use crate::store::sandbox_mongo::{is_transient, start_transaction};

// Reproduce generation 173's transaction/head write shape without installing
// new metadata. This is a protocol race test, not an executed old-binary test.
pub(super) async fn old_writer_append(store: &MongoSandboxStore, id: &AgentId) -> u64 {
    for _ in 0..16 {
        let mut session = store.client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        let result = async {
            let row = store.agents.find_one(doc! { "_id": id.as_str() })
                .session(&mut session).await?.unwrap();
            let agent = crate::store::agents::read_agent(&row).unwrap();
            let rows: Vec<Document> = store.agent_log.find(doc! { "agent_id": id.as_str() })
                .sort(doc! { "seq": 1 }).session(&mut session).await?
                .stream(&mut session).try_collect().await?;
            assert_eq!(rows.len() as u64, agent.conversation.last_seq);
            for (index, row) in rows.iter().enumerate() {
                assert_eq!(crate::store::agent_log::read_entry(row).unwrap().seq, index as u64 + 1);
            }
            let sequence = agent.conversation.last_seq + 1;
            let result = store.agents.update_one(doc! { "_id": id.as_str(),
                "last_seq": agent.conversation.last_seq as i64,
                "record.conversation.last_seq": agent.conversation.last_seq as i64 },
                doc! { "$set": { "last_seq": sequence as i64, "record.conversation.last_seq": sequence as i64 } })
                .session(&mut session).await?;
            assert_eq!(result.matched_count, 1);
            store.agent_log.insert_one(log_document(&AgentLogEntry {
                agent_id: id.clone(), seq: sequence, appended_at: NOW.into(),
                kind: AgentLogKind::WorkStepCompleted { step: AgentWorkStep::None }
            }).unwrap()).session(&mut session).await?;
            session.commit_transaction().await?;
            Ok::<_, mongodb::error::Error>(sequence)
        }.await;
        match result {
            Ok(sequence) => return sequence,
            Err(error) if is_transient(&error) => {
                let _ = session.abort_transaction().await;
            }
            Err(error) => panic!("legacy append: {error}"),
        }
    }
    panic!("legacy append exhausted contention retries")
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn mixed_writer_race_and_disable_reenable_keep_original_history() {
    let (client, name) = database("bounded_mixed_writers").await;
    let (legacy, agent, _) = fixture(&name, &client).await;
    let (store, agent) = prepare_history(legacy, agent, 1000).await;
    let kind = AgentLogKind::WorkStepCompleted {
        step: AgentWorkStep::None,
    };
    let (old, new) = tokio::join!(
        old_writer_append(&store, &agent.id),
        store.append_agent_log(&agent.id, NOW.into(), kind.clone())
    );
    let mut sequences = [old, new.unwrap().seq];
    sequences.sort();
    assert_eq!(sequences, [1001, 1002]);
    old_writer_append(&store, &agent.id).await;
    isolated(async {
        store
            .append_agent_log(&agent.id, NOW.into(), kind.clone())
            .await
            .unwrap();
        assert_eq!(snapshot(ScanCaller::Other, true)[0], 1);
        store
            .append_agent_log(&agent.id, NOW.into(), kind.clone())
            .await
            .unwrap();
        assert_eq!(snapshot(ScanCaller::Other, true)[0], 1);
    })
    .await;
    let disabled = MongoSandboxStore::open(&client, &name).await.unwrap();
    isolated(async {
        disabled
            .append_agent_log(&agent.id, NOW.into(), kind.clone())
            .await
            .unwrap();
        assert_eq!(snapshot(ScanCaller::Other, true)[0], 1);
    })
    .await;
    let prepared = disabled
        .prepare_bounded_admission(std::slice::from_ref(&agent.id))
        .await
        .unwrap();
    isolated(async {
        prepared
            .append_agent_log(&agent.id, NOW.into(), kind)
            .await
            .unwrap();
        assert_eq!(snapshot(ScanCaller::Other, true)[0], 0);
    })
    .await;
    let row = prepared
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert!(row.contains_key(INTEGRITY_FIELD));
    assert!(!row.contains_key(QUARANTINE_FIELD));
    assert_eq!(parse_integrity(&row).unwrap().unwrap().entry_count, 1007);
    // Unknown envelope fields remain invisible to the retained record decoder.
    let decoded: Agent = crate::store::sandbox_mongo::read_record(&row).unwrap();
    assert_eq!(decoded.conversation.last_seq, 1007);
    assert_eq!(decoded.creation_worktree_id(), agent.creation_worktree_id());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn initialization_rejects_mismatched_created_lineage_and_preserves_evidence() {
    let (client, name) = database("bounded_creation_lineage").await;
    let (legacy, agent, _) = fixture(&name, &client).await;
    let (store, agent) = prepare_history(legacy, agent, 1000).await;
    store
        .agent_log
        .update_one(
            doc! { "agent_id": agent.id.as_str(), "seq": 1_i64 },
            doc! { "$set": { "record.kind.worktree_id": "wtr_foreign" } },
        )
        .await
        .unwrap();
    assert!(store
        .clone()
        .prepare_bounded_admission(std::slice::from_ref(&agent.id))
        .await
        .is_err());
    let row = store
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert!(row.contains_key(QUARANTINE_FIELD));
    assert!(row.contains_key(INTEGRITY_FIELD));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn metadata_decision_table_separates_fallback_from_corruption() {
    for (case, repairable) in [
        ("missing", true),
        ("unsupported", true),
        ("stale", true),
        ("head_ahead", false),
        ("revision_ahead", false),
        ("negative_bytes", false),
        ("count_mismatch", false),
        ("missing_tail", false),
        ("extra_tail", false),
        ("physical_head", false),
    ] {
        let (client, name) = database("bounded_metadata_states").await;
        let (legacy, agent, _) = fixture(&name, &client).await;
        let (store, agent) = prepare_history(legacy, agent, 1000).await;
        let update = match case {
            "missing" => doc! { "$unset": { INTEGRITY_FIELD: "" } },
            "unsupported" => doc! { "$set": { "agent_log_integrity_v1.format": 2_i64 } },
            "stale" => doc! { "$inc": { "revision": 1_i64, "record.revision": 1_i64 } },
            "head_ahead" => doc! { "$set": { "agent_log_integrity_v1.head_seq": 1001_i64 } },
            "revision_ahead" => {
                doc! { "$set": { "agent_log_integrity_v1.bound_agent_revision": agent.revision as i64 + 1 } }
            }
            "negative_bytes" => doc! { "$set": { "agent_log_integrity_v1.encoded_bytes": -1_i64 } },
            "physical_head" => doc! { "$set": {
                "last_seq": 10_129_i64, "record.conversation.last_seq": 10_129_i64
            } },
            "count_mismatch" => doc! { "$set": { "agent_log_integrity_v1.entry_count": 999_i64 } },
            "missing_tail" => {
                store
                    .agent_log
                    .delete_one(doc! { "agent_id": agent.id.as_str(), "seq": 1000_i64 })
                    .await
                    .unwrap();
                doc! { "$inc": { "agent_command_admission_fence": 1_i64 } }
            }
            "extra_tail" => {
                store
                    .agent_log
                    .insert_one(
                        log_document(&AgentLogEntry {
                            agent_id: agent.id.clone(),
                            seq: 1001,
                            appended_at: NOW.into(),
                            kind: AgentLogKind::WorkStepCompleted {
                                step: AgentWorkStep::None,
                            },
                        })
                        .unwrap(),
                    )
                    .await
                    .unwrap();
                doc! { "$inc": { "agent_command_admission_fence": 1_i64 } }
            }
            _ => unreachable!(),
        };
        store
            .agents
            .update_one(doc! { "_id": agent.id.as_str() }, update)
            .await
            .unwrap();
        isolated(async {
            let result = store
                .append_agent_log(
                    &agent.id,
                    NOW.into(),
                    AgentLogKind::WorkStepCompleted {
                        step: AgentWorkStep::None,
                    },
                )
                .await;
            assert_eq!(result.is_ok(), repairable, "{case}: {result:?}");
            assert_eq!(snapshot(ScanCaller::Other, repairable)[0], 1, "{case}");
            assert_eq!(snapshot(ScanCaller::Other, !repairable)[0], 0, "{case}");
        })
        .await;
        let row = store
            .agents
            .find_one(doc! { "_id": agent.id.as_str() })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.contains_key(QUARANTINE_FIELD), !repairable, "{case}");
        assert_eq!(
            row.get_i64("last_seq").unwrap(),
            if repairable {
                1001
            } else if case == "physical_head" {
                10_129
            } else {
                1000
            }
        );
        client.database(&name).drop().await.unwrap();
    }
}
