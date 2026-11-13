use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn terminal_head_faults_fall_back_to_the_same_full_audit_result() {
    for fault in [
        "missing",
        "extra",
        "identity",
        "quarantine",
        "empty",
        "empty_extra",
    ] {
        let mut baseline = None;
        for enabled in [false, true] {
            let (client, name, store) = database().await;
            let store = store.with_runtime_loss_bounded_proof(enabled);
            let sandbox = active_sandbox("sbx_fault");
            store.create_sandbox(sandbox.clone()).await.unwrap();
            let mut agent = history(&store, &sandbox, "agt_fault", 10, AgentStatus::Stopped).await;
            match fault {
                "missing" => {
                    store
                        .agent_log
                        .delete_one(doc! { "_id": log_id(&agent.id, 10) })
                        .await
                        .unwrap();
                }
                "extra" => {
                    let entry = AgentLogEntry {
                        agent_id: agent.id.clone(),
                        seq: 11,
                        appended_at: BEFORE.into(),
                        kind: AgentLogKind::AgentMessage {
                            content: "extra".into(),
                        },
                    };
                    store
                        .agent_log
                        .insert_one(log_document(&entry).unwrap())
                        .await
                        .unwrap();
                }
                "identity" => {
                    store
                        .agent_log
                        .update_one(
                            doc! { "_id": log_id(&agent.id, 10) },
                            doc! { "$set": { "record.seq": 9_i64 } },
                        )
                        .await
                        .unwrap();
                }
                "quarantine" => {
                    store.agents.update_one(doc! { "_id": agent.id.as_str() }, doc! { "$set": { QUARANTINE_FIELD: { "reason": "integrity_validation_failed" } } }).await.unwrap();
                }
                "empty" | "empty_extra" => {
                    agent.conversation.last_seq = 0;
                    let mut fields = agent_replacement_fields(&agent).unwrap();
                    fields.insert("last_seq", 0_i64);
                    store
                        .agents
                        .update_one(doc! { "_id": agent.id.as_str() }, doc! { "$set": fields })
                        .await
                        .unwrap();
                    if fault == "empty" {
                        store
                            .agent_log
                            .delete_many(doc! { "agent_id": agent.id.as_str() })
                            .await
                            .unwrap();
                    }
                }
                _ => unreachable!(),
            }
            let result = agent_log_telemetry::isolated_test(async {
                let result = store.recover_runtime_loss(loss(&sandbox)).await;
                let scans = agent_log_telemetry::snapshot_all()[0];
                assert_eq!(
                    scans,
                    u64::from(fault != "quarantine" && (!enabled || fault != "empty")),
                    "{fault}, enabled={enabled}"
                );
                result
            })
            .await;
            if let Some(expected) = &baseline {
                assert_eq!(&result, expected, "{fault}");
            } else {
                baseline = Some(result);
            }
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn live_runtime_loss_summaries_require_fresh_semantics_and_cohort_membership() {
    for state in ["trusted", "stale", "untrusted", "outside_cohort"] {
        let (client, name, store) = database().await;
        let sandbox = active_sandbox("sbx_summary");
        store.create_sandbox(sandbox.clone()).await.unwrap();
        let agent = history(&store, &sandbox, "agt_summary", 100, AgentStatus::Ready).await;
        let mut store = store
            .prepare_bounded_admission(std::slice::from_ref(&agent.id))
            .await
            .unwrap()
            .with_runtime_loss_bounded_proof(true);
        let path = match state {
            "stale" => Some((
                format!("{INTEGRITY_FIELD}.bound_agent_revision"),
                mongodb::bson::Bson::Int64(0),
            )),
            "untrusted" => Some((
                format!("{INTEGRITY_FIELD}.semantic_checkpoint_v1.valid"),
                mongodb::bson::Bson::Boolean(false),
            )),
            "outside_cohort" => {
                store.bounded_admission_agents.clear();
                None
            }
            _ => None,
        };
        if let Some((path, value)) = path {
            store
                .agents
                .update_one(
                    doc! { "_id": agent.id.as_str() },
                    doc! { "$set": { path: value } },
                )
                .await
                .unwrap();
        }
        agent_log_telemetry::isolated_test(async {
            assert_eq!(
                store.recover_runtime_loss(loss(&sandbox)).await.unwrap(),
                SandboxRuntimeLossOutcome::Failed
            );
            assert_eq!(
                agent_log_telemetry::snapshot(ScanCaller::RuntimeReconciliation, true)[0],
                u64::from(state != "trusted"),
                "{state}"
            );
        })
        .await;
        client.database(&name).drop().await.unwrap();
    }
}
