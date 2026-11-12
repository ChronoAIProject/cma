//! Reopen receipts are not read-only: the original port writes a parent fence.
use super::*;
use layer2_core::sandbox::store::{AgentReopenGuard, SandboxMutationGuard};
use layer2_core::sandbox::{
    AgentMessageAssembly, AgentStatus, SandboxCheckpointConversation, SandboxCheckpointRef,
    SandboxImageRotationPhase, SandboxLifecycleOperation, SandboxRotationIdentity,
};
use layer3_adapter::{CheckpointReceipt, ConversationSnapshot, RuntimeOutputSource, WriterState};
use sha2::{Digest, Sha256};

async fn reopening(
    store: &MongoSandboxStore,
) -> (Sandbox, Agent, AgentReopenGuard, AgentMessageCheckpoint) {
    let (mut parent, mut current) =
        crate::store::sandbox_maintenance::mongo_integration_tests::fixture(
            store,
            "sbx_output_reopen",
        )
        .await;
    parent.runtime.as_mut().unwrap().boot_id = Some("b".repeat(32));
    support::retain_home(&mut parent);
    let at = layer2_core::now_rfc3339();
    let native = current.conversation.conversation_id.clone().unwrap();
    let archive = b"original conversation fixture".to_vec();
    let conversation = SandboxCheckpointConversation::new(
        current.id.clone(),
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
            object_reference: "checkpoint/output-fixture".into(),
            sha256: "e".repeat(64),
            writers: WriterState::Resumed,
        },
        &[conversation],
        &at,
    )
    .unwrap();
    parent.lifecycle.checkpoints.publish(checkpoint).unwrap();
    let identity =
        SandboxRotationIdentity::capture(&parent, std::slice::from_ref(&current)).unwrap();
    parent
        .begin_image_rotation("fixture-image-v2".into(), identity, at.clone())
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
    parent.runtime.as_mut().unwrap().runtime_version = "fixture-image-v2".into();
    parent.lifecycle.runtime_generation_high_water = 2;
    parent
        .work
        .claim("reopen-fixture".into(), &at, "2999-01-01T00:00:00Z".into())
        .unwrap();
    parent.validate().unwrap();
    current.status = AgentStatus::Paused;
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
            doc! {"_id":current.id.as_str()},
            doc! {"$set":crate::store::agents::agent_replacement_fields(&current).unwrap()},
        )
        .await
        .unwrap();
    let guard = AgentReopenGuard {
        sandbox: SandboxMutationGuard::capture(&parent),
        agent_fence: current.work.next_fence,
        native_conversation_id: native.clone(),
        runtime_generation: 2,
    };
    let mut next = partial(&guard.project(&current).unwrap(), "");
    next.assembly = AgentMessageAssembly::default();
    next.output = Some(RuntimeOutputSource {
        stream_id: uuid::Uuid::new_v4().to_string(),
        cursor: 1,
        native_conversation_id: native,
        payload_sha256: "d".repeat(64),
    });
    (parent, current, guard, next)
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and output hooks"]
async fn reopen_output_and_its_receipt_replay_cannot_bypass_either_hold() {
    for replay in [false, true] {
        for hold in [0, 1, 2] {
            let (client, name, store) = database().await;
            let (parent, current, guard, next) = reopening(&store).await;
            if replay {
                assert_eq!(
                    store
                        .commit_agent_reopen_update(
                            current.clone(),
                            None,
                            next.clone(),
                            None,
                            guard.clone()
                        )
                        .await
                        .unwrap(),
                    next.outcome
                );
            }
            if hold != 0 {
                maintenance::hold(&store, &parent, hold == 1).await;
            }
            let before = support::all_rows(&store).await;
            let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
            let result = reopened
                .commit_agent_reopen_update(current.clone(), None, next.clone(), None, guard)
                .await;
            if hold == 0 {
                assert_eq!(result.unwrap(), next.outcome);
                assert!(reopened.get_agent(&current.id).await.unwrap() == Some(current.clone()));
                assert_eq!(
                    reopened
                        .agent_message_checkpoint(&current.id)
                        .await
                        .unwrap(),
                    Some(next)
                );
                assert_ne!(support::all_rows(&store).await, before);
            } else {
                assert_eq!(result, Err(SandboxStoreError::Conflict));
                assert_eq!(support::all_rows(&store).await, before);
            }
            client.database(&name).drop().await.unwrap();
        }
    }
}
