//! Retirement uses the existing aggregate transaction in both log formats.
use super::*;
use layer2_core::sandbox::store::{AgentWorkMutation, AgentWorkerStore};
use layer2_core::sandbox::{
    AgentCleanupDebt, AgentLogEntry, FirstMessageDelivery, FirstMessageRecord, QueuedAgentMessage,
};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retirement_case6_requires_exact_native_finality_across_two_coordinators() {
    crate::sandbox::exercise_native_retirement_finality().await;
}

pub(super) fn finality_loss(sandbox: &Sandbox) -> SandboxRuntimeLossRequest {
    SandboxRuntimeLossRequest {
        native_retirement_finality: true,
        ..loss(sandbox)
    }
}

pub(super) async fn retiring(store: &MongoSandboxStore, segmented: bool) -> (Sandbox, Vec<Agent>) {
    let mut sandbox = active_sandbox("sbx_retirement");
    sandbox.runtime.as_mut().unwrap().runtime_id = "agent-sandbox:retiring:original-uid".into();
    store.create_sandbox(sandbox.clone()).await.unwrap();
    let revision = sandbox.revision;
    sandbox.begin_retirement(BEFORE.into()).unwrap();
    store.save_sandbox(revision, sandbox.clone()).await.unwrap();
    let mut working = ready_agent(&sandbox, "agt_working");
    working.status = AgentStatus::Working;
    working.conversation.active_turn = Some(5);
    working.conversation.last_seq = 5;
    working.conversation.operation = Some(
        AgentOperationRecord::new(
            1,
            1,
            5,
            AgentOperationIntent::send_message("interrupted owner work"),
            BEFORE.into(),
        )
        .unwrap(),
    );
    working.conversation.queued_messages =
        vec![QueuedAgentMessage::new(4, "queued work".into(), BEFORE.into()).unwrap()];
    working
        .request_stop(AgentStopReason::Owner, BEFORE.into())
        .unwrap();
    working
        .work
        .schedule(AgentWorkStep::CloseProcess, BEFORE.into());
    working
        .work
        .claim(
            "old-worker".into(),
            "2026-09-05T00:29:00Z",
            "2026-09-05T00:47:00Z".into(),
        )
        .unwrap();
    working.work.cleanup_debt.push(AgentCleanupDebt {
        step: AgentWorkStep::OpenConversation,
        operation_id: "original-open".into(),
        request_sha256: "a".repeat(64),
    });
    store
        .agents
        .insert_one(crate::store::agents::agent_document(&working).unwrap())
        .await
        .unwrap();
    for (i, kind) in [
        AgentLogKind::Created {
            workspace_id: working.workspace_id.clone(),
            worktree_id: working.worktree_id.clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: working.conversation.conversation_id.clone().unwrap(),
            runtime_generation: 1,
        },
        AgentLogKind::OwnerMessage {
            content: "interrupted owner work".into(),
        },
        AgentLogKind::OwnerMessage {
            content: "queued work".into(),
        },
        AgentLogKind::TurnStarted {
            turn_sequence: 5,
            origin_key: Some("queued-message:3".into()),
            model: None,
        },
    ]
    .into_iter()
    .enumerate()
    {
        store
            .agent_log
            .insert_one(
                crate::store::agent_log::log_document(&AgentLogEntry {
                    agent_id: working.id.clone(),
                    seq: i as u64 + 1,
                    appended_at: BEFORE.into(),
                    kind,
                })
                .unwrap(),
            )
            .await
            .unwrap();
    }
    let worktree = layer2_core::sandbox::Worktree {
        id: working.worktree_id.clone(),
        workspace_id: working.workspace_id.clone(),
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        name: "wt-1".into(),
        path: "/home/cma/project/.worktrees/wt-1".into(),
        branch: "cma/wt-1".into(),
        base_commit: "synthetic-original".into(),
        status: layer2_core::sandbox::WorktreeStatus::Ready,
        agent_id: Some(working.id.clone()),
        revision: 1,
    };
    store
        .worktrees
        .insert_one(crate::store::worktrees::worktree_document(&worktree).unwrap())
        .await
        .unwrap();
    if segmented {
        let db = store.client.database(&store.agents.namespace().db);
        crate::store::agent_log_segments::schema::migrate(&db)
            .await
            .unwrap();
        crate::store::migrations::agent_log_source_keys::prepare_for_test(&db).await;
        working = scoped::admit(store, &working, 0).await;
    }
    let mut children = vec![working];
    for (id, status) in [
        ("agt_ready", AgentStatus::Ready),
        ("agt_starting", AgentStatus::Starting),
        ("agt_paused", AgentStatus::Paused),
        ("agt_terminal", AgentStatus::Stopped),
    ] {
        let mut child = ready_agent(&sandbox, id);
        child.status = status;
        if status == AgentStatus::Starting {
            child.ready_at = None;
            child.conversation.conversation_id = None;
            child.conversation.runtime_generation = None;
            child.conversation.first_message =
                Some(FirstMessageRecord::new("launch", "first work", BEFORE).unwrap());
        }
        if status == AgentStatus::Stopped {
            child.stop_reason = Some(AgentStopReason::Owner);
            child.stopped_at = Some(BEFORE.into());
        }
        if status == AgentStatus::Starting {
            store
                .agents
                .insert_one(crate::store::agents::agent_document(&child).unwrap())
                .await
                .unwrap();
            store
                .agent_log
                .insert_one(
                    crate::store::agent_log::log_document(&AgentLogEntry {
                        agent_id: child.id.clone(),
                        seq: 1,
                        appended_at: BEFORE.into(),
                        kind: AgentLogKind::Created {
                            workspace_id: child.workspace_id.clone(),
                            worktree_id: child.worktree_id.clone(),
                        },
                    })
                    .unwrap(),
                )
                .await
                .unwrap();
        } else {
            insert_agent(store, &child, BEFORE).await;
        }
        children.push(child);
    }
    (sandbox, children)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retirement_finality_is_atomic_truthful_and_rejects_delayed_close_publication() {
    for segmented in [false, true] {
        let (client, name, store) = database().await;
        let (sandbox, children) = retiring(&store, segmented).await;
        let working = &children[0];
        let history = store.agent_log(&working.id, 0, 100).await.unwrap();
        let left = store.recover_runtime_loss(finality_loss(&sandbox));
        let right = store.recover_runtime_loss(finality_loss(&sandbox));
        let (left, right) = tokio::join!(left, right);
        let outcomes = [left.unwrap(), right.unwrap()];
        assert_eq!(
            outcomes
                .iter()
                .filter(|o| **o == SandboxRuntimeLossOutcome::RecoveryScheduled)
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|o| **o == SandboxRuntimeLossOutcome::Superseded)
                .count(),
            1
        );
        let settled = store.get_sandbox(&sandbox.id).await.unwrap().unwrap();
        assert_eq!(settled.status, SandboxStatus::Retiring);
        assert_eq!(settled.work.step, SandboxWorkStep::StopAgents);
        assert!(settled.runtime.is_none());
        assert_eq!(settled.lifecycle.home, sandbox.lifecycle.home);
        assert_eq!(settled.lifecycle.checkpoints, sandbox.lifecycle.checkpoints);
        assert_eq!(settled.credential, sandbox.credential);
        assert!(settled.lifecycle.operation.is_none());
        for child in &children {
            let after = store.get_agent(&child.id).await.unwrap().unwrap();
            if child.status == AgentStatus::Stopped {
                assert_eq!(&after, child);
                continue;
            }
            assert_eq!(after.status, AgentStatus::Stopped);
            assert_eq!(after.work.step, AgentWorkStep::None);
            assert!(after.work.lease.is_none());
            assert!(after.work.cleanup_debt.is_empty());
            assert_eq!(
                after.conversation.conversation_id,
                child.conversation.conversation_id
            );
            assert_eq!(
                after.stop_reason,
                Some(
                    child
                        .requested_stop_reason
                        .clone()
                        .unwrap_or(AgentStopReason::SandboxRetired)
                )
            );
            assert!(!after
                .work
                .receipts
                .iter()
                .any(|r| r.step == AgentWorkStep::CloseProcess));
        }
        let after = store.get_agent(&working.id).await.unwrap().unwrap();
        assert_eq!(
            after.conversation.operation.unwrap().state,
            AgentOperationDelivery::RuntimeLost
        );
        assert!(after.conversation.active_turn.is_none());
        assert_eq!(
            after.conversation.queued_messages,
            working.conversation.queued_messages
        );
        let operations = layer2_core::sandbox::AgentOperationService::new(
            std::sync::Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap()),
            std::sync::Arc::new(crate::sandbox::LiveAgentRegistry::default()),
        );
        assert!(matches!(
            operations
                .dispatch_lifecycle_queued_message(&settled, &working.id, 4, OBSERVED.into())
                .await,
            Err(layer2_core::sandbox::AgentOperationError::StatusConflict {
                status: AgentStatus::Stopped
            })
        ));
        assert_eq!(after.conversation.runtime_generation, Some(2));
        assert_eq!(
            store
                .get_agent(&children[2].id)
                .await
                .unwrap()
                .unwrap()
                .conversation
                .first_message
                .unwrap()
                .state,
            FirstMessageDelivery::Failed
        );
        let logs = store.agent_log(&working.id, 0, 100).await.unwrap();
        assert_eq!(&logs[..history.len()], history);
        assert!(logs
            .iter()
            .any(|l| l.kind == AgentLogKind::TurnInterrupted { turn_sequence: 5 }));
        assert!(!logs
            .iter()
            .any(|l| matches!(l.kind, AgentLogKind::TurnCompleted { .. })));
        // The old exact work claim cannot publish its delayed close result.
        let old_lease = working.work.lease.as_ref().unwrap();
        let mut delayed = working.clone();
        delayed.revision += 1;
        delayed
            .work
            .schedule(AgentWorkStep::PublishStopped, OBSERVED.into());
        let delayed_result = store
            .commit_agent_work(AgentWorkMutation {
                expected_agent_revision: working.revision,
                holder: old_lease.holder.clone(),
                fence: old_lease.fence,
                agent: delayed,
                worktree: None,
                appended_at: OBSERVED.into(),
                sandbox_event: None,
                agent_logs: vec![],
            })
            .await;
        assert!(
            matches!(delayed_result, Err(SandboxStoreError::Conflict)),
            "{delayed_result:?}"
        );
        assert_eq!(store.agent_log(&working.id, 0, 100).await.unwrap(), logs);
        assert_eq!(
            store
                .recover_runtime_loss(finality_loss(&sandbox))
                .await
                .unwrap(),
            SandboxRuntimeLossOutcome::Superseded
        );
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retirement_finality_refuses_new_child_stale_parent_and_external_debt() {
    for segmented in [false, true] {
        for change in ["new_child", "newer_child", "parent", "external_debt"] {
            let (client, name, store) = database().await;
            let (sandbox, children) = retiring(&store, segmented).await;
            let request = finality_loss(&sandbox);
            let mut expected_parent = sandbox.clone();
            match change {
                "new_child" => {
                    let mut new_child = ready_agent(&sandbox, "agt_new_child");
                    new_child.updated_at = "2026-09-05T00:30:01Z".into();
                    insert_agent(&store, &new_child, &new_child.updated_at).await;
                }
                "parent" => {
                    expected_parent.revision += 1;
                    expected_parent.name.push_str("-new");
                    store
                        .save_sandbox(sandbox.revision, expected_parent.clone())
                        .await
                        .unwrap();
                }
                "newer_child" | "external_debt" => {
                    let mut child = children[1].clone();
                    child.revision += 1;
                    if change == "newer_child" {
                        child.updated_at = "2026-09-05T00:30:01Z".into();
                    } else {
                        child.work.cleanup_debt.push(AgentCleanupDebt {
                            step: AgentWorkStep::DispatchFirstMessage,
                            operation_id: "external-effect".into(),
                            request_sha256: "b".repeat(64),
                        });
                    }
                    store.save_agent(children[1].revision, child).await.unwrap();
                }
                _ => unreachable!(),
            }
            let before: Vec<_> =
                futures_util::future::join_all(children.iter().map(|c| store.get_agent(&c.id)))
                    .await;
            let outcome = store.recover_runtime_loss(request).await.unwrap();
            assert_eq!(
                outcome,
                if change == "parent" {
                    SandboxRuntimeLossOutcome::Superseded
                } else {
                    SandboxRuntimeLossOutcome::NotEligible
                },
                "{change}"
            );
            assert_eq!(
                store.get_sandbox(&sandbox.id).await.unwrap(),
                Some(expected_parent)
            );
            for (child, before) in children.iter().zip(before) {
                assert_eq!(store.get_agent(&child.id).await.unwrap(), before.unwrap());
            }
            client.database(&name).drop().await.unwrap();
        }
    }
}
