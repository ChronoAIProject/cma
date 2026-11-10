use super::bounded_admission_tests::prepare_history;
use super::*;
use crate::store::agent_log_integrity::{
    read::{read_head, ReadHead},
    INTEGRITY_FIELD, QUARANTINE_FIELD,
};
use crate::store::agent_log_telemetry::{isolated_test as isolated, snapshot, ScanCaller};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn public_proof_retains_one_snapshot_when_a_writer_advances() {
    let (client, name) = database("bounded_read_snapshot").await;
    let (legacy, agent, _) = fixture(&name, &client).await;
    let (store, agent) = prepare_history(legacy, agent, 20).await;
    let mut session = client.start_session().snapshot(true).await.unwrap();
    let before = read_head(&store, &agent.id, &mut session).await.unwrap();
    assert_eq!(
        before,
        Some(ReadHead {
            last_seq: 20,
            trusted: true
        })
    );
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
    assert_eq!(
        read_head(&store, &agent.id, &mut session).await.unwrap(),
        before
    );
    let mut fresh = client.start_session().snapshot(true).await.unwrap();
    assert_eq!(
        read_head(&store, &agent.id, &mut fresh).await.unwrap(),
        Some(ReadHead {
            last_seq: 21,
            trusted: true
        })
    );
    assert_eq!(
        read_head(&store, &AgentId::generate(), &mut fresh)
            .await
            .unwrap(),
        None
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set with profiler access"]
async fn public_proof_shares_trust_rules_without_writes_or_audits() {
    for case in [
        "trusted",
        "disabled",
        "missing",
        "unsupported",
        "stale",
        "head_ahead",
        "revision_ahead",
        "head_mismatch",
        "identity",
        "bytes",
        "count",
        "quarantine",
        "missing_tail",
        "tail_hash",
        "extra_tail",
    ] {
        let (client, name) = database("bounded_read_rules").await;
        let (legacy, agent, _) = fixture(&name, &client).await;
        let (mut store, agent) = prepare_history(legacy, agent, 20).await;
        let filter = doc! { "_id": agent.id.as_str() };
        let update = match case {
            "disabled" => {
                store = MongoSandboxStore::open(&client, &name).await.unwrap();
                None
            }
            "trusted" => None,
            "missing" => Some(doc! { "$unset": { INTEGRITY_FIELD: "" } }),
            "unsupported" => Some(doc! { "$set": { "agent_log_integrity_v1.format": 2_i64 } }),
            "stale" => Some(doc! { "$inc": { "revision": 1_i64, "record.revision": 1_i64 } }),
            "head_ahead" => Some(doc! { "$set": { "agent_log_integrity_v1.head_seq": 21_i64 } }),
            "revision_ahead" => Some(
                doc! { "$set": { "agent_log_integrity_v1.bound_agent_revision": agent.revision as i64 + 1 } },
            ),
            "head_mismatch" => Some(doc! { "$set": { "last_seq": 21_i64 } }),
            "identity" => Some(
                doc! { "$set": { "agent_log_integrity_v1.history_identity": Bson::Binary(mongodb::bson::Binary {
                    subtype: mongodb::bson::spec::BinarySubtype::Generic, bytes: vec![0; 32],
                }) } },
            ),
            "bytes" => {
                Some(doc! { "$set": { "agent_log_integrity_v1.encoded_bytes": 70_000_000_i64 } })
            }
            "count" => Some(doc! { "$set": { "agent_log_integrity_v1.entry_count": 19_i64 } }),
            "quarantine" => Some(doc! { "$set": { QUARANTINE_FIELD: {} } }),
            "missing_tail" => {
                store
                    .agent_log
                    .delete_one(doc! { "agent_id": agent.id.as_str(), "seq": 20_i64 })
                    .await
                    .unwrap();
                None
            }
            "tail_hash" => {
                store
                    .agent_log
                    .update_one(
                        doc! { "agent_id": agent.id.as_str(), "seq": 20_i64 },
                        doc! { "$set": { "record.appended_at": TWO_MINUTES } },
                    )
                    .await
                    .unwrap();
                None
            }
            "extra_tail" => {
                store
                    .agent_log
                    .insert_one(
                        log_document(&AgentLogEntry {
                            agent_id: agent.id.clone(),
                            seq: 21,
                            appended_at: NOW.into(),
                            kind: AgentLogKind::WorkStepCompleted {
                                step: AgentWorkStep::None,
                            },
                        })
                        .unwrap(),
                    )
                    .await
                    .unwrap();
                None
            }
            _ => unreachable!(),
        };
        if let Some(update) = update {
            store
                .agents
                .update_one(filter.clone(), update)
                .await
                .unwrap();
        }
        let before = store
            .agents
            .find_one(filter.clone())
            .await
            .unwrap()
            .unwrap();
        let db = client.database(&name);
        db.run_command(doc! { "profile": 2, "slowms": 0 })
            .await
            .unwrap();
        let mut session = client.start_session().snapshot(true).await.unwrap();
        isolated(async {
            let result = read_head(&store, &agent.id, &mut session).await;
            if matches!(
                case,
                "trusted" | "disabled" | "missing" | "unsupported" | "stale"
            ) {
                assert_eq!(
                    result.unwrap(),
                    Some(ReadHead {
                        last_seq: 20,
                        trusted: case == "trusted"
                    }),
                    "{case}"
                );
            } else {
                assert!(
                    matches!(result, Err(SandboxStoreError::Corrupt(_))),
                    "{case}: {result:?}"
                );
            }
            assert_eq!(snapshot(ScanCaller::PublicWindowPrefix, true)[0], 0);
            assert_eq!(snapshot(ScanCaller::PublicWindowPrefix, false)[0], 0);
        })
        .await;
        db.run_command(doc! { "profile": 0 }).await.unwrap();
        let operations: Vec<Document> = db
            .collection("system.profile")
            .find(doc! {})
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        for operation in operations {
            assert!(
                !matches!(
                    operation.get_str("op").unwrap_or(""),
                    "update" | "insert" | "remove"
                ),
                "{case}: {operation:?}"
            );
            if let Ok(command) = operation.get_document("command") {
                assert!(
                    ![
                        "abortTransaction",
                        "commitTransaction",
                        "startTransaction",
                        "update",
                        "insert",
                        "delete"
                    ]
                    .iter()
                    .any(|key| command.contains_key(key)),
                    "{case}: {command:?}"
                );
            }
        }
        assert_eq!(
            store.agents.find_one(filter).await.unwrap().unwrap(),
            before,
            "{case}"
        );
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn public_proof_does_not_treat_legacy_zero_head_orphans_as_empty() {
    let (client, name) = database("bounded_read_zero").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut agent = ready_agent(&active_sandbox("sbx_zero_read"), "agt_zero_read");
    agent.conversation.last_seq = 0;
    agent.conversation.conversation_id = None;
    agent.conversation.runtime_generation = None;
    store.create_agent(agent.clone()).await.unwrap();
    store
        .agents
        .update_one(
            doc! { "_id": agent.id.as_str() },
            doc! { "$unset": { INTEGRITY_FIELD: "" } },
        )
        .await
        .unwrap();
    let mut session = client.start_session().snapshot(true).await.unwrap();
    assert_eq!(
        read_head(&store, &agent.id, &mut session).await.unwrap(),
        Some(ReadHead {
            last_seq: 0,
            trusted: false
        })
    );
    store
        .agent_log
        .insert_one(doc! { "_id": "orphan", "agent_id": agent.id.as_str(), "seq": "malformed" })
        .bypass_document_validation(true)
        .await
        .unwrap();
    let mut fresh = client.start_session().snapshot(true).await.unwrap();
    assert!(matches!(
        read_head(&store, &agent.id, &mut fresh).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    assert!(!store
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .unwrap()
        .unwrap()
        .contains_key(QUARANTINE_FIELD));
    client.database(&name).drop().await.unwrap();
}
