use super::*;
use layer3_adapter::AgentOpenRequest;

#[tokio::test]
async fn prepared_native_session_reaches_ready_through_owned_home_intent() {
    let skills = ScriptedSkills::new([Ok(vec![])]);
    let harness = Harness::new(skills.clone()).await;
    let admitted = harness.admit("prepared-handoff", vec![]).await;
    harness.run(&admitted.id, harness.now).await;
    harness.run(&admitted.id, harness.now).await;
    let mut agent = harness.stored(&admitted.id).await;
    let sandbox = harness
        .store
        .get_sandbox(&agent.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let worktree = harness
        .store
        .get_worktree(&agent.worktree_id)
        .await
        .unwrap()
        .unwrap();
    let options = crate::sandbox::runtime_mapping::runtime_options(&agent, &sandbox, &worktree);
    let (updates, _receiver) = mpsc::channel(256);
    let opened = harness
        .adapter
        .open_agent(
            &harness.runtime,
            agent.work.next_fence,
            &AgentOpenRequest::new(format!("agent:{}:open:v1", agent.id), options, None),
            updates,
        )
        .await
        .unwrap();
    let conversation = opened.conversation_id.unwrap();
    let revision = agent.revision;
    agent.work.warm_conversation_id = Some(conversation.clone());
    agent.work.resolved_skills = Some(vec![]);
    agent.work.step = AgentWorkStep::MaterializeHome;
    agent
        .work
        .receipts
        .retain(|r| r.step == AgentWorkStep::AddWorktree);
    agent.revision += 1;
    skills.locators.lock().unwrap().push_back(vec![]);
    harness
        .store
        .save_agent(revision, agent.clone())
        .await
        .unwrap();

    assert!(harness
        .worker
        .run_scheduled(&agent.id, harness.now)
        .await
        .unwrap());
    let ready = harness.stored(&agent.id).await;
    assert_eq!(ready.status, AgentStatus::Ready);
    assert_eq!(ready.work.step, AgentWorkStep::None);
    assert_eq!(ready.conversation.conversation_id, Some(conversation));
    assert_eq!(ready.work.resolved_skills, Some(vec![]));
    assert_eq!(skills.calls.load(Ordering::Relaxed), 1);
    assert_eq!(
        ready
            .work
            .receipts
            .iter()
            .filter(|r| r.step == AgentWorkStep::MaterializeHome)
            .count(),
        1
    );
}
