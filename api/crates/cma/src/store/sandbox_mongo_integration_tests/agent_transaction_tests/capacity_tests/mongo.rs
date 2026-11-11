use super::*;
use crate::store::agent_log_integrity::{NORMAL_AGENT_LOG_BYTES, NORMAL_AGENT_LOG_ENTRIES};
use futures_util::TryStreamExt;
use sha2::{Digest, Sha256};

mod retry_tests;
mod telemetry_tests;
mod terminal_tests;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_log_count_capacity_keeps_stop_and_delete_reachable() {
    verify_full_log_cleanup(false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_log_byte_capacity_keeps_stop_and_delete_reachable() {
    verify_full_log_cleanup(true).await;
}

async fn verify_full_log_cleanup(bytes: bool) {
    let (client, name) = database(if bytes {
        "capacity_bytes"
    } else {
        "capacity_count"
    })
    .await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let agent = ready_conversation_fixture(&store, "full_log_cleanup").await;
    let agent = fill_normal_capacity(&store, agent, bytes).await;
    let original_head = agent.conversation.last_seq;
    let original_history = fingerprint(&store, &agent.id, original_head).await;
    let claimed = claim(&store, &agent.id, NOW).await;
    let publication = advance(&claimed, AgentWorkStep::None, NOW);
    for _ in 0..2 {
        assert_eq!(
            store.commit_agent_work(publication.clone()).await,
            Err(SandboxStoreError::AgentLogCapacityExceeded)
        );
        assert_eq!(
            store.get_agent(&agent.id).await.unwrap(),
            Some(claimed.clone())
        );
    }
    assert!(!store
        .due_agent_work(MINUTE, 200)
        .await
        .unwrap()
        .contains(&agent.id));

    // Owner stop supersedes the failed connection claim, using the published
    // process's original close identity. No replacement conversation is created.
    store
        .schedule_agent_stop(AgentStopRequest {
            target: AgentControlTarget::from(&claimed),
            reason: AgentStopReason::Owner,
            requested_at: MINUTE.into(),
        })
        .await
        .unwrap();
    let fenced = claim(&store, &agent.id, MINUTE).await;
    store
        .commit_agent_work(advance(&fenced, AgentWorkStep::CloseProcess, MINUTE))
        .await
        .unwrap();
    let closing = claim(&store, &agent.id, MINUTE).await;
    let mut closed = advance(&closing, AgentWorkStep::PublishStopped, MINUTE);
    add_receipt(&mut closed, AgentWorkStep::CloseProcess);
    let mut unproved = closed.clone();
    unproved.agent.work.receipts = closing.work.receipts.clone();
    assert_eq!(
        store.commit_agent_work(unproved).await,
        Err(SandboxStoreError::Conflict)
    );
    let mut wrong_holder = closed.clone();
    wrong_holder.holder = "replaced-worker".into();
    assert_eq!(
        store.commit_agent_work(wrong_holder).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(closing));
    store.commit_agent_work(closed).await.unwrap();

    let publishing = claim(&store, &agent.id, MINUTE).await;
    let mut stopped = advance(&publishing, AgentWorkStep::None, MINUTE);
    // advance already accounts for the single publication revision.
    stopped.agent.revision -= 1;
    stopped
        .agent
        .transition(
            AgentStatus::Stopped,
            MINUTE.into(),
            Some(AgentStopReason::Owner),
        )
        .unwrap();
    stopped.agent_logs = vec![AgentLogKind::Stopped {
        reason: "owner".into(),
    }];
    stopped.sandbox_event = Some(SandboxEventKind::AgentStatusChanged {
        agent_id: agent.id.clone(),
        from: publishing.status,
        to: AgentStatus::Stopped,
    });
    store.commit_agent_work(stopped.clone()).await.unwrap();
    assert!(stopped.agent.work.lease.is_none());
    assert_eq!(stopped.agent.work.step, AgentWorkStep::None);

    store
        .schedule_agent_delete(AgentDeleteRequest {
            target: AgentControlTarget::from(&stopped.agent),
            requested_at: TWO_MINUTES.into(),
        })
        .await
        .unwrap();
    for next in [
        AgentWorkStep::RemoveHome,
        AgentWorkStep::RemoveWorktree,
        AgentWorkStep::PublishDeleted,
    ] {
        let current = claim(&store, &agent.id, TWO_MINUTES).await;
        let mut publication = advance(&current, next, TWO_MINUTES);
        match current.work.step {
            AgentWorkStep::EnsureProcessClosed => {
                // The existing CloseProcess receipt proves the process effect.
                change_worktree(&store, &mut publication, WorktreeStatus::Removing).await;
            }
            AgentWorkStep::RemoveHome => add_receipt(&mut publication, AgentWorkStep::RemoveHome),
            AgentWorkStep::RemoveWorktree => {
                add_receipt(&mut publication, AgentWorkStep::RemoveWorktree);
                change_worktree(&store, &mut publication, WorktreeStatus::Removed).await;
                publication.sandbox_event = Some(SandboxEventKind::WorktreeRemoved {
                    workspace_id: current.workspace_id.clone(),
                    worktree_id: current.worktree_id.clone(),
                });
            }
            _ => panic!("unexpected cleanup step"),
        }
        store.commit_agent_work(publication).await.unwrap();
    }
    let publishing = claim(&store, &agent.id, TWO_MINUTES).await;
    let mut deleted = advance(&publishing, AgentWorkStep::None, TWO_MINUTES);
    deleted.agent.revision -= 1;
    deleted
        .agent
        .transition(AgentStatus::Deleted, TWO_MINUTES.into(), None)
        .unwrap();
    deleted.agent_logs = vec![AgentLogKind::Deleted];
    deleted.sandbox_event = Some(SandboxEventKind::AgentDeleted {
        agent_id: agent.id.clone(),
    });
    store.commit_agent_work(deleted.clone()).await.unwrap();

    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened.get_agent(&agent.id).await.unwrap(),
        Some(deleted.agent.clone())
    );
    assert_eq!(
        fingerprint(&reopened, &agent.id, original_head).await,
        original_history
    );
    let tail = reopened
        .agent_log(&agent.id, original_head, 200)
        .await
        .unwrap();
    assert_eq!(tail.len(), 7);
    assert_eq!(tail.last().unwrap().seq, original_head + 7);
    assert_eq!(tail.last().unwrap().kind, AgentLogKind::Deleted);
    assert!(reopened
        .due_agent_work(TWO_MINUTES, 200)
        .await
        .unwrap()
        .is_empty());
    client.database(&name).drop().await.unwrap();
}

async fn claim(store: &MongoSandboxStore, id: &AgentId, at: &str) -> Agent {
    store
        .claim_agent_work(id, "worker", at.into(), "2026-09-05T00:20:00Z".into())
        .await
        .unwrap()
        .unwrap()
}

fn advance(current: &Agent, next: AgentWorkStep, at: &str) -> AgentWorkMutation {
    let fence = current.work.lease.as_ref().unwrap().fence;
    let mut agent = current.clone();
    agent
        .work
        .advance("worker", fence, next, at.into())
        .unwrap();
    agent.revision += 1;
    agent.updated_at = at.into();
    agent.conversation.last_seq += 1;
    AgentWorkMutation {
        expected_agent_revision: current.revision,
        holder: "worker".into(),
        fence,
        agent,
        worktree: None,
        appended_at: at.into(),
        sandbox_event: None,
        agent_logs: vec![AgentLogKind::WorkStepCompleted {
            step: current.work.step,
        }],
    }
}

fn add_receipt(publication: &mut AgentWorkMutation, step: AgentWorkStep) {
    let agent = &mut publication.agent;
    let operation_id = match step {
        AgentWorkStep::CloseProcess => format!("agent:{}:close:v1", agent.id),
        AgentWorkStep::RemoveHome => format!("agent:{}:home:remove:v1", agent.id),
        AgentWorkStep::RemoveWorktree => format!("worktree:{}:remove:v1", agent.worktree_id),
        _ => panic!("unexpected cleanup receipt"),
    };
    agent.work.receipts.push(AgentWorkReceipt {
        step,
        operation_id,
        request_sha256: "b".repeat(64),
        completed_at: publication.appended_at.clone(),
    });
}

async fn change_worktree(
    store: &MongoSandboxStore,
    publication: &mut AgentWorkMutation,
    to: WorktreeStatus,
) {
    let mut worktree = store
        .get_worktree(&publication.agent.worktree_id)
        .await
        .unwrap()
        .unwrap();
    let expected_revision = worktree.revision;
    worktree.transition(to).unwrap();
    publication.worktree = Some(VersionedWorktree {
        expected_revision,
        worktree,
    });
}

async fn fill_normal_capacity(store: &MongoSandboxStore, mut agent: Agent, bytes: bool) -> Agent {
    let mut existing = store
        .agent_log
        .find(doc! {"agent_id": agent.id.as_str()})
        .await
        .unwrap();
    let mut used = 0;
    while let Some(doc) = existing.try_next().await.unwrap() {
        used += doc.to_vec().unwrap().len();
    }
    let mut documents = Vec::new();
    while if bytes {
        used < NORMAL_AGENT_LOG_BYTES
    } else {
        agent.conversation.last_seq < NORMAL_AGENT_LOG_ENTRIES
    } {
        agent.conversation.last_seq += 1;
        let mut entry = AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: agent.conversation.last_seq,
            appended_at: NOW.into(),
            kind: AgentLogKind::ToolOutputReported {
                call_id: "retained-capacity-fixture".into(),
                output: layer2_core::sandbox::RecordedToolOutput {
                    text: "Retained tool output".into(),
                    truncated: false,
                },
            },
        };
        if bytes {
            entry.kind = AgentLogKind::OwnerMessage {
                content: String::new(),
            };
            let overhead = log_document(&entry).unwrap().to_vec().unwrap().len();
            let remaining = NORMAL_AGENT_LOG_BYTES - used;
            let size = if remaining <= 128 * 1024 + overhead {
                remaining
            } else {
                128 * 1024
            };
            entry.kind = AgentLogKind::OwnerMessage {
                content: "x".repeat(size - overhead),
            };
        }
        let document = log_document(&entry).unwrap();
        used += document.to_vec().unwrap().len();
        documents.push(document);
    }
    if bytes {
        assert_eq!(used, NORMAL_AGENT_LOG_BYTES);
    }
    store.agent_log.insert_many(documents).await.unwrap();
    let mut fields = agent_replacement_fields(&agent).unwrap();
    fields.insert(
        "last_seq",
        i64::try_from(agent.conversation.last_seq).unwrap(),
    );
    store
        .agents
        .update_one(doc! {"_id": agent.id.as_str()}, doc! {"$set":fields})
        .await
        .unwrap();
    agent
}

async fn fingerprint(store: &MongoSandboxStore, id: &AgentId, through: u64) -> String {
    let mut cursor = store
        .agent_log
        .find(doc! {"agent_id":id.as_str(), "seq":{"$lte":i64::try_from(through).unwrap()}})
        .sort(doc! {"seq":1})
        .await
        .unwrap();
    let mut hash = Sha256::new();
    while let Some(document) = cursor.try_next().await.unwrap() {
        hash.update(document.to_vec().unwrap());
    }
    format!("{:x}", hash.finalize())
}
