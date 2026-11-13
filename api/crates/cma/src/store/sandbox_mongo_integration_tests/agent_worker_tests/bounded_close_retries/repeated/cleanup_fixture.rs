use super::*;

fn advance(agent: &Agent, step: AgentWorkStep) -> AgentWorkMutation {
    let mut result = retry(agent);
    result.agent = agent.clone();
    result
        .agent
        .work
        .advance("worker", result.fence, step, MINUTE.into())
        .unwrap();
    result.agent.revision += 1;
    result.agent.updated_at = MINUTE.into();
    result.agent.conversation.last_seq += 1;
    result.agent_logs = vec![AgentLogKind::WorkStepCompleted {
        step: agent.work.step,
    }];
    result
}

pub(super) async fn position(f: &mut Fixture, step: AgentWorkStep) {
    if step == AgentWorkStep::CloseProcess {
        return;
    }
    let mut closed = advance(&f.agent, AgentWorkStep::PublishStopped);
    closed.agent.work.receipts.push(AgentWorkReceipt {
        step: AgentWorkStep::CloseProcess,
        operation_id: format!("agent:{}:close:v1", f.agent.id),
        request_sha256: "b".repeat(64),
        completed_at: MINUTE.into(),
    });
    f.store.commit_agent_work(closed).await.unwrap();
    let current = claim(&f.store, &f.agent.id, MINUTE).await;
    let mut stopped = advance(&current, AgentWorkStep::None);
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
        agent_id: f.agent.id.clone(),
        from: current.status,
        to: AgentStatus::Stopped,
    });
    f.store.commit_agent_work(stopped.clone()).await.unwrap();
    f.store
        .schedule_agent_delete(AgentDeleteRequest {
            target: AgentControlTarget::from(&stopped.agent),
            requested_at: MINUTE.into(),
        })
        .await
        .unwrap();
    f.agent = claim(&f.store, &f.agent.id, MINUTE).await;
    if step == AgentWorkStep::EnsureProcessClosed {
        return;
    }
    let mut removing = advance(&f.agent, AgentWorkStep::RemoveHome);
    let mut worktree = f
        .store
        .get_worktree(&f.agent.worktree_id)
        .await
        .unwrap()
        .unwrap();
    let revision = worktree.revision;
    worktree.transition(WorktreeStatus::Removing).unwrap();
    removing.worktree = Some(VersionedWorktree {
        expected_revision: revision,
        worktree,
    });
    f.store.commit_agent_work(removing).await.unwrap();
    f.agent = claim(&f.store, &f.agent.id, MINUTE).await;
    if step == AgentWorkStep::RemoveHome {
        return;
    }
    let mut removed = advance(&f.agent, AgentWorkStep::RemoveWorktree);
    removed.agent.work.receipts.push(AgentWorkReceipt {
        step: AgentWorkStep::RemoveHome,
        operation_id: format!("agent:{}:home:remove:v1", f.agent.id),
        request_sha256: "b".repeat(64),
        completed_at: MINUTE.into(),
    });
    f.store.commit_agent_work(removed).await.unwrap();
    f.agent = claim(&f.store, &f.agent.id, MINUTE).await;
}
