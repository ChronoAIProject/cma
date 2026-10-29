use super::*;

#[tokio::test]
async fn agent_control_stop_schedules_every_live_status_without_publishing_terminal_state() {
    for status in [
        AgentStatus::Starting,
        AgentStatus::Ready,
        AgentStatus::Working,
        AgentStatus::WaitingApproval,
        AgentStatus::Paused,
    ] {
        let store = ready_store(1).await;
        let creation = created(store.admit_agent(admission(status.name())).await.unwrap());
        let mut original = creation.agent;
        original.status = status;
        original.work.schedule(AgentWorkStep::None, NOW.into());
        replace_agent(&store, original.clone());

        let result = store
            .schedule_agent_stop(stop_request(&original, AgentStopReason::Owner))
            .await
            .unwrap();
        let AgentControlResult::Scheduled(scheduled) = result else {
            panic!("first stop request was not scheduled");
        };
        assert_eq!(scheduled.status, status);
        assert_eq!(
            scheduled.requested_stop_reason,
            Some(AgentStopReason::Owner)
        );
        assert_eq!(scheduled.stop_reason, None);
        assert_eq!(scheduled.stopped_at, None);
        assert_eq!(scheduled.deleted_at, None);
        assert_eq!(scheduled.work.step, AgentWorkStep::FenceInteraction);
        assert_eq!(scheduled.work.ready_at, MINUTE);
        assert_eq!(scheduled.updated_at, MINUTE);
        assert_eq!(scheduled.revision, original.revision + 1);
    }
}

#[tokio::test]
async fn agent_control_stop_replays_only_during_active_stop_work_and_rejects_changed_reason() {
    let store = ready_store(1).await;
    let creation = created(store.admit_agent(admission("stop-replay")).await.unwrap());
    let request = stop_request(&creation.agent, AgentStopReason::Owner);
    let scheduled = store
        .schedule_agent_stop(request.clone())
        .await
        .unwrap()
        .into_agent();

    assert_eq!(
        store.schedule_agent_stop(request.clone()).await.unwrap(),
        AgentControlResult::Existing(scheduled.clone())
    );
    let mut changed = request;
    changed.reason = AgentStopReason::Operator;
    assert_eq!(
        store.schedule_agent_stop(changed).await.unwrap_err(),
        AgentControlStoreError::StopReasonConflict
    );

    for status in [
        AgentStatus::Stopped,
        AgentStatus::Failed,
        AgentStatus::Deleted,
    ] {
        let completed = terminal_agent(scheduled.clone(), status);
        replace_agent(&store, completed.clone());
        assert_eq!(
            store
                .schedule_agent_stop(stop_request(&completed, AgentStopReason::Owner))
                .await
                .unwrap_err(),
            AgentControlStoreError::StatusConflict { status }
        );
    }
}

#[tokio::test]
async fn agent_control_stop_compatibility_is_explicit_for_every_work_step() {
    let store = ready_store(1).await;
    let creation = created(store.admit_agent(admission("stop-work")).await.unwrap());
    for step in all_work_steps() {
        let mut agent = creation.agent.clone();
        agent.work.schedule(step, NOW.into());
        replace_agent(&store, agent.clone());
        let result = store
            .schedule_agent_stop(stop_request(&agent, AgentStopReason::Owner))
            .await;
        let can_stop = matches!(
            step,
            AgentWorkStep::AddWorktree
                | AgentWorkStep::MaterializeHome
                | AgentWorkStep::OpenConversation
                | AgentWorkStep::PublishReady
                | AgentWorkStep::PublishConnection
                | AgentWorkStep::DispatchFirstMessage
                | AgentWorkStep::None
        );
        if can_stop {
            assert!(
                matches!(result, Ok(AgentControlResult::Scheduled(_))),
                "{step:?}"
            );
        } else {
            assert_eq!(
                result.unwrap_err(),
                AgentControlStoreError::WorkConflict { step }
            );
        }
    }

    for step in [
        AgentWorkStep::FenceInteraction,
        AgentWorkStep::CloseProcess,
        AgentWorkStep::PublishStopped,
    ] {
        let mut agent = creation.agent.clone();
        agent.requested_stop_reason = Some(AgentStopReason::Owner);
        agent.work.schedule(step, NOW.into());
        replace_agent(&store, agent.clone());
        assert_eq!(
            store
                .schedule_agent_stop(stop_request(&agent, AgentStopReason::Owner))
                .await
                .unwrap(),
            AgentControlResult::Existing(agent)
        );
    }
}

