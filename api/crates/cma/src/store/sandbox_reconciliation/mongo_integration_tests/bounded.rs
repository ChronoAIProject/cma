//! Compare persisted bytes and consumed documents, not elapsed-time guesses.
use super::*;
use crate::store::{
    agent_log::{log_document, log_id},
    agent_log_integrity::{INTEGRITY_FIELD, QUARANTINE_FIELD},
    agent_log_telemetry::{self, ScanCaller},
    mongo_read_telemetry,
};
use layer2_core::sandbox::AgentLogEntry;
use mongodb::bson::Document;

mod faults;

async fn history(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
    id: &str,
    count: u64,
    status: AgentStatus,
) -> Agent {
    let mut agent = ready_agent(sandbox, id);
    agent.conversation.last_seq = count;
    if status != AgentStatus::Ready {
        agent
            .transition(
                AgentStatus::Stopped,
                BEFORE.into(),
                Some(AgentStopReason::Owner),
            )
            .unwrap();
        if status == AgentStatus::Deleted {
            agent
                .transition(AgentStatus::Deleted, BEFORE.into(), None)
                .unwrap();
        }
    }
    let mut entries = vec![
        AgentLogKind::Created {
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.creation_worktree_id().clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: agent.conversation.conversation_id.clone().unwrap(),
            runtime_generation: 1,
        },
        AgentLogKind::TurnStarted {
            turn_sequence: 3,
            origin_key: None,
            model: None,
        },
        AgentLogKind::OwnerMessage {
            content: "retained question".into(),
        },
    ];
    entries.resize(
        count as usize - 1,
        AgentLogKind::AgentMessage {
            content: "retained output".into(),
        },
    );
    entries.push(AgentLogKind::TurnCompleted { turn_sequence: 3 });
    store
        .agents
        .insert_one(crate::store::agents::agent_document(&agent).unwrap())
        .await
        .unwrap();
    for (batch, chunk) in entries.chunks(200).enumerate() {
        let documents = chunk.iter().enumerate().map(|(offset, kind)| {
            log_document(&AgentLogEntry {
                agent_id: agent.id.clone(),
                seq: (batch * 200 + offset + 1) as u64,
                appended_at: BEFORE.into(),
                kind: kind.clone(),
            })
            .unwrap()
        });
        store.agent_log.insert_many(documents).await.unwrap();
    }
    agent
}

