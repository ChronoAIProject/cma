use super::*;
use crate::store::{agent_commands::records, agent_log::log_document, sandbox_mongo::record_bson};
use layer2_core::sandbox::agent_io::*;
use layer2_core::sandbox::store::AgentLogStore;

fn command(agent: &Agent) -> AdmittedAgentCommand {
    let input = AgentCommand::message("Retained test input");
    AdmittedAgentCommand {
        queued_input: None,
        receipt: AdmissionReceipt {
            envelope: AgentCommandEnvelope {
                continuation: None,
                schema_version: AGENT_IO_SCHEMA_VERSION,
                command_id: layer2_core::ids::AgentCommandId::generate(),
                agent_id: agent.id.clone(),
                owner: AgentOwner::Person {
                    subject: agent.owner.clone(),
                },
                actor: AgentActor::Human {
                    subject: agent.owner.clone(),
                },
                source: AgentCommandSource {
                    kind: AgentSourceKind::UserInterface,
                    reference: "retained-input".into(),
                },
                correlation_id: "retained-correlation".into(),
                request_fingerprint: input.fingerprint().unwrap(),
                authorization: AgentAuthorizationRevision {
                    authority_id: "personal-owner".into(),
                    policy_revision: 1,
                    grant_revision: None,
                },
                command: input,
            },
            response_id: Some(layer2_core::ids::AgentResponseId::generate()),
            admitted_at: NOW.into(),
            agent_order: 1,
            event_cursor: agent.conversation.last_seq,
        },
        key_sha256: "f".repeat(64),
        queue_deadline: "2026-09-05T00:32:00Z".into(),
        status: AgentResponseStatus::Failed,
        revision: 1,
        target_turn: None,
        work: CommandWorkState {
            failure: Some(CommandFailure {
                code: CommandFailureCode::AuthorityRevoked,
                at: NOW.into(),
            }),
            ..Default::default()
        },
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn retained_reopen_mongo_complete_commands_are_required_and_live_lanes_refuse() {
    let (client, name, store) = database().await;
    for case in ["terminal", "queued", "lease", "missing", "changed"] {
        let request = reopen_fixture(&store, &format!("sbx_commands_{case}")).await;
        let id = &request.snapshot.retained.sandbox.id;
        let agent = &request.snapshot.retained.agents[0];
        let mut record = command(agent);
        if case == "queued" {
            record.status = AgentResponseStatus::Queued;
            record.work.failure = None;
        }
        if case != "missing" {
            store
                .agent_commands
                .insert_one(records::document(&record).unwrap())
                .await
                .unwrap();
        }
        let lease = (case == "lease").then(|| CommandLease {
            claim: CommandClaim {
                agent_id: agent.id.clone(),
                command_id: record.receipt.envelope.command_id.clone(),
                holder: "other-worker".into(),
                fence: 1,
            },
            expires_at: "2026-09-05T00:31:00Z".into(),
        });
        store.agent_command_orders.insert_one(doc! {"_id":agent.id.as_str(), "document_schema_version":crate::store::sandbox_mongo::DOCUMENT_SCHEMA_VERSION,
            "last_order":1_i64, "next_fence":2_i64, "lease":record_bson(&lease).unwrap()}).await.unwrap();
        if case == "terminal" {
            let mut fresh = request.clone();
            fresh.snapshot = store.retained_reopen_snapshot(id).await.unwrap();
            store.admit_retained_reopen(fresh).await.unwrap();
            let row = store
                .agent_command_orders
                .find_one(doc! {"_id":agent.id.as_str()})
                .await
                .unwrap()
                .unwrap();
            assert_eq!(row.get_i64("sandbox_recovery_fence").unwrap(), 1);
            assert_eq!(
                records::read(
                    &store
                        .agent_commands
                        .find_one(doc! {"_id":record.receipt.envelope.command_id.as_str()})
                        .await
                        .unwrap()
                        .unwrap()
                )
                .unwrap(),
                record
            );
        } else {
            // "changed" is a new, fully terminal inventory after observation;
            // the original snapshot still cannot authorize this newer state.
            assert!(
                store.admit_retained_reopen(request.clone()).await.is_err(),
                "{case}"
            );
            assert!(store.retained_reopen_receipt(id).await.unwrap().is_none());
            assert!(store.sandbox_events(id, 0, 100).await.unwrap().is_empty());
        }
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn retained_reopen_mongo_semantic_corruption_revokes_trust_and_never_admits() {
    let (client, name, store) = database().await;
    let request = reopen_fixture(&store, "sbx_reopen_corruption").await;
    let id = &request.snapshot.retained.sandbox.id;
    let agent = &request.snapshot.retained.agents[0];
    let mut tail = store
        .agent_log(&agent.id, 0, 10)
        .await
        .unwrap()
        .pop()
        .unwrap();
    let store = store
        .prepare_bounded_admission(std::slice::from_ref(&agent.id))
        .await
        .unwrap();
    tail.kind = AgentLogKind::StatusChanged {
        from: AgentStatus::Ready,
        to: AgentStatus::Working,
    };
    store
        .agent_log
        .replace_one(
            doc! {"agent_id":agent.id.as_str(),"seq":3_i64},
            log_document(&tail).unwrap(),
        )
        .await
        .unwrap();
    assert!(store.admit_retained_reopen(request.clone()).await.is_err());
    assert!(store.retained_reopen_receipt(id).await.unwrap().is_none());
    let row = store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert!(row.contains_key("agent_log_integrity_quarantine_v1"));
    assert!(store.sandbox_events(id, 0, 100).await.unwrap().is_empty());
    client.database(&name).drop().await.unwrap();
}
