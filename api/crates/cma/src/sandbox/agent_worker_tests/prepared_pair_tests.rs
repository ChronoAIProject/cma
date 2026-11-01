use super::*;
use layer3_adapter::wire::RequestFrame;
mod batch_tests;

async fn prepared() -> (Harness, Agent) {
    prepared_with_prompt(crate::agent_lifecycle::guidance::managed_v2()).await
}

async fn prepared_with_prompt(prompt: &str) -> (Harness, Agent) {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let mut profile = agent_profile(vec![]);
    profile.content.system_prompt = prompt.into();
    profile.digest = layer2_core::profiles::content_digest(&profile.content).unwrap();
    let origin = AgentOrigin::Lifecycle {
        operation_id: "prepared-lifecycle".into(),
    };
    let creation_key = format!(
        "lifecycle:{}",
        layer2_core::profiles::content_digest(&origin).unwrap()
    );
    let creation = harness
        .store
        .admit_agent(AgentAdmission {
            binding: None,
            created_by: None,
            owner: harness.workspace.owner.clone(),
            workspace_id: harness.workspace.id.clone(),
            agent_id: AgentId::generate(),
            worktree_id: WorktreeId::generate(),
            profile,
            capabilities: RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            title: None,
            first_message: None,
            origin,
            creation_key,
            event_at: rfc3339_at(harness.now),
        })
        .await
        .unwrap();
    let id = creation.creation().agent.id.clone();
    // Prepare real runtime resources first. Owned replay must then execute the
    // same requests with a later resource fence, without creating duplicates.
    harness.run(&id, harness.now).await;
    harness.run(&id, harness.now).await;
    let mut agent = harness.stored(&id).await;
    let parent = harness
        .store
        .get_sandbox(&agent.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let tree = harness
        .store
        .get_worktree(&agent.worktree_id)
        .await
        .unwrap()
        .unwrap();
    let (updates, _receiver) = mpsc::channel(256);
    let channel = harness
        .adapter
        .open_agent(
            &harness.runtime,
            agent.work.next_fence,
            &layer3_adapter::AgentOpenRequest::new(
                format!("agent:{}:open:v1", agent.id),
                crate::sandbox::runtime_options(&agent, &parent, &tree),
                None,
            ),
            updates,
        )
        .await
        .unwrap();
    let revision = agent.revision;
    agent.work.warm_conversation_id = channel.conversation_id;
    agent.work.resolved_skills = Some(vec![]);
    agent.work.step = AgentWorkStep::AddWorktree;
    agent.work.receipts.clear();
    agent.work.cleanup_debt.clear();
    agent.work.next_fence += 1;
    agent.revision += 1;
    harness
        .store
        .save_agent(revision, agent.clone())
        .await
        .unwrap();
    (harness, agent)
}

async fn claim(harness: &Harness, agent: &Agent, now: SystemTime) -> Agent {
    harness
        .store
        .claim_agent_work(
            &agent.id,
            "agent-worker",
            rfc3339_at(now),
            rfc3339_at(now + Duration::from_secs(30)),
        )
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn prepared_pair_retains_each_legacy_prompt_and_conversation() {
    for prompt in ["", crate::agent_lifecycle::guidance::GITHUB_API_V1] {
        let (harness, agent) = prepared_with_prompt(prompt).await;
        let claimed = claim(&harness, &agent, harness.now).await;
        harness
            .worker
            .replay_prepared_for_test(claimed, harness.now)
            .await
            .unwrap();
        let after = harness.stored(&agent.id).await;
        assert_eq!(after.profile, agent.profile);
        assert_eq!(after.work.step, AgentWorkStep::OpenConversation);
        assert_eq!(
            after.work.warm_conversation_id,
            agent.work.warm_conversation_id
        );
    }
}

#[tokio::test]
async fn prepared_pair_replays_both_resource_fences_with_one_fresh_observation() {
    let (mut harness, agent) = prepared().await;
    let observed = super::startup_publication_tests::interleave(
        &mut harness,
        &agent,
        WireOperation::RuntimeStatus,
        super::startup_publication_tests::Interleaving::ObserveOnly,
    );
    let before = harness.backend.requests().await.len();
    let claimed = claim(&harness, &agent, harness.now).await;
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    harness
        .worker
        .replay_prepared_for_test(claimed, harness.now)
        .await
        .unwrap();
    let requests = harness.backend.requests().await;
    let replay = &requests[before..];
    assert_eq!(observed.adoptions.load(Ordering::SeqCst), 1);
    let fences: Vec<_> = replay
        .iter()
        .filter_map(|(_, r)| match r {
            RequestFrame::AddWorktree(r) => Some((WireOperation::AddWorktree, r.authority_fence)),
            RequestFrame::MaterializeAgentHome(r) => {
                Some((WireOperation::MaterializeAgentHome, r.authority_fence))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        fences,
        vec![
            (WireOperation::AddWorktree, fence),
            (WireOperation::MaterializeAgentHome, fence)
        ]
    );
    let after = harness.stored(&agent.id).await;
    assert_eq!(after.work.step, AgentWorkStep::OpenConversation);
    assert!(after.work.cleanup_debt.is_empty());
    assert_eq!(after.work.receipts.len(), 2);
    assert_eq!(
        after.work.receipts[0].completed_at,
        after.work.receipts[1].completed_at
    );
    assert_eq!(
        after.work.warm_conversation_id,
        agent.work.warm_conversation_id
    );
    let snapshot = harness.backend.snapshot(&harness.runtime).await.unwrap();
    assert_eq!(
        (
            snapshot.worktrees.len(),
            snapshot.homes.len(),
            snapshot.agents.len()
        ),
        (1, 1, 1)
    );
    for _ in 0..3 {
        harness.run(&agent.id, harness.now).await;
    }
    let ready = harness.stored(&agent.id).await;
    assert_eq!(ready.status, AgentStatus::Ready);
    assert_eq!(
        ready.conversation.conversation_id,
        agent.work.warm_conversation_id
    );
}

#[tokio::test]
async fn prepared_pair_unknown_effect_retains_both_debts_and_replays_after_reclaim() {
    for operation in [
        WireOperation::AddWorktree,
        WireOperation::MaterializeAgentHome,
    ] {
        let (harness, agent) = prepared().await;
        harness.backend.lose_next_response(operation);
        let claimed = claim(&harness, &agent, harness.now).await;
        harness
            .worker
            .replay_prepared_for_test(claimed, harness.now)
            .await
            .unwrap();
        let failed = harness.stored(&agent.id).await;
        assert_eq!(failed.work.step, AgentWorkStep::AddWorktree);
        assert_eq!(failed.work.cleanup_debt.len(), 2);
        assert!(failed.work.receipts.is_empty());
        assert!(failed.work.lease.is_none());
        let later = harness.now + Duration::from_secs(2);
        let claimed = claim(&harness, &failed, later).await;
        harness
            .worker
            .replay_prepared_for_test(claimed, later)
            .await
            .unwrap();
        let completed = harness.stored(&agent.id).await;
        assert_eq!(completed.work.step, AgentWorkStep::OpenConversation);
        assert_eq!(completed.work.receipts.len(), 2);
        assert!(completed.work.cleanup_debt.is_empty());
        let snapshot = harness.backend.snapshot(&harness.runtime).await.unwrap();
        assert_eq!((snapshot.worktrees.len(), snapshot.homes.len()), (1, 1));
    }
}

#[tokio::test]
async fn prepared_pair_debts_recover_through_ordinary_worker_when_proof_is_unavailable() {
    let (harness, agent) = prepared().await;
    harness
        .backend
        .lose_next_response(WireOperation::MaterializeAgentHome);
    let claimed = claim(&harness, &agent, harness.now).await;
    harness
        .worker
        .replay_prepared_for_test(claimed, harness.now)
        .await
        .unwrap();
    // The in-memory port defaults to false, modeling a missing preparation.
    let later = harness.now + Duration::from_secs(2);
    harness.run(&agent.id, later).await;
    let middle = harness.stored(&agent.id).await;
    assert_eq!(middle.work.step, AgentWorkStep::MaterializeHome);
    assert_eq!(middle.work.cleanup_debt.len(), 1);
    harness.run(&agent.id, later).await;
    assert_eq!(
        harness.stored(&agent.id).await.work.step,
        AgentWorkStep::OpenConversation
    );
}

#[tokio::test(start_paused = true)]
async fn prepared_pair_concurrent_first_prompt_publication_does_not_repeat_effects() {
    let (mut harness, agent) = prepared().await;
    let _backend = super::startup_publication_tests::interleave(
        &mut harness,
        &agent,
        WireOperation::MaterializeAgentHome,
        super::startup_publication_tests::Interleaving::FirstPrompt,
    );
    let before = harness.backend.requests().await.len();
    let claimed = claim(&harness, &agent, harness.now).await;
    harness
        .worker
        .replay_prepared_for_test(claimed, harness.now)
        .await
        .unwrap();
    let completed = harness.stored(&agent.id).await;
    assert_eq!(completed.work.step, AgentWorkStep::OpenConversation);
    assert_eq!(
        completed.conversation.first_message.unwrap().text,
        "Inspect the workspace"
    );
    assert_eq!(
        completed.title.as_deref(),
        Some("Owner's title during startup")
    );
    let requests = harness.backend.requests().await;
    for operation in [
        WireOperation::AddWorktree,
        WireOperation::MaterializeAgentHome,
    ] {
        assert_eq!(
            requests[before..]
                .iter()
                .filter(|(_, r)| r.operation() == operation)
                .count(),
            1
        );
    }
    assert_eq!(completed.work.receipts.len(), 2);
}

#[tokio::test]
async fn prepared_pair_refuses_stale_worker_and_stopped_parent_before_effects() {
    for pause in [false, true] {
        let (harness, agent) = prepared().await;
        let claimed = claim(&harness, &agent, harness.now).await;
        if pause {
            let mut parent = harness
                .store
                .get_sandbox(&agent.sandbox_id)
                .await
                .unwrap()
                .unwrap();
            let revision = parent.revision;
            parent.begin_pause(rfc3339_at(harness.now), None).unwrap();
            harness.store.save_sandbox(revision, parent).await.unwrap();
        } else {
            let mut current = claimed.clone();
            current.work.lease.as_mut().unwrap().holder = "successor".into();
            current.revision += 1;
            harness
                .store
                .save_agent(claimed.revision, current)
                .await
                .unwrap();
        }
        let before = harness.backend.requests().await.len();
        assert!(matches!(
            harness
                .worker
                .replay_prepared_for_test(claimed, harness.now)
                .await,
            Err(layer2_core::sandbox::SandboxStoreError::Conflict)
        ));
        let requests = harness.backend.requests().await;
        assert!(!requests[before..].iter().any(|(_, r)| matches!(
            r.operation(),
            WireOperation::AddWorktree | WireOperation::MaterializeAgentHome
        )));
        assert!(harness.stored(&agent.id).await.work.receipts.is_empty());
    }
}

#[tokio::test(start_paused = true)]
async fn prepared_pair_does_not_publish_after_stop_or_worker_takeover_during_effects() {
    use super::startup_publication_tests::{interleave, Interleaving};
    for operation in [
        WireOperation::AddWorktree,
        WireOperation::MaterializeAgentHome,
    ] {
        for action in [
            Interleaving::Stop,
            Interleaving::Holder,
            Interleaving::Fence,
            Interleaving::Expire,
        ] {
            let (mut harness, agent) = prepared().await;
            let _backend = interleave(&mut harness, &agent, operation, action);
            let claimed = claim(&harness, &agent, harness.now).await;
            assert!(matches!(
                harness
                    .worker
                    .replay_prepared_for_test(claimed, harness.now)
                    .await,
                Err(layer2_core::sandbox::SandboxStoreError::Conflict)
            ));
            let current = harness.stored(&agent.id).await;
            assert!(current.work.receipts.is_empty());
            assert_eq!(current.work.cleanup_debt.len(), 2);
            assert!(current.conversation.conversation_id.is_none());
        }
    }
}