#[tokio::test]
async fn agent_control_stop_clears_authority_and_fences_the_prior_worker_commit() {
    let store = ready_store(1).await;
    let creation = created(store.admit_agent(admission("stop-fence")).await.unwrap());
    let claimed = store
        .claim_agent_work(
            &creation.agent.id,
            "old-worker",
            NOW.into(),
            TWO_MINUTES.into(),
        )
        .await
        .unwrap()
        .unwrap();
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    let scheduled = store
        .schedule_agent_stop(stop_request(&claimed, AgentStopReason::Owner))
        .await
        .unwrap()
        .into_agent();
    assert!(scheduled.work.lease.is_none());
    assert_eq!(scheduled.work.ready_at, MINUTE);
    assert_eq!(
        scheduled.work.cleanup_not_before.as_deref(),
        Some(TWO_MINUTES)
    );

    let mut stale = claimed.clone();
    stale
        .work
        .advance(
            "old-worker",
            fence,
            AgentWorkStep::MaterializeHome,
            MINUTE.into(),
        )
        .unwrap();
    stale.revision += 1;
    assert_eq!(
        store
            .commit_agent_work(AgentWorkMutation {
                expected_agent_revision: claimed.revision,
                holder: "old-worker".into(),
                fence,
                agent: stale,
                worktree: None,
                appended_at: MINUTE.into(),
                sandbox_event: None,
                agent_logs: vec![],
            })
            .await
            .unwrap_err(),
        SandboxStoreError::Conflict
    );
    assert_eq!(
        store.get_agent(&scheduled.id).await.unwrap(),
        Some(scheduled)
    );
}

#[tokio::test]
async fn agent_control_starting_agent_can_publish_stopped_without_a_conversation() {
    let store = ready_store(1).await;
    let creation = created(
        store
            .admit_agent(admission("stop-before-conversation"))
            .await
            .unwrap(),
    );
    let scheduled = store
        .schedule_agent_stop(stop_request(&creation.agent, AgentStopReason::Owner))
        .await
        .unwrap()
        .into_agent();
    let mut claimed = store
        .claim_agent_work(
            &scheduled.id,
            "stop-worker",
            MINUTE.into(),
            TWO_MINUTES.into(),
        )
        .await
        .unwrap()
        .unwrap();
    claimed.work.step = AgentWorkStep::PublishStopped;
    replace_agent(&store, claimed.clone());
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    let mut stopped = claimed.clone();
    stopped
        .work
        .advance(
            "stop-worker",
            fence,
            AgentWorkStep::None,
            TWO_MINUTES.into(),
        )
        .unwrap();
    stopped
        .transition(
            AgentStatus::Stopped,
            TWO_MINUTES.into(),
            Some(AgentStopReason::Owner),
        )
        .unwrap();
    stopped.conversation.last_seq += 1;
    let mut failed_worktree = creation.worktree;
    let expected_worktree_revision = failed_worktree.revision;
    failed_worktree.transition(WorktreeStatus::Failed).unwrap();
    store
        .commit_agent_work(AgentWorkMutation {
            expected_agent_revision: claimed.revision,
            holder: "stop-worker".into(),
            fence,
            agent: stopped.clone(),
            worktree: Some(VersionedWorktree {
                expected_revision: expected_worktree_revision,
                worktree: failed_worktree,
            }),
            appended_at: TWO_MINUTES.into(),
            sandbox_event: Some(SandboxEventKind::AgentStatusChanged {
                agent_id: stopped.id.clone(),
                from: AgentStatus::Starting,
                to: AgentStatus::Stopped,
            }),
            agent_logs: vec![AgentLogKind::Stopped {
                reason: AgentStopReason::Owner.as_str().into(),
            }],
        })
        .await
        .unwrap();
    assert_eq!(stopped.conversation.conversation_id, None);
    assert_eq!(store.get_agent(&stopped.id).await.unwrap(), Some(stopped));
}

