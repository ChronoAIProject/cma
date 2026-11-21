use super::*;
use layer2_core::profiles::{SkillRef, SkillSource};
use layer2_core::sandbox::store::AgentWorkerStore;
use layer2_core::sandbox::{
    AgentSkillInstallation, SandboxCheckpointConversation, SandboxCheckpointRef,
    SandboxImageRotationPhase, SandboxRotationIdentity,
};
use layer3_adapter::{CheckpointReceipt, ConversationSnapshot, WriterState};
use sha2::{Digest, Sha256};

fn restored_rotation(sandbox: &mut Sandbox, agent: &Agent, publish: bool) {
    let archive = b"restored-conversation".to_vec();
    let conversation = SandboxCheckpointConversation::new(
        agent.id.clone(),
        agent.conversation.conversation_id.clone().unwrap(),
        ConversationSnapshot {
            conversation_id: agent.conversation.conversation_id.clone().unwrap(),
            archive_sha256: format!("{:x}", Sha256::digest(&archive)),
            archive,
        },
    )
    .unwrap();
    let checkpoint = SandboxCheckpointRef::seal(
        &CheckpointReceipt {
            epoch: 1,
            object_reference: "checkpoint/connection".into(),
            sha256: "e".repeat(64),
            writers: WriterState::Resumed,
        },
        &[conversation],
        IDLE_AT,
    )
    .unwrap();
    sandbox.lifecycle.checkpoints.publish(checkpoint).unwrap();
    let identity = SandboxRotationIdentity::capture(sandbox, std::slice::from_ref(agent)).unwrap();
    sandbox
        .begin_image_rotation("image-v2".into(), identity, LATER.into())
        .unwrap();
    let Some(SandboxLifecycleOperation::ImageRotation {
        phase,
        checkpoint_epoch,
        ..
    }) = sandbox.lifecycle.operation.as_mut()
    else {
        unreachable!()
    };
    *phase = if publish {
        SandboxImageRotationPhase::PublishActive
    } else {
        SandboxImageRotationPhase::QueuedMessageDelivery
    };
    *checkpoint_epoch = Some(1);
    sandbox.runtime.as_mut().unwrap().runtime_version = "image-v2".into();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn runtime_update_mongo_claims_only_proven_restored_connections() {
    let (client, name, store) = database().await;
    for publish in [false, true] {
        for scenario in [
            "connection",
            "skill",
            "generation",
            "conversation",
            "member",
            "phase",
            "dispatch",
            "cleanup",
            "credential",
            "stopped",
        ] {
            let mut sandbox = active_sandbox(&format!("sbx_restore_{publish}_{scenario}"));
            store.create_sandbox(sandbox.clone()).await.unwrap();
            let mut agent = ready_agent(&sandbox, &format!("agt_restore_{publish}_{scenario}"));
            let mut checkpoint_agent = agent.clone();
            if scenario == "member" {
                checkpoint_agent.id = AgentId::generate();
            }
            restored_rotation(&mut sandbox, &checkpoint_agent, publish);
            agent
                .work
                .schedule(AgentWorkStep::PublishConnection, LATER.into());
            match scenario {
                "skill" => {
                    agent.work.step = AgentWorkStep::InstallSkill;
                    agent
                        .work
                        .live_skills
                        .push(AgentSkillInstallation::new(SkillRef {
                            source: SkillSource::Ornn,
                            id: layer2_core::SkillId("recovery-skill".into()),
                            version: Some("1".into()),
                        }));
                }
                "generation" => agent.conversation.runtime_generation = Some(2),
                "conversation" => agent.conversation.conversation_id = Some("foreign".into()),
                "phase" => {
                    let Some(SandboxLifecycleOperation::ImageRotation { phase, .. }) =
                        sandbox.lifecycle.operation.as_mut()
                    else {
                        unreachable!()
                    };
                    *phase = SandboxImageRotationPhase::ConversationReopen;
                }
                "dispatch" => agent.work.step = AgentWorkStep::DispatchFirstMessage,
                "cleanup" => agent.work.step = AgentWorkStep::RemoveHome,
                "credential" => {
                    sandbox.credential.delivered = false;
                    sandbox.credential.pending = sandbox.credential.current.take();
                }
                "stopped" => agent.status = AgentStatus::Stopped,
                _ => {}
            }
            store
                .save_sandbox(1, sandbox.clone())
                .await
                .unwrap_or_else(|error| panic!("{publish} {scenario}: {error:?}"));
            insert_agent(&store, &agent, IDLE_AT).await;
            let claimed = store
                .claim_agent_work(
                    &agent.id,
                    "reconnect",
                    LATER.into(),
                    "2026-09-05T00:40:00Z".into(),
                )
                .await
                .unwrap();
            let expected = matches!(scenario, "connection" | "skill");
            assert_eq!(claimed.is_some(), expected, "{publish} {scenario}");
            if let Some(claimed) = claimed {
                assert_eq!(claimed.conversation, agent.conversation);
                assert_eq!(claimed.work.step, agent.work.step);
                assert_eq!(claimed.revision, agent.revision + 1);
                assert!(claimed.work.lease.is_some());
                let parent = store
                    .sandboxes
                    .find_one(doc! {"_id": sandbox.id.as_str()})
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(parent.get_i64("runtime_writer_claim_fence").unwrap(), 1);
            } else {
                assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
            }
            assert_eq!(store.get_sandbox(&sandbox.id).await.unwrap(), Some(sandbox));
        }
    }
    client.database(&name).drop().await.unwrap();
}
