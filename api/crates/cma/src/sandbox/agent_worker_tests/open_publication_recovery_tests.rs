//! A completed Open must survive an aborted combined readiness publication.
use super::*;
use crate::sandbox::live_agents::LiveAgentAttachment;
use layer2_core::sandbox::{AgentCleanupDebt, SandboxStoreError};
use layer3_adapter::AgentOpenRequest;

struct Opened {
    harness: Harness,
    agent: Agent,
    sandbox: Sandbox,
    request: AgentOpenRequest,
    attachment: LiveAgentAttachment,
}

impl Opened {
    async fn new() -> Self {
        let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        let admitted = harness.admit("open-publication-recovery", vec![]).await;
        for _ in 0..2 {
            harness.run(&admitted.id, harness.now).await;
        }
        let mut agent = harness
            .store
            .claim_agent_work(
                &admitted.id,
                "agent-worker",
                rfc3339_at(harness.now),
                rfc3339_at(harness.now + Duration::from_secs(18 * 60)),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(agent.work.step, AgentWorkStep::OpenConversation);
        let sandbox = harness
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
        let request = AgentOpenRequest::new(
            format!("agent:{}:open:v1", agent.id),
            crate::sandbox::runtime_options(&agent, &sandbox, &tree),
            None,
        );
        let revision = agent.revision;
        agent.work.cleanup_debt.push(AgentCleanupDebt {
            step: AgentWorkStep::OpenConversation,
            operation_id: request.operation_id.clone(),
            request_sha256: request.request_sha256.clone(),
        });
        agent.revision += 1;
        harness
            .store
            .save_agent(revision, agent.clone())
            .await
            .unwrap();
        let fence = agent.work.lease.as_ref().unwrap().fence;
        let (updates, receiver) = mpsc::channel(256);
        let channel = harness
            .adapter
            .open_agent(&harness.runtime, fence, &request, updates)
            .await
            .unwrap();
        let attachment = harness
            .registry
            .attach(
                agent.id.clone(),
                channel.conversation_id.unwrap(),
                1,
                fence,
                agent.capabilities.clone(),
                channel.input,
                receiver,
                harness.updates.clone(),
            )
            .unwrap();
        Self {
            harness,
            agent,
            sandbox,
            request,
            attachment,
        }
    }

    async fn recover(&self, now: SystemTime) -> Result<(), SandboxStoreError> {
        self.harness
            .worker
            .retain_completed_open_after_conflict(
                &self.agent,
                &self.sandbox,
                &self.request,
                &self.attachment,
                now,
            )
            .await
    }

    async fn open_count(&self) -> usize {
        self.harness
            .backend
            .requests()
            .await
            .iter()
            .filter(|(_, request)| {
                matches!(request, layer3_adapter::wire::RequestFrame::OpenAgent(_))
            })
            .count()
    }
}

#[tokio::test]
async fn aborted_prepared_snapshot_keeps_one_open_and_finishes_with_fresh_parents() {
    let opened = Opened::new().await;
    let harness = &opened.harness;
    // A parent-only update invalidates the combined publication snapshot while
    // leaving the Agent revision and its long startup lease completely unchanged.
    let mut changed = opened.sandbox.clone();
    changed.revision += 1;
    changed.name = "Renamed during startup".into();
    harness
        .store
        .save_sandbox(opened.sandbox.revision, changed.clone())
        .await
        .unwrap();
    assert_eq!(harness.stored(&opened.agent.id).await, opened.agent);
    opened.recover(harness.now).await.unwrap();
    let pending = harness.stored(&opened.agent.id).await;
    assert_eq!(pending.status, AgentStatus::Starting);
    assert_eq!(pending.work.step, AgentWorkStep::PublishReady);
    assert!(pending.work.lease.is_none());
    assert!(pending.work.cleanup_debt.is_empty());
    assert_eq!(pending.work.receipts.len(), 3);
    assert_eq!(opened.open_count().await, 1);
    assert_eq!(
        harness
            .store
            .get_sandbox(&opened.sandbox.id)
            .await
            .unwrap()
            .unwrap(),
        changed
    );
    for _ in 0..2 {
        harness.run(&opened.agent.id, harness.now).await;
    }
    let ready = harness.stored(&opened.agent.id).await;
    assert_eq!(ready.status, AgentStatus::Ready);
    assert_eq!(ready.work.step, AgentWorkStep::None);
    assert_eq!(opened.open_count().await, 1);
    assert_eq!(
        ready.conversation.conversation_id,
        Some(opened.attachment.identity().conversation_id)
    );
    assert!(harness
        .registry
        .published_target_is_attached(&Harness::target(&ready)));
}

#[tokio::test]
async fn completed_open_recovery_preserves_first_message_and_owner_title() {
    let opened = Opened::new().await;
    let harness = &opened.harness;
    let mut changed = opened.agent.clone();
    changed.revision += 1;
    changed.title = Some("Owner title".into());
    changed.conversation.first_message = Some(
        FirstMessageRecord::new(
            &changed.creation_key,
            "Run exactly once",
            &changed.updated_at,
        )
        .unwrap(),
    );
    harness
        .store
        .save_agent(opened.agent.revision, changed.clone())
        .await
        .unwrap();
    opened.recover(harness.now).await.unwrap();
    let retained = harness.stored(&opened.agent.id).await;
    assert_eq!(retained.title, changed.title);
    assert_eq!(
        retained.conversation.first_message,
        changed.conversation.first_message
    );
    assert_eq!(retained.work.step, AgentWorkStep::PublishReady);
    assert!(retained.work.lease.is_none());
    assert_eq!(opened.open_count().await, 1);
}

#[tokio::test]
async fn completed_open_recovery_cannot_use_changed_authority_or_lost_channel() {
    for variant in 0..6 {
        let opened = Opened::new().await;
        let harness = &opened.harness;
        let mut changed = opened.agent.clone();
        match variant {
            0 => changed.work.lease.as_mut().unwrap().holder = "successor".into(),
            1 => {
                changed.work.lease.as_mut().unwrap().fence += 1;
                changed.work.next_fence += 1;
            }
            2 => changed.work.cleanup_debt.clear(),
            3 => {
                changed.work.step = AgentWorkStep::FenceInteraction;
                changed.requested_stop_reason = Some(AgentStopReason::Owner);
            }
            4 => harness.registry.rollback(&opened.attachment),
            5 => {}
            _ => unreachable!(),
        }
        if changed != opened.agent {
            changed.revision += 1;
            harness
                .store
                .save_agent(opened.agent.revision, changed.clone())
                .await
                .unwrap();
        }
        let now = harness.now
            + if variant == 5 {
                Duration::from_secs(18 * 60)
            } else {
                Duration::ZERO
            };
        assert!(
            matches!(opened.recover(now).await, Err(SandboxStoreError::Conflict)),
            "variant {variant}"
        );
        assert_eq!(harness.stored(&opened.agent.id).await, changed);
        assert_eq!(opened.open_count().await, 1);
    }
}

#[tokio::test]
async fn completed_open_receipt_never_bypasses_a_paused_parents_readiness_check() {
    let opened = Opened::new().await;
    let harness = &opened.harness;
    let mut paused = opened.sandbox.clone();
    paused.begin_pause(rfc3339_at(harness.now), None).unwrap();
    harness
        .store
        .save_sandbox(opened.sandbox.revision, paused)
        .await
        .unwrap();
    opened.recover(harness.now).await.unwrap();
    harness.run(&opened.agent.id, harness.now).await;
    let waiting = harness.stored(&opened.agent.id).await;
    assert_ne!(waiting.status, AgentStatus::Ready);
    assert_eq!(waiting.work.step, AgentWorkStep::PublishReady);
    assert!(!harness
        .registry
        .published_target_is_attached(&Harness::target(&waiting)));
    assert_eq!(opened.open_count().await, 1);
}
