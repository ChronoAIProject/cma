use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::{database, fixture};
use layer2_core::sandbox::store::{
    AgentConversationStore, AgentLogStore, AgentReopenGuard, AgentStore,
    SandboxLifecycleAgentMutation, SandboxLifecycleAgentScope, SandboxLifecycleExecutionStore,
    SandboxLifecycleMutation, SandboxMutationGuard, SandboxStore,
};
use layer2_core::sandbox::{
    AgentLogKind, AgentMessageAssembly, AgentStatus, AgentUpdateRecorder, AgentWorkReceipt,
    AgentWorkStep, DurableAgentUpdateRecorder, SandboxCheckpointConversation, SandboxCheckpointRef,
    SandboxEventKind, SandboxImageRotationPhase, SandboxLifecycleOperation,
    SandboxRotationIdentity, SandboxWorkStep,
};
use layer3_adapter::{
    CheckpointReceipt, ConversationSnapshot, ConversationUpdate, RuntimeOutputReceipt,
    RuntimeOutputSource, WriterState,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn reopen_output_mongo_fences_parent_and_preserves_receipts_on_publication() {
    let (client, name, store) = database().await;
    let (mut parent, mut agent) = fixture(&store, "sbx_reopen_output").await;
    let at = layer2_core::now_rfc3339();
    let native = agent.conversation.conversation_id.clone().unwrap();
    let archive = b"original conversation archive".to_vec();
    let conversation = SandboxCheckpointConversation::new(
        agent.id.clone(),
        native.clone(),
        ConversationSnapshot {
            conversation_id: native.clone(),
            archive_sha256: format!("{:x}", Sha256::digest(&archive)),
            archive,
        },
    )
    .unwrap();
    let checkpoint = SandboxCheckpointRef::seal(
        &CheckpointReceipt {
            epoch: 1,
            object_reference: "checkpoint/reopen".into(),
            sha256: "e".repeat(64),
            writers: WriterState::Resumed,
        },
        &[conversation],
        &at,
    )
    .unwrap();
    parent.lifecycle.checkpoints.publish(checkpoint).unwrap();
    let identity = SandboxRotationIdentity::capture(&parent, std::slice::from_ref(&agent)).unwrap();
    parent
        .begin_image_rotation("image-v2".into(), identity, at.clone())
        .unwrap();
    let Some(SandboxLifecycleOperation::ImageRotation {
        phase,
        checkpoint_epoch,
        ..
    }) = parent.lifecycle.operation.as_mut()
    else {
        unreachable!()
    };
    *phase = SandboxImageRotationPhase::ConversationReopen;
    *checkpoint_epoch = Some(1);
    parent.runtime.as_mut().unwrap().provision_generation = Some(2);
    parent.runtime.as_mut().unwrap().runtime_version = "image-v2".into();
    parent.lifecycle.runtime_generation_high_water = 2;
    parent
        .work
        .claim(
            "reopen-worker".into(),
            &at,
            layer2_core::rfc3339_at(
                std::time::SystemTime::now() + std::time::Duration::from_secs(300),
            ),
        )
        .unwrap();
    parent.validate().unwrap();
    agent.status = AgentStatus::Paused;
    store
        .sandboxes
        .update_one(
            doc! {"_id":parent.id.as_str()},
            doc! {"$set":crate::store::sandboxes::sandbox_replacement_fields(&parent).unwrap()},
        )
        .await
        .unwrap();
    store
        .agents
        .update_one(
            doc! {"_id":agent.id.as_str()},
            doc! {"$set":crate::store::agents::agent_replacement_fields(&agent).unwrap()},
        )
        .await
        .unwrap();
    let guard = AgentReopenGuard {
        sandbox: SandboxMutationGuard::capture(&parent),
        agent_fence: agent.work.next_fence,
        native_conversation_id: native.clone(),
        runtime_generation: 2,
    };
    let (acks, _) = tokio::sync::mpsc::channel(1);
    let output = |cursor| ConversationUpdate::RuntimeOutput {
        receipt: RuntimeOutputReceipt::new(
            RuntimeOutputSource {
                stream_id: "d99003d1-0c3c-46cd-aa47-9bbdbed50b12".into(),
                cursor,
                native_conversation_id: native.clone(),
                payload_sha256: format!("{cursor:064x}"),
            },
            acks.clone(),
        ),
        update: None,
    };
    let recorder = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
    let mut assembly = AgentMessageAssembly::default();
    // A transaction that read the parent before startup receipt publication
    // cannot subsequently replace its lease from that stale snapshot.
    let mut stale = client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut stale)
        .await
        .unwrap();
    store
        .sandboxes
        .find_one(doc! {"_id":parent.id.as_str()})
        .session(&mut stale)
        .await
        .unwrap();
    recorder
        .record_reopen_update(&agent.id, &guard, &mut assembly, output(1), at.clone())
        .await
        .unwrap();
    assert!(store
        .sandboxes
        .update_one(
            doc! {"_id":parent.id.as_str()},
            doc! {"$inc":{"agent_conversation_reopen_fence":1_i64}}
        )
        .session(&mut stale)
        .await
        .is_err());
    let _ = stale.abort_transaction().await;
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap(),
        Some(agent.clone())
    );
    let held = store
        .agent_message_checkpoint(&agent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(held.runtime_generation, 2);
    let mut obsolete = guard.clone();
    obsolete.sandbox.lease.as_mut().unwrap().fence += 1;
    assert!(recorder
        .record_reopen_update(&agent.id, &obsolete, &mut assembly, output(1), at.clone())
        .await
        .is_err());
    assert_eq!(
        store.agent_message_checkpoint(&agent.id).await.unwrap(),
        Some(held)
    );
    recorder
        .record_reopen_update(
            &agent.id,
            &guard,
            &mut AgentMessageAssembly::default(),
            output(1),
            at.clone(),
        )
        .await
        .unwrap();
    let lease = parent.work.lease.clone().unwrap();
    let mut next_parent = parent.clone();
    next_parent
        .complete_lifecycle_conversation_reopen(&lease.holder, lease.fence, at.clone())
        .unwrap();
    let mut ready = agent.clone();
    ready.conversation.runtime_generation = Some(2);
    ready
        .work
        .receipts
        .retain(|receipt| receipt.step != AgentWorkStep::OpenConversation);
    ready.work.receipts.push(AgentWorkReceipt {
        step: AgentWorkStep::OpenConversation,
        operation_id: format!("agent:{}:open:v1", agent.id),
        request_sha256: "a".repeat(64),
        completed_at: at.clone(),
    });
    ready
        .transition(AgentStatus::Ready, at.clone(), None)
        .unwrap();
    ready.conversation.last_seq += 2;
    store
        .commit_sandbox_lifecycle(SandboxLifecycleMutation {
            guard: SandboxMutationGuard::capture(&parent),
            sandbox: next_parent,
            agents: vec![SandboxLifecycleAgentMutation {
                expected_revision: agent.revision,
                agent: ready.clone(),
                logs: vec![
                    AgentLogKind::ConversationOpened {
                        conversation_id: native.clone(),
                        runtime_generation: 2,
                    },
                    AgentLogKind::StatusChanged {
                        from: AgentStatus::Paused,
                        to: AgentStatus::Ready,
                    },
                ],
            }],
            agent_scope: SandboxLifecycleAgentScope::RestoreParticipants,
            appended_at: at.clone(),
            events: vec![
                SandboxEventKind::AgentStatusChanged {
                    agent_id: agent.id.clone(),
                    from: AgentStatus::Paused,
                    to: AgentStatus::Ready,
                },
                SandboxEventKind::WorkStepCompleted {
                    step: SandboxWorkStep::RunLifecycleOperation,
                },
            ],
        })
        .await
        .unwrap();
    let checkpoint = store
        .agent_message_checkpoint(&agent.id)
        .await
        .unwrap()
        .unwrap();
    assert!(checkpoint.matches(&ready));
    assert_eq!(checkpoint.output.unwrap().cursor, 1);
    recorder
        .record_update(
            &agent.id,
            2,
            &mut AgentMessageAssembly::default(),
            output(2),
            at.clone(),
        )
        .await
        .unwrap();
    assert_eq!(
        store.agent_log(&agent.id, 0, 200).await.unwrap().len() as u64,
        ready.conversation.last_seq
    );
    assert_eq!(
        store
            .get_sandbox(&parent.id)
            .await
            .unwrap()
            .unwrap()
            .runtime,
        parent.runtime
    );
    client.database(&name).drop().await.unwrap();
}
