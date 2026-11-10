use super::bounded_fixtures::prepare_history;
use super::*;
use crate::store::agent_log_integrity::{parse_integrity, INTEGRITY_FIELD, QUARANTINE_FIELD};
use crate::store::agent_log_telemetry::{isolated_test as isolated, snapshot, ScanCaller};
use crate::store::agents::agent_replacement_fields;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn semantic_audit_quarantines_middle_damage_across_revision_writes_and_restart() {
    for transition in [false, true] {
        let (client, name) = database("bounded_semantic_audit").await;
        let (legacy, agent, _) = fixture(&name, &client).await;
        let (store, mut agent) = prepare_history(legacy, agent, 1000).await;
        let change = if transition {
            doc! { "$set": { "record.kind": crate::store::sandbox_mongo::record_bson(
                &AgentLogKind::TurnCompleted { turn_sequence: 997 }
            ).unwrap() } }
        } else {
            let invalid = "x".repeat(NOW.len());
            doc! { "$set": { "record.appended_at": invalid } }
        };
        store
            .agent_log
            .update_one(
                doc! { "agent_id": agent.id.as_str(), "seq": 500_i64 },
                change,
            )
            .await
            .unwrap();
        // A normal revision-only writer carries the still-selectable checkpoint;
        // the audit must revoke it for the already running prepared store too.
        let revision = agent.revision;
        agent.revision += 1;
        store.save_agent(revision, agent.clone()).await.unwrap();
        let filter = doc! { "_id": agent.id.as_str() };
        let before = store
            .agents
            .find_one(filter.clone())
            .await
            .unwrap()
            .unwrap();
        assert!(parse_integrity(&before).unwrap().unwrap().semantic.valid);
        let mut snapshot_session = client.start_session().snapshot(true).await.unwrap();
        assert!(
            crate::store::agent_log_integrity::read::read_head(
                &store,
                &agent.id,
                &mut snapshot_session,
            )
            .await
            .unwrap()
            .unwrap()
            .trusted
        );
        assert!(
            crate::store::agent_log_integrity::scan_collection(
                ScanCaller::RecoveryProof,
                &store.agent_log,
                &agent.id,
                1000,
                &mut snapshot_session,
                true,
                |_| Ok(()),
            )
            .await
            .is_ok(),
            "the retained damage must pass structural validation"
        );
        assert!(store
            .clone()
            .prepare_bounded_admission(std::slice::from_ref(&agent.id))
            .await
            .is_err());
        let after = store
            .agents
            .find_one(filter.clone())
            .await
            .unwrap()
            .unwrap();
        assert!(after.contains_key(QUARANTINE_FIELD));
        assert_eq!(after.get(INTEGRITY_FIELD), before.get(INTEGRITY_FIELD));
        let restarted = MongoSandboxStore::open(&client, &name).await.unwrap();
        for retained in [&store, &restarted] {
            assert!(matches!(
                retained
                    .append_agent_log(
                        &agent.id,
                        NOW.into(),
                        AgentLogKind::WorkStepCompleted {
                            step: AgentWorkStep::None,
                        }
                    )
                    .await,
                Err(SandboxStoreError::Corrupt(_))
            ));
        }
        assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
        assert_eq!(
            store
                .agent_log
                .count_documents(doc! { "agent_id": filter.get_str("_id").unwrap() })
                .await
                .unwrap(),
            1000
        );
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn flag_off_semantic_admission_audit_also_revokes_the_shared_checkpoint() {
    let (client, name) = database("bounded_disabled_semantic_audit").await;
    let (legacy, agent, parent) = fixture(&name, &client).await;
    let (enabled, agent) = prepare_history(legacy, agent, 1000).await;
    enabled
        .agent_log
        .update_one(
            doc! { "agent_id": agent.id.as_str(), "seq": 500_i64 },
            doc! { "$set": { "record.appended_at": "x".repeat(NOW.len()) } },
        )
        .await
        .unwrap();
    let disabled = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut command = request(&agent, &parent, "semantic-audit");
    AnswerContext {
        turn_sequence: 997,
        answer_sequence: 999,
    }
    .bind(&mut command.envelope.command)
    .unwrap();
    command.envelope.request_fingerprint = command.envelope.command.fingerprint().unwrap();
    assert!(disabled.reserve_command(command).await.is_err());
    assert!(enabled
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .unwrap()
        .unwrap()
        .contains_key(QUARANTINE_FIELD));
    assert!(enabled
        .append_agent_log(
            &agent.id,
            NOW.into(),
            AgentLogKind::WorkStepCompleted {
                step: AgentWorkStep::None,
            }
        )
        .await
        .is_err());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn incomplete_legacy_turn_stays_eligible_for_full_fallback_without_quarantine() {
    for invalid_checkpoint in [false, true] {
        let (client, name) = database("bounded_incomplete_semantics").await;
        let (legacy, agent, _) = fixture(&name, &client).await;
        let (mut store, mut agent) = prepare_history(legacy, agent, 20).await;
        store.bounded_admission_agents.clear();
        let started = AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: 21,
            appended_at: NOW.into(),
            kind: AgentLogKind::TurnStarted {
                turn_sequence: 21,
                origin_key: None,
                model: None,
            },
        };
        store
            .agent_log
            .insert_one(log_document(&started).unwrap())
            .await
            .unwrap();
        agent.conversation.last_seq = 21;
        agent.conversation.active_turn = Some(21);
        agent.status = AgentStatus::Working;
        agent.revision += 1;
        let mut fields = agent_replacement_fields(&agent).unwrap();
        fields.insert("last_seq", 21_i64);
        store
            .agents
            .update_one(
                doc! { "_id": agent.id.as_str() },
                doc! { "$set": fields, "$unset": { INTEGRITY_FIELD: "" } },
            )
            .await
            .unwrap();
        if invalid_checkpoint {
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
            let row = store
                .agents
                .find_one(doc! { "_id": agent.id.as_str() })
                .await
                .unwrap()
                .unwrap();
            assert!(!parse_integrity(&row).unwrap().unwrap().semantic.valid);
        }
        assert!(store
            .clone()
            .prepare_bounded_admission(std::slice::from_ref(&agent.id))
            .await
            .is_err());
        isolated(async {
            store
                .append_agent_log(
                    &agent.id,
                    NOW.into(),
                    AgentLogKind::OwnerMessage {
                        content: "pending input".into(),
                    },
                )
                .await
                .unwrap();
            assert_eq!(snapshot(ScanCaller::Other, true)[0], 1);
        })
        .await;
        // A later full fold sees the now-complete boundary and can re-establish
        // semantics without clearing a quarantine marker or rewriting history.
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
        let prepared = store
            .prepare_bounded_admission(std::slice::from_ref(&agent.id))
            .await
            .unwrap();
        let row = prepared
            .agents
            .find_one(doc! { "_id": agent.id.as_str() })
            .await
            .unwrap()
            .unwrap();
        assert!(!row.contains_key(QUARANTINE_FIELD));
        assert!(parse_integrity(&row).unwrap().unwrap().semantic.valid);
        client.database(&name).drop().await.unwrap();
    }
}
