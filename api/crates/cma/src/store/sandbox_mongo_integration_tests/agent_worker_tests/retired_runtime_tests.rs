use super::*;
use layer2_core::sandbox::{SandboxCheckpointConversation, SandboxCheckpointRef};
use layer3_adapter::{CheckpointReceipt, ConversationSnapshot, WriterState};
use sha2::{Digest, Sha256};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retired_delete_cleanup_is_parent_fenced_and_rejects_unsafe_parent_evidence() {
    let (client, name) = database("retired_delete_cleanup").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    for variant in ["settled", "active_parent", "foreign_owner"] {
        let (mut parent, mut worktree, claimed) = claimed_agent_work_fixture(&store, variant).await;
        let mut claimed = ready_publication_claim(claimed, "retained-conversation");
        claimed.status = AgentStatus::Stopped;
        claimed.ready_at = Some(NOW.into());
        claimed.stopped_at = Some(NOW.into());
        claimed.stop_reason = Some(AgentStopReason::Owner);
        claimed.conversation.active_turn = Some(9);
        claimed.work.step = AgentWorkStep::EnsureProcessClosed;
        replace_agent_fixture(&store, &claimed).await;
        worktree.transition(WorktreeStatus::Ready).unwrap();
        replace_worktree_fixture(&store, &worktree).await;

        let revision = parent.revision;
        parent.begin_retirement(NOW.into()).unwrap();
        store.save_sandbox(revision, parent.clone()).await.unwrap();
        let revision = parent.revision;
        parent.credential.delivered = false;
        parent.credential.current = None;
        parent
            .transition(SandboxStatus::Retired, NOW.into(), None)
            .unwrap();
        parent
            .work
            .schedule(SandboxWorkStep::WaitForPurge, NOW.into());
        store.save_sandbox(revision, parent.clone()).await.unwrap();
        // Independent parent fixtures prove the transaction does not trust the Agent alone.
        match variant {
            "active_parent" => {
                parent.status = SandboxStatus::Active;
                parent.retired_at = None;
                parent.purge_after = None;
                parent.work.schedule(SandboxWorkStep::None, NOW.into());
                store
                    .sandboxes
                    .update_one(
                        doc! { "_id": parent.id.as_str() },
                        doc! { "$set": crate::store::sandboxes::sandbox_replacement_fields(&parent).unwrap() },
                    )
                    .await
                    .unwrap();
            }
            "foreign_owner" => {
                parent.owner = SubjectId::new("other-owner");
                store
                    .sandboxes
                    .update_one(
                        doc! { "_id": parent.id.as_str() },
                        doc! { "$set": crate::store::sandboxes::sandbox_replacement_fields(&parent).unwrap() },
                    )
                    .await
                    .unwrap();
            }
            _ => {}
        }
        let before_parent = store
            .sandboxes
            .find_one(doc! { "_id": parent.id.as_str() })
            .await
            .unwrap()
            .unwrap();
        let fence = claimed.work.lease.as_ref().unwrap().fence;
        let mut next = claimed.clone();
        next.work
            .advance("worker", fence, AgentWorkStep::RemoveHome, NOW.into())
            .unwrap();
        next.revision += 1;
        next.conversation.last_seq += 1;
        let worktree_revision = worktree.revision;
        worktree.transition(WorktreeStatus::Removing).unwrap();
        let result = store
            .commit_agent_work(AgentWorkMutation {
                expected_agent_revision: claimed.revision,
                holder: "worker".into(),
                fence,
                agent: next.clone(),
                worktree: Some(VersionedWorktree {
                    expected_revision: worktree_revision,
                    worktree,
                }),
                appended_at: NOW.into(),
                sandbox_event: None,
                agent_logs: vec![AgentLogKind::WorkStepCompleted {
                    step: AgentWorkStep::EnsureProcessClosed,
                }],
            })
            .await;
        let after_parent = store
            .sandboxes
            .find_one(doc! { "_id": parent.id.as_str() })
            .await
            .unwrap()
            .unwrap();
        if variant == "settled" {
            assert_eq!(result, Ok(()));
            assert_eq!(
                store.get_agent(&claimed.id).await.unwrap(),
                Some(next.clone())
            );
            assert_eq!(next.work.receipts, claimed.work.receipts);
            assert_eq!(store.agent_log(&claimed.id, 0, 10).await.unwrap().len(), 1);
            assert_eq!(
                after_parent.get_i64("runtime_writer_claim_fence").unwrap(),
                before_parent
                    .get_i64("runtime_writer_claim_fence")
                    .unwrap_or(0)
                    + 1
            );
            assert_eq!(store.get_sandbox(&parent.id).await.unwrap(), Some(parent));
        } else {
            assert_eq!(result, Err(SandboxStoreError::Conflict), "{variant}");
            assert_eq!(
                store.get_agent(&claimed.id).await.unwrap(),
                Some(claimed.clone())
            );
            assert!(store
                .agent_log(&claimed.id, 0, 10)
                .await
                .unwrap()
                .is_empty());
            assert_eq!(after_parent, before_parent);
        }
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retired_runtime_close_is_parent_fenced_and_rejects_unsafe_parent_evidence() {
    let (client, name) = database("retired_runtime_close").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    for variant in ["settled", "active_parent", "missing_checkpoint"] {
        let (mut parent, mut worktree, claimed) = claimed_agent_work_fixture(&store, variant).await;
        let mut claimed = ready_publication_claim(claimed, "retained-conversation");
        claimed.status = AgentStatus::Paused;
        claimed.ready_at = Some(NOW.into());
        claimed.requested_stop_reason = Some(AgentStopReason::SandboxRetired);
        claimed.work.step = AgentWorkStep::CloseProcess;
        claimed.conversation.runtime_generation = Some(4);
        replace_agent_fixture(&store, &claimed).await;
        worktree.transition(WorktreeStatus::Ready).unwrap();
        replace_worktree_fixture(&store, &worktree).await;

        let archive = b"retained conversation".to_vec();
        let conversation = SandboxCheckpointConversation::new(
            claimed.id.clone(),
            "retained-conversation",
            ConversationSnapshot {
                conversation_id: "retained-conversation".into(),
                archive_sha256: format!("{:x}", Sha256::digest(&archive)),
                archive,
            },
        )
        .unwrap();
        parent.lifecycle.checkpoints.latest = Some(
            SandboxCheckpointRef::seal(
                &CheckpointReceipt {
                    epoch: 1,
                    object_reference: "checkpoint/paused".into(),
                    sha256: "a".repeat(64),
                    writers: WriterState::Resumed,
                },
                &[conversation],
                NOW,
            )
            .unwrap(),
        );
        parent.lifecycle.runtime_generation_high_water = 3;
        let parent_revision = parent.revision;
        parent.status = SandboxStatus::Paused;
        parent.begin_retirement(NOW.into()).unwrap();
        match variant {
            "active_parent" => {
                parent.status = SandboxStatus::Active;
                parent.work.schedule(SandboxWorkStep::None, NOW.into());
            }
            "missing_checkpoint" => parent.lifecycle.checkpoints.latest = None,
            _ => {}
        }
        store
            .save_sandbox(parent_revision, parent.clone())
            .await
            .unwrap();
        let before_parent = store
            .sandboxes
            .find_one(doc! { "_id": parent.id.as_str() })
            .await
            .unwrap()
            .unwrap();
        let fence = claimed.work.lease.as_ref().unwrap().fence;
        let mut next = claimed.clone();
        next.work
            .advance("worker", fence, AgentWorkStep::PublishStopped, NOW.into())
            .unwrap();
        next.revision += 1;
        next.conversation.last_seq += 1;
        let mutation = AgentWorkMutation {
            expected_agent_revision: claimed.revision,
            holder: "worker".into(),
            fence,
            agent: next.clone(),
            worktree: None,
            appended_at: NOW.into(),
            sandbox_event: None,
            agent_logs: vec![AgentLogKind::WorkStepCompleted {
                step: AgentWorkStep::CloseProcess,
            }],
        };
        let result = store.commit_agent_work(mutation).await;
        let after_parent = store
            .sandboxes
            .find_one(doc! { "_id": parent.id.as_str() })
            .await
            .unwrap()
            .unwrap();
        if variant == "settled" {
            assert_eq!(result, Ok(()));
            assert_eq!(
                store.get_agent(&claimed.id).await.unwrap(),
                Some(next.clone())
            );
            assert_eq!(next.work.receipts, claimed.work.receipts);
            assert_eq!(store.agent_log(&claimed.id, 0, 10).await.unwrap().len(), 1);
            assert_eq!(
                after_parent.get_i64("runtime_writer_claim_fence").unwrap(),
                before_parent
                    .get_i64("runtime_writer_claim_fence")
                    .unwrap_or(0)
                    + 1,
            );
            assert_eq!(store.get_sandbox(&parent.id).await.unwrap(), Some(parent));
        } else {
            assert_eq!(result, Err(SandboxStoreError::Conflict), "{variant}");
            assert_eq!(
                store.get_agent(&claimed.id).await.unwrap(),
                Some(claimed.clone())
            );
            assert!(store
                .agent_log(&claimed.id, 0, 10)
                .await
                .unwrap()
                .is_empty());
            assert_eq!(after_parent, before_parent);
        }
    }
    client.database(&name).drop().await.unwrap();
}
