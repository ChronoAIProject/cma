//! Public runtime-loss root after real gated adoption, with legacy siblings.
use super::*;
use crate::store::agent_log_segments::{self as segments, publication, writer_gate as gate};
use crate::store::sandbox_mongo::{commit_transaction, CommitOutcome};
use layer2_core::sandbox::{store::AgentLogPublicationIdentity, AgentLogEntry};
use mongodb::bson::Document;

mod fences;
mod receipt;
mod uncertainty;

async fn adopted(store: &MongoSandboxStore, id: &str) -> (Sandbox, Agent) {
    let db = store.client.database(&store.agents.namespace().db);
    segments::schema::migrate(&db).await.unwrap();
    crate::store::migrations::agent_log_source_keys::prepare_for_test(&db).await;
    let mut sandbox = active_sandbox(id);
    let mut agent = ready_agent(&sandbox, &format!("agt_{id}"));
    agent.conversation.last_seq = 2;
    sandbox.lifecycle.checkpoints.latest = Some(checkpoint(std::slice::from_ref(&agent)));
    store.create_sandbox(sandbox.clone()).await.unwrap();
    store
        .agents
        .insert_one(crate::store::agents::agent_document(&agent).unwrap())
        .await
        .unwrap();
    // Complete legacy source: adoption must validate Created and native identity.
    for (index, kind) in [
        AgentLogKind::Created {
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.worktree_id.clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: agent.conversation.conversation_id.clone().unwrap(),
            runtime_generation: 1,
        },
    ]
    .into_iter()
    .enumerate()
    {
        store
            .agent_log
            .insert_one(
                crate::store::agent_log::log_document(&AgentLogEntry {
                    agent_id: agent.id.clone(),
                    seq: index as u64 + 1,
                    appended_at: BEFORE.into(),
                    kind,
                })
                .unwrap(),
            )
            .await
            .unwrap();
    }
    let agent = admit(store, &agent, 0).await;
    (sandbox, agent)
}

pub(super) async fn admit(store: &MongoSandboxStore, agent: &Agent, revision: i64) -> Agent {
    let db = store.client.database(&store.agents.namespace().db);
    let mut session = store.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    gate::replace(
        &db,
        gate::GateName::Segments,
        revision,
        gate::Policy {
            enabled: true,
            agents: vec![agent.id.to_string()],
            ..Default::default()
        },
        &mut session,
    )
    .await
    .unwrap();
    assert!(matches!(
        commit_transaction(&mut session).await,
        CommitOutcome::Committed
    ));
    let kind = AgentLogKind::ConversationGap { frames: 1 };
    store
        .append_agent_log_keyed(
            &agent.id,
            &AgentLogPublicationIdentity {
                key: format!("adopt-{}", agent.id),
                appended_at: BEFORE.into(),
                fingerprint: publication::request_digest(&kind)
                    .unwrap()
                    .bytes
                    .try_into()
                    .unwrap(),
            },
            kind,
        )
        .await
        .unwrap();
    assert_eq!(
        row(store, agent)
            .await
            .get_i64("document_schema_version")
            .unwrap(),
        2
    );
    store.get_agent(&agent.id).await.unwrap().unwrap()
}

async fn row(store: &MongoSandboxStore, agent: &Agent) -> Document {
    store
        .agents
        .find_one(doc! {"_id": agent.id.as_str()})
        .await
        .unwrap()
        .unwrap()
}

fn collection(store: &MongoSandboxStore, name: &str) -> mongodb::Collection<Document> {
    store
        .client
        .database(&store.agents.namespace().db)
        .collection(name)
}

async fn publication_count(store: &MongoSandboxStore) -> u64 {
    collection(store, segments::PUBLICATIONS)
        .count_documents(doc! {})
        .await
        .unwrap()
}

