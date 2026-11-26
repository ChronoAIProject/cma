//! Mutate a segmented child in the original lifecycle transaction.
use super::*;
use layer2_core::sandbox::{store::*, *};
use layer3_adapter::{CheckpointReceipt, ConversationSnapshot, WriterState};
use sha2::{Digest, Sha256};

async fn pause(f: &Fixture) -> SandboxLifecycleMutation {
    let mut parent = lifecycle_caller::parent(f).await;
    parent.begin_pause(NOW.into(), None).unwrap();
    let fence = parent
        .work
        .claim("worker".into(), NOW, "2026-11-11T00:10:00Z".into())
        .unwrap();
    parent.revision += 1;
    let archive = b"segmented-child-checkpoint".to_vec();
    let capture = SandboxCheckpointConversation::new(
        f.agent.id.clone(),
        "native-fixture",
        ConversationSnapshot {
            conversation_id: "native-fixture".into(),
            archive_sha256: format!("{:x}", Sha256::digest(&archive)),
            archive,
        },
    )
    .unwrap();
    let checkpoint = SandboxCheckpointRef::seal(
        &CheckpointReceipt {
            epoch: 1,
            object_reference: "checkpoints/segmented-child".into(),
            sha256: "b".repeat(64),
            writers: WriterState::Held,
        },
        &[capture],
        NOW,
    )
    .unwrap();
    parent
        .publish_lifecycle_checkpoint(checkpoint, "worker", fence, NOW.into())
        .unwrap();
    let fence = parent
        .work
        .claim("worker".into(), NOW, "2026-11-11T00:10:00Z".into())
        .unwrap();
    parent.revision += 1;
    f.store
        .sandboxes
        .update_one(
            doc! {"_id":parent.id.as_str()},
            doc! {"$set":crate::store::sandboxes::sandbox_replacement_fields(&parent).unwrap()},
        )
        .await
        .unwrap();
    let mut next_parent = parent.clone();
    next_parent
        .complete_lifecycle_agent_pause("worker", fence, NOW.into())
        .unwrap();
    let mut next_agent = f.agent.clone();
    next_agent
        .transition(AgentStatus::Paused, NOW.into(), None)
        .unwrap();
    next_agent.conversation.last_seq += 1;
    SandboxLifecycleMutation {
        guard: SandboxMutationGuard::capture(&parent),
        sandbox: next_parent,
        agents: vec![SandboxLifecycleAgentMutation {
            expected_revision: f.agent.revision,
            agent: next_agent,
            logs: vec![AgentLogKind::StatusChanged {
                from: AgentStatus::Ready,
                to: AgentStatus::Paused,
            }],
        }],
        agent_scope: SandboxLifecycleAgentScope::AllLive,
        appended_at: NOW.into(),
        events: vec![
            SandboxEventKind::AgentStatusChanged {
                agent_id: f.agent.id.clone(),
                from: AgentStatus::Ready,
                to: AgentStatus::Paused,
            },
            SandboxEventKind::WorkStepCompleted {
                step: SandboxWorkStep::RunLifecycleOperation,
            },
        ],
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn lifecycle_child_success_uncertainty_abort_and_foreign_receipt_are_exact() {
    use crate::store::lifecycle_transaction::{apply_lifecycle, reconciliation};
    let mut f = fixture(2).await;
    let mutation = pause(&f).await;
    let mut invalid = mutation.clone();
    invalid.events.pop();
    assert!(f.store.commit_sandbox_lifecycle(invalid).await.is_err());
    unchanged(&f).await;
    assert_eq!(
        collection(&f.store, PUBLICATIONS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    let mut session = transaction(&f).await;
    let scope = TransactionScope::start(&mut session).await.unwrap();
    let original = apply_lifecycle(&f.store, &mutation, scope, None)
        .await
        .unwrap();
    assert_eq!(original.segmented.len(), 1);
    session.abort_transaction().await.unwrap();
    let mut snapshot = f.client.start_session().snapshot(true).await.unwrap();
    assert!(
        !reconciliation::snapshot(&f.store, &original, &mut snapshot)
            .await
            .unwrap()
    );
    let (admin, app) = keyed_restart::isolated(&mut f).await;
    admin.database("admin").run_command(doc! {
        "configureFailPoint":"failCommand","mode":{"times":5_i32},
        "data":{"failCommands":["commitTransaction"],"appName":app,
        "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected uncertainty"}}
    }).await.unwrap();
    let result = f.store.commit_sandbox_lifecycle(mutation.clone()).await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    result.unwrap();
    assert_eq!(
        f.store.get_agent(&f.agent.id).await.unwrap(),
        Some(mutation.agents[0].agent.clone())
    );
    let mut snapshot = f.client.start_session().snapshot(true).await.unwrap();
    assert!(reconciliation::snapshot(&f.store, &original, &mut snapshot)
        .await
        .unwrap());
    let mut foreign = original.clone();
    foreign.segmented[0].publication.head_revision += 1;
    assert!(!reconciliation::snapshot(&f.store, &foreign, &mut snapshot)
        .await
        .unwrap());
    assert_eq!(f.store.agent_log.count_documents(doc! {}).await.unwrap(), 3);
    assert_eq!(
        collection(&f.store, PUBLICATIONS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        f.store
            .sandbox_events(&f.agent.sandbox_id, 0, 10)
            .await
            .unwrap()
            .len(),
        2
    );
    f.destroy().await;
}