#[tokio::test]
async fn agent_control_worker_must_publish_the_exact_requested_stop_reason() {
    let store = ready_store(1).await;
    let creation = created(
        store
            .admit_agent(admission("stop-reason-fence"))
            .await
            .unwrap(),
    );
    let mut ready = creation.agent;
    ready.status = AgentStatus::Ready;
    ready.conversation.conversation_id = Some("conversation-control".into());
    ready.conversation.runtime_generation = Some(1);
    ready.ready_at = Some(NOW.into());
    ready.work.schedule(AgentWorkStep::None, NOW.into());
    replace_worktree_status(&store, &ready, WorktreeStatus::Ready);
    replace_agent(&store, ready.clone());
    let scheduled = store
        .schedule_agent_stop(stop_request(&ready, AgentStopReason::Owner))
        .await
        .unwrap()
        .into_agent();
    let mut claimed = store
        .claim_agent_work(
            &scheduled.id,
            "stop-worker",
            MINUTE.into(),
            TWO_MINUTES.into(),
        )
        .await
        .unwrap()
        .unwrap();
    claimed.work.step = AgentWorkStep::PublishStopped;
    replace_agent(&store, claimed.clone());
    let fence = claimed.work.lease.as_ref().unwrap().fence;

    let mut wrong = claimed.clone();
    wrong
        .work
        .advance(
            "stop-worker",
            fence,
            AgentWorkStep::None,
            TWO_MINUTES.into(),
        )
        .unwrap();
    wrong.status = AgentStatus::Stopped;
    wrong.requested_stop_reason = None;
    wrong.stop_reason = Some(AgentStopReason::Operator);
    wrong.stopped_at = Some(TWO_MINUTES.into());
    wrong.revision += 1;
    assert_eq!(
        store
            .commit_agent_work(AgentWorkMutation {
                expected_agent_revision: claimed.revision,
                holder: "stop-worker".into(),
                fence,
                agent: wrong,
                worktree: None,
                appended_at: TWO_MINUTES.into(),
                sandbox_event: None,
                agent_logs: vec![],
            })
            .await
            .unwrap_err(),
        SandboxStoreError::Conflict
    );

    let mut published = claimed.clone();
    published
        .work
        .advance(
            "stop-worker",
            fence,
            AgentWorkStep::None,
            TWO_MINUTES.into(),
        )
        .unwrap();
    published
        .transition(
            AgentStatus::Stopped,
            TWO_MINUTES.into(),
            Some(AgentStopReason::Owner),
        )
        .unwrap();
    published.conversation.last_seq += 1;
    store
        .commit_agent_work(AgentWorkMutation {
            expected_agent_revision: claimed.revision,
            holder: "stop-worker".into(),
            fence,
            agent: published.clone(),
            worktree: None,
            appended_at: TWO_MINUTES.into(),
            sandbox_event: Some(SandboxEventKind::AgentStatusChanged {
                agent_id: published.id.clone(),
                from: AgentStatus::Ready,
                to: AgentStatus::Stopped,
            }),
            agent_logs: vec![AgentLogKind::Stopped {
                reason: AgentStopReason::Owner.as_str().into(),
            }],
        })
        .await
        .unwrap();
    assert_eq!(
        store.get_agent(&published.id).await.unwrap(),
        Some(published)
    );
}

#[tokio::test]
async fn published_process_stop_skips_only_connection_leases_and_preserves_writer_drain() {
    use crate::sandbox::{AgentCleanupDebt, AgentWorkLease, AgentWorkReceipt};
    for step in [
        AgentWorkStep::PublishReady,
        AgentWorkStep::PublishConnection,
        AgentWorkStep::DispatchFirstMessage,
        AgentWorkStep::None,
        AgentWorkStep::AddWorktree,
        AgentWorkStep::MaterializeHome,
        AgentWorkStep::OpenConversation,
        AgentWorkStep::InstallSkill,
    ] {
        for has_debt in [false, true] {
            let store = ready_store(1).await;
            let mut agent = created(
                store
                    .admit_agent(admission("stop-published"))
                    .await
                    .unwrap(),
            )
            .agent;
            agent.status = AgentStatus::Ready;
            agent.conversation.conversation_id = Some("existing-conversation".into());
            agent.conversation.runtime_generation = Some(1);
            agent.work.step = step;
            agent.work.lease = Some(AgentWorkLease {
                holder: "previous".into(),
                fence: 1,
                expires_at: TWO_MINUTES.into(),
            });
            agent.work.receipts = vec![AgentWorkReceipt {
                step: AgentWorkStep::OpenConversation,
                operation_id: "completed-open".into(),
                request_sha256: "a".repeat(64),
                completed_at: NOW.into(),
            }];
            if has_debt {
                agent.work.cleanup_debt.push(AgentCleanupDebt {
                    step: AgentWorkStep::MaterializeHome,
                    operation_id: "incomplete-home".into(),
                    request_sha256: "b".repeat(64),
                });
            }
            replace_agent(&store, agent.clone());
            let stopped = store
                .schedule_agent_stop(stop_request(&agent, AgentStopReason::Owner))
                .await
                .unwrap()
                .into_agent();
            let safe = !has_debt
                && matches!(
                    step,
                    AgentWorkStep::PublishReady
                        | AgentWorkStep::PublishConnection
                        | AgentWorkStep::DispatchFirstMessage
                        | AgentWorkStep::None
                );
            assert_eq!(
                stopped.work.cleanup_not_before.as_deref(),
                if safe { None } else { Some(TWO_MINUTES) },
                "{step:?}, debt={has_debt}"
            );
            assert_eq!(stopped.work.ready_at, MINUTE);
            assert!(stopped.work.lease.is_none());
        }
    }
}