async fn persisted(store: &MongoSandboxStore) -> Vec<Vec<u8>> {
    let mut result = Vec::new();
    for collection in [
        &store.sandboxes,
        &store.agents,
        &store.agent_log,
        &store.sandbox_events,
    ] {
        let rows: Vec<Document> = collection
            .find(doc! {})
            .sort(doc! { "_id": 1 })
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        result.extend(rows.iter().map(|row| {
            mongodb::bson::RawDocumentBuf::try_from(row)
                .unwrap()
                .into_bytes()
        }));
    }
    result
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn large_runtime_loss_logs_are_bounded_and_publish_identical_bytes() {
    for count in [100, 10_001] {
        let mut baseline = None;
        for enabled in [false, true] {
            let (client, name, store) = database().await;
            let sandbox = active_sandbox("sbx_bounded");
            store.create_sandbox(sandbox.clone()).await.unwrap();
            let trusted = history(&store, &sandbox, "agt_trusted", count, AgentStatus::Ready).await;
            history(&store, &sandbox, "agt_legacy", count, AgentStatus::Ready).await;
            history(&store, &sandbox, "agt_stopped", count, AgentStatus::Stopped).await;
            history(&store, &sandbox, "agt_deleted", count, AgentStatus::Deleted).await;
            let store = store
                .prepare_bounded_admission(&[trusted.id])
                .await
                .unwrap()
                .with_runtime_loss_bounded_proof(enabled);
            let started = std::time::Instant::now();
            let (scans, reads) =
                agent_log_telemetry::isolated_test(mongo_read_telemetry::isolated_test(async {
                    assert_eq!(
                        store.recover_runtime_loss(loss(&sandbox)).await.unwrap(),
                        SandboxRuntimeLossOutcome::Failed
                    );
                    (
                        agent_log_telemetry::snapshot(ScanCaller::RuntimeReconciliation, true),
                        mongo_read_telemetry::snapshot("agent_log"),
                    )
                }))
                .await;
            assert_eq!(scans[0], if enabled { 1 } else { 4 });
            assert_eq!(scans[1], count * if enabled { 1 } else { 4 });
            // Three bounded proofs consume only their head document, regardless
            // of history size. The untrusted live sibling retains its full audit.
            assert_eq!(reads[2], if enabled { count + 3 } else { count * 4 + 1 });
            eprintln!("runtime_loss enabled={enabled} history={count} full_scans={} documents={} elapsed_ms={}", scans[0], reads[2], started.elapsed().as_millis());
            let result = persisted(&store).await;
            if let Some(expected) = &baseline {
                assert_eq!(&result, expected);
            } else {
                baseline = Some(result);
            }
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn stale_runtime_loss_observations_read_zero_log_documents() {
    let (client, name, store) = database().await;
    let store = store.with_runtime_loss_bounded_proof(true);
    for case in ["revision", "sandbox_updated", "agent_updated"] {
        let (mut sandbox, mut agent) = checkpoint_fixture(&store, &format!("sbx_{case}")).await;
        let mut request = loss(&sandbox);
        match case {
            "revision" => request.expected_revision += 1,
            "sandbox_updated" => {
                sandbox.updated_at = "2099-01-01T00:00:00Z".into();
                store
                    .sandboxes
                    .update_one(
                        doc! { "_id": sandbox.id.as_str() },
                        doc! { "$set": sandbox_replacement_fields(&sandbox).unwrap() },
                    )
                    .await
                    .unwrap();
            }
            "agent_updated" => {
                agent.updated_at = "2099-01-01T00:00:00Z".into();
                store
                    .agents
                    .update_one(
                        doc! { "_id": agent.id.as_str() },
                        doc! { "$set": agent_replacement_fields(&agent).unwrap() },
                    )
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        agent_log_telemetry::isolated_test(mongo_read_telemetry::isolated_test(async {
            assert_eq!(
                store.recover_runtime_loss(request).await.unwrap(),
                if case == "revision" {
                    SandboxRuntimeLossOutcome::Superseded
                } else {
                    SandboxRuntimeLossOutcome::NotEligible
                }
            );
            assert_eq!(agent_log_telemetry::snapshot_all()[0], 0);
            assert_eq!(mongo_read_telemetry::snapshot("agent_log"), [0; 4]);
        }))
        .await;
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn fully_trusted_large_recovery_reads_only_four_heads() {
    let (client, name, store) = database().await;
    let sandbox = active_sandbox("sbx_all_bounded");
    store.create_sandbox(sandbox.clone()).await.unwrap();
    let mut live = Vec::new();
    for (id, status) in [
        ("agt_a", AgentStatus::Ready),
        ("agt_b", AgentStatus::Ready),
        ("agt_c", AgentStatus::Stopped),
        ("agt_d", AgentStatus::Deleted),
    ] {
        let agent = history(&store, &sandbox, id, 10_001, status).await;
        if agent.status.is_live() {
            live.push(agent.id);
        }
    }
    let store = store
        .prepare_bounded_admission(&live)
        .await
        .unwrap()
        .with_runtime_loss_bounded_proof(true);
    agent_log_telemetry::isolated_test(mongo_read_telemetry::isolated_test(async {
        let started = std::time::Instant::now();
        assert_eq!(
            store.recover_runtime_loss(loss(&sandbox)).await.unwrap(),
            SandboxRuntimeLossOutcome::Failed
        );
        assert_eq!(agent_log_telemetry::snapshot_all()[0], 0);
        assert_eq!(mongo_read_telemetry::snapshot("agent_log")[2], 4);
        eprintln!(
            "runtime_loss all_trusted=true history=10001 full_scans=0 documents=4 elapsed_ms={}",
            started.elapsed().as_millis()
        );
    }))
    .await;
    client.database(&name).drop().await.unwrap();
}