async fn unchanged(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
    agent: &Agent,
    original: &Document,
) {
    assert_eq!(
        store.get_sandbox(&sandbox.id).await.unwrap().as_ref(),
        Some(sandbox)
    );
    assert_eq!(&row(store, agent).await, original);
    assert_eq!(
        store
            .agent_log
            .count_documents(doc! {"agent_id":agent.id.as_str()})
            .await
            .unwrap(),
        agent.conversation.last_seq
    );
    assert_eq!(publication_count(store).await, 1); // adoption only
                                                   // Raw absence remains checkable when the test deliberately corrupts the
                                                   // event head; the public reader must refuse that incomplete stream.
    assert_eq!(
        store
            .sandbox_events
            .count_documents(doc! {"sandbox_id":sandbox.id.as_str()})
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn root_recovers_adopted_agent_and_preserves_legacy_sibling_format() {
    for enabled in [false, true] {
        let (client, name, store) = database().await;
        let store = store.with_runtime_loss_bounded_proof(enabled);
        let (sandbox, agent) = adopted(&store, "sbx_mixed").await;
        let sibling = ready_agent(&sandbox, "agt_legacy_sibling");
        insert_agent(&store, &sibling, BEFORE).await;
        let old_logs = store.agent_log(&agent.id, 0, 100).await.unwrap();
        assert_eq!(
            store.recover_runtime_loss(loss(&sandbox)).await.unwrap(),
            SandboxRuntimeLossOutcome::RecoveryScheduled
        );
        let next = store.get_agent(&agent.id).await.unwrap().unwrap();
        assert_eq!(next.status, AgentStatus::Paused);
        assert_eq!(next.revision, agent.revision + 1);
        assert_eq!(
            store.get_agent(&sibling.id).await.unwrap().unwrap().status,
            AgentStatus::Stopped
        );
        let logs = store.agent_log(&agent.id, 0, 100).await.unwrap();
        assert_eq!(&logs[..old_logs.len()], old_logs);
        assert_eq!(
            row(&store, &agent)
                .await
                .get_i64("document_schema_version")
                .unwrap(),
            2
        );
        assert_eq!(
            row(&store, &sibling)
                .await
                .get_i64("document_schema_version")
                .unwrap(),
            1
        );
        assert_eq!(publication_count(&store).await, 2);
        assert_eq!(
            store.recover_runtime_loss(loss(&sandbox)).await.unwrap(),
            SandboxRuntimeLossOutcome::Superseded
        );
        assert_eq!(store.agent_log(&agent.id, 0, 100).await.unwrap(), logs);
        assert_eq!(publication_count(&store).await, 2);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn root_refuses_corrupt_segmented_evidence_and_rolls_back_late_domain_failure() {
    for enabled in [false, true] {
        for corruption in ["incarnation", "late_event", "legacy_sibling"] {
            let (client, name, store) = database().await;
            let store = store.with_runtime_loss_bounded_proof(enabled);
            let (sandbox, agent) = adopted(&store, &format!("sbx_{corruption}")).await;
            match corruption {
                "incarnation" => {
                    store
                        .agents
                        .update_one(
                            doc! {"_id":agent.id.as_str()},
                            doc! {"$set":{"agent_log_incarnation":"foreign"}},
                        )
                        .await
                        .unwrap();
                }
                "late_event" => {
                    store
                        .sandboxes
                        .update_one(
                            doc! {"_id":sandbox.id.as_str()},
                            doc! {"$set":{"last_seq":1_i64}},
                        )
                        .await
                        .unwrap();
                }
                "legacy_sibling" => {
                    let sibling = ready_agent(&sandbox, "agt_invalid_legacy");
                    insert_agent(&store, &sibling, BEFORE).await;
                    store
                        .agent_log
                        .delete_many(doc! {"agent_id":sibling.id.as_str()})
                        .await
                        .unwrap();
                }
                _ => unreachable!(),
            }
            let original = row(&store, &agent).await;
            let head = collection(&store, segments::HEADS)
                .find_one(doc! {})
                .await
                .unwrap();
            let projection = collection(&store, segments::PROJECTIONS)
                .find_one(doc! {})
                .await
                .unwrap();
            assert!(store.recover_runtime_loss(loss(&sandbox)).await.is_err());
            unchanged(&store, &sandbox, &agent, &original).await;
            assert_eq!(
                collection(&store, segments::HEADS)
                    .find_one(doc! {})
                    .await
                    .unwrap(),
                head
            );
            assert_eq!(
                collection(&store, segments::PROJECTIONS)
                    .find_one(doc! {})
                    .await
                    .unwrap(),
                projection
            );
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn root_refuses_child_count_overflow_before_capture_or_publication() {
    let (client, name, store) = database().await;
    let (sandbox, agent) = adopted(&store, "sbx_child_bound").await;
    let original = row(&store, &agent).await;
    let children = (0..512)
        .map(|index| {
            crate::store::agents::agent_document(&ready_agent(
                &sandbox,
                &format!("agt_child_{index:04}"),
            ))
            .unwrap()
        })
        .collect::<Vec<_>>();
    store.agents.insert_many(children).await.unwrap();
    assert!(matches!(
        store.recover_runtime_loss(loss(&sandbox)).await,
        Err(SandboxStoreError::Unavailable(_))
    ));
    unchanged(&store, &sandbox, &agent, &original).await;
    client.database(&name).drop().await.unwrap();
}
