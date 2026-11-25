use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::{database, fixture};
use crate::store::work_claim_authority::LEDGER_MAINTENANCE_FIELD;
use layer2_core::sandbox::store::{
    AgentConversationStore, AgentLogStore, AgentReopenGuard, AgentStore, ReopenRefusalFence,
    SandboxLifecycleAgentMutation, SandboxLifecycleAgentScope, SandboxLifecycleExecutionStore,
    SandboxLifecycleMutation, SandboxMutationGuard, SandboxStore,
};
use layer2_core::sandbox::{
    AgentConversationError, AgentLogKind, AgentMessageAssembly, AgentStatus, AgentUpdateRecorder,
    AgentWorkReceipt, AgentWorkStep, DurableAgentUpdateRecorder, SandboxCheckpointConversation,
    SandboxCheckpointRef, SandboxEventKind, SandboxImageRotationPhase, SandboxLifecycleOperation,
    SandboxRotationIdentity, SandboxRuntimeUpdateRequest, SandboxRuntimeUpdateState,
    SandboxWorkStep, TypedFailure,
};
use layer3_adapter::{
    CheckpointReceipt, ConversationSnapshot, ConversationUpdate, RuntimeOutputReceipt,
    RuntimeOutputSource, WriterState,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;
#[path = "reopen_owner_tests.rs"]
mod ownership;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn reopen_output_mongo_fences_parent_and_preserves_receipts_on_publication() {
    for count in [0, 2, 3] {
        run_case(count, false).await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn runtime_update_reopen_refusal_mongo_fences_checkpoint_owner_and_claim() {
    run_case(2, true).await;
}

#[tokio::test]
#[ignore = "requires a disposable local MongoDB replica set"]
async fn runtime_update_reopen_http_requires_the_callers_checkpoint_and_owner_fence() {
    crate::sandbox::exercise_runtime_update_reopen_http().await;
}

async fn run_case(count: usize, park: bool) {
    let (client, name, store) = database().await;
    let (mut parent, mut agent) = fixture(&store, "sbx_reopen_output").await;
    let (store, authority) = ownership::setup(store, &agent, count).await;
    ownership::under(authority.as_ref(), async {
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
        if park {
            parent.lifecycle.runtime_update = Some(SandboxRuntimeUpdateState {
                checked_at: at.clone(),
                observed_version: "image-v2".into(),
                request: Some(SandboxRuntimeUpdateRequest {
                    target_version: "image-v2".into(),
                    requested_at: at.clone(),
                    automatic: false,
                    started_at: None,
                    completed_at: None,
                    failure: None,
                    retained_transition: None,
                }),
            });
        }
        let identity =
            SandboxRotationIdentity::capture(&parent, std::slice::from_ref(&agent)).unwrap();
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
            owner_restart: None,
            sandbox: SandboxMutationGuard::capture(&parent),
            agent_fence: agent.work.next_fence,
            native_conversation_id: native.clone(),
            runtime_generation: 2,
        };
        let (acks, _) = tokio::sync::mpsc::channel(1);
        let output_with_ack = |cursor, acknowledgements| ConversationUpdate::RuntimeOutput {
            receipt: RuntimeOutputReceipt::new(
                RuntimeOutputSource {
                    stream_id: "d99003d1-0c3c-46cd-aa47-9bbdbed50b12".into(),
                    cursor,
                    native_conversation_id: native.clone(),
                    payload_sha256: format!("{cursor:064x}"),
                },
                acknowledgements,
            ),
            update: None,
        };
        let output = |cursor| output_with_ack(cursor, acks.clone());
        let recorder = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
        let mut assembly = AgentMessageAssembly::default();
        if park {
            assert_eq!(
                recorder
                    .record_reopen_update(&agent.id, &guard, &mut assembly, output(3), at.clone())
                    .await,
                Err(AgentConversationError::ReopenOutputDiscontinuity)
            );
            assert!(store
                .agent_message_checkpoint(&agent.id)
                .await
                .unwrap()
                .is_none());
            recorder
                .record_reopen_update(&agent.id, &guard, &mut assembly, output(1), at.clone())
                .await
                .unwrap();
            assert_eq!(
                recorder
                    .record_reopen_update(&agent.id, &guard, &mut assembly, output(3), at.clone())
                    .await,
                Err(AgentConversationError::ReopenOutputDiscontinuity)
            );
            prove_refusal_fences(&store, &parent, &agent, authority.as_ref().unwrap()).await;
            return;
        }
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
        let log_before_hold = store.agent_log(&agent.id, 0, 200).await.unwrap();
        let hold = doc! { "phase": "requested" };
        let mut fields = doc! {};
        fields.insert(LEDGER_MAINTENANCE_FIELD, hold.clone());
        let parent_id = doc! { "_id": parent.id.as_str() };
        // The guard was captured before the hold and no WorkAuthority scope is active.
        assert!(!layer2_core::sandbox::store::WorkAuthority::active());
        assert_eq!(
            store
                .sandboxes
                .update_one(parent_id.clone(), doc! { "$set": fields })
                .await
                .unwrap()
                .matched_count,
            1,
        );
        let (sender, mut acknowledgements) = tokio::sync::mpsc::channel(1);
        let refused = recorder
            .record_reopen_update(
                &agent.id,
                &guard,
                &mut AgentMessageAssembly::default(),
                output_with_ack(2, sender),
                at.clone(),
            )
            .await;
        assert_eq!(
            refused,
            Err(AgentConversationError::Unavailable(
                "conversation changed during every commit attempt".into(),
            ))
        );
        assert!(acknowledgements.try_recv().is_err());
        assert_eq!(
            store.agent_message_checkpoint(&agent.id).await.unwrap(),
            Some(held.clone())
        );
        assert_eq!(
            store.agent_log(&agent.id, 0, 200).await.unwrap(),
            log_before_hold
        );
        assert_eq!(
            store.get_agent(&agent.id).await.unwrap(),
            Some(agent.clone())
        );
        let persisted = store
            .sandboxes
            .find_one(parent_id.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            persisted.get_document(LEDGER_MAINTENANCE_FIELD).unwrap(),
            &hold
        );
        let mut fields = doc! {};
        fields.insert(LEDGER_MAINTENANCE_FIELD, "");
        store
            .sandboxes
            .update_one(parent_id, doc! { "$unset": fields })
            .await
            .unwrap();
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
        let mutation = SandboxLifecycleMutation {
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
        };
        let successor = ownership::publish(&store, mutation, authority.as_ref()).await;
        let checkpoint = store
            .agent_message_checkpoint(&agent.id)
            .await
            .unwrap()
            .unwrap();
        assert!(checkpoint.matches(&ready));
        assert_eq!(checkpoint.output.unwrap().cursor, 1);
        ownership::under(
            successor.as_ref(),
            recorder.record_update(
                &agent.id,
                2,
                &mut AgentMessageAssembly::default(),
                output(2),
                at.clone(),
            ),
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
    })
    .await;
    client.database(&name).drop().await.unwrap();
}

async fn prove_refusal_fences(
    store: &MongoSandboxStore,
    parent: &layer2_core::sandbox::Sandbox,
    agent: &layer2_core::sandbox::Agent,
    authority: &layer2_core::agent_ownership::Authority,
) {
    use layer2_core::agent_ownership::{candidates, OwnershipStore};
    let at = layer2_core::now_rfc3339();
    let failure = TypedFailure {
        class: "terminal_validation".into(),
        detail: "retained conversation output is discontinuous".into(),
    };
    let mut parked = parent.clone();
    let lease = parent.work.lease.as_ref().unwrap();
    parked
        .park_failed_runtime_update(failure.clone(), &lease.holder, lease.fence, at.clone())
        .unwrap();
    let mutation = SandboxLifecycleMutation {
        guard: SandboxMutationGuard::capture(parent),
        sandbox: parked.clone(),
        agents: Vec::new(),
        agent_scope: SandboxLifecycleAgentScope::Touched,
        appended_at: at,
        events: vec![SandboxEventKind::WorkStepFailed {
            step: SandboxWorkStep::RunLifecycleOperation,
            failure,
        }],
    };
    let mut fence = ReopenRefusalFence {
        agent_id: agent.id.clone(),
        agent_revision: agent.revision,
        checkpoint: None,
        owner: Some(authority.token.clone()),
        released: None,
    };
    assert_eq!(
        store
            .park_sandbox_reopen_refusal(mutation.clone(), fence.clone())
            .await,
        Err(SandboxStoreError::Conflict)
    );
    fence.checkpoint = store
        .agent_message_checkpoint(&agent.id)
        .await
        .unwrap()
        .map(Box::new);
    let mut stale_claim = mutation.clone();
    stale_claim.guard.lease.as_mut().unwrap().fence += 1;
    assert_eq!(
        store
            .park_sandbox_reopen_refusal(stale_claim, fence.clone())
            .await,
        Err(SandboxStoreError::Conflict)
    );
    let owners = store.agent_ownership.as_ref().unwrap();
    owners.release(&authority.token).await.unwrap();
    owners
        .heartbeat(&authority.token.instance_id, true)
        .await
        .unwrap();
    let membership = owners.membership().await.unwrap();
    let member = candidates(&agent.id, &membership, None).unwrap().remove(0);
    let successor = owners
        .acquire(&agent.id, &member.instance_id)
        .await
        .unwrap()
        .unwrap()
        .token;
    assert!(successor.epoch > authority.token.epoch);
    assert_eq!(
        store
            .park_sandbox_reopen_refusal(mutation.clone(), fence.clone())
            .await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store.get_sandbox(&parent.id).await.unwrap().unwrap(),
        *parent
    );
    fence.owner = Some(successor);
    // A fresh reader sees the same checkpoint and can park under its new epoch.
    store
        .park_sandbox_reopen_refusal(mutation, fence)
        .await
        .unwrap();
    assert_eq!(
        store.get_sandbox(&parent.id).await.unwrap().unwrap(),
        parked
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap().unwrap(), *agent);
}
