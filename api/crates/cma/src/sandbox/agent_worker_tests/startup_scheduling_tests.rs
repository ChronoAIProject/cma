use super::*;

#[tokio::test]
async fn startup_execution_keeps_retirement_work_out_of_the_worker_future() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let agent = harness.admit("startup-stack-budget", Vec::new()).await;
    let execution = harness.worker.execute_claimed_for_test(agent, harness.now);
    assert!(std::mem::size_of_val(&execution) < 64 * 1024);
}

#[tokio::test]
async fn fresh_agent_startup_drains_to_ready_with_each_effect_receipted_once() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let admitted = harness.admit("fresh-scheduled", vec![]).await;
    assert!(admitted.work.warm_conversation_id.is_none());
    assert!(harness
        .worker
        .run_scheduled(&admitted.id, harness.now)
        .await
        .unwrap());
    let ready = harness.stored(&admitted.id).await;
    assert_eq!(ready.status, AgentStatus::Ready);
    assert_eq!(ready.work.step, AgentWorkStep::None);
    assert!(ready.conversation.conversation_id.is_some());
    assert!(ready.work.cleanup_debt.is_empty());
    assert_eq!(
        ready
            .work
            .receipts
            .iter()
            .map(|r| r.step)
            .collect::<Vec<_>>(),
        [
            AgentWorkStep::AddWorktree,
            AgentWorkStep::MaterializeHome,
            AgentWorkStep::OpenConversation,
        ]
    );
    let snapshot = harness.backend.snapshot(&harness.runtime).await.unwrap();
    assert_eq!(snapshot.worktrees.len(), 1);
    assert_eq!(snapshot.homes.len(), 1);
}

#[tokio::test]
async fn startup_drain_stops_at_retry_deadline_and_resumes_without_repeating_worktree() {
    let skills = ScriptedSkills::new([
        Err(AgentSkillResolutionError::Unavailable("outage".into())),
        Ok(vec![]),
    ]);
    let harness = Harness::new(skills.clone()).await;
    let admitted = harness.admit("scheduled-retry", vec![]).await;
    assert!(harness
        .worker
        .run_scheduled(&admitted.id, harness.now)
        .await
        .unwrap());
    let pending = harness.stored(&admitted.id).await;
    assert_eq!(pending.status, AgentStatus::Starting);
    assert_eq!(pending.work.step, AgentWorkStep::MaterializeHome);
    assert_eq!(pending.work.attempts, 1);
    assert_eq!(skills.calls.load(Ordering::Relaxed), 1);
    assert!(pending.conversation.conversation_id.is_none());
    assert_eq!(pending.work.receipts.len(), 1);
    assert_eq!(pending.work.receipts[0].step, AgentWorkStep::AddWorktree);
    assert!(!harness
        .worker
        .run_scheduled(&admitted.id, harness.now)
        .await
        .unwrap());
    let retry_at = SystemTime::UNIX_EPOCH
        + Duration::from_secs(layer2_core::seconds_from_rfc3339(&pending.work.ready_at).unwrap());
    assert!(harness
        .worker
        .run_scheduled(&admitted.id, retry_at)
        .await
        .unwrap());
    let ready = harness.stored(&admitted.id).await;
    assert_eq!(ready.status, AgentStatus::Ready);
    assert_eq!(ready.work.step, AgentWorkStep::None);
    assert_eq!(skills.calls.load(Ordering::Relaxed), 2);
    assert_eq!(ready.work.receipts.len(), 3);
    assert_eq!(ready.work.receipts[0], pending.work.receipts[0]);
    let snapshot = harness.backend.snapshot(&harness.runtime).await.unwrap();
    assert_eq!(snapshot.worktrees.len(), 1);
    assert_eq!(snapshot.homes.len(), 1);
}
