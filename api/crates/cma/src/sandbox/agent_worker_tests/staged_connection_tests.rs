use super::*;
use crate::sandbox::live_agents::{DurableAgentConnectionLossHandler, LiveAgentAttachment};

struct ObservedConnectionLoss {
    durable: Arc<DurableAgentConnectionLossHandler>,
    completed: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl LiveAgentConnectionLossHandler for ObservedConnectionLoss {
    async fn connection_lost(&self, attachment: LiveAgentAttachmentIdentity) {
        self.durable.connection_lost(attachment).await;
        self.completed.notify_one();
    }

    async fn publication_lost(&self, attachment: LiveAgentAttachmentIdentity) {
        self.durable.publication_lost(attachment).await;
    }
}

async fn publication_with_completed_eof() -> (Harness, Agent, LiveAgentAttachment) {
    let mut harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let pending = pending_publication(&harness).await;
    let agent = harness
        .store
        .claim_agent_work(
            &pending.id,
            "agent-worker",
            rfc3339_at(harness.now),
            rfc3339_at(harness.now + Duration::from_secs(30)),
        )
        .await
        .unwrap()
        .unwrap();
    let fence = agent.work.lease.as_ref().unwrap().fence;
    let registry = Arc::new(LiveAgentRegistry::default());
    let observed = Arc::new(ObservedConnectionLoss {
        durable: DurableAgentConnectionLossHandler::new(harness.store.clone()),
        completed: tokio::sync::Notify::new(),
    });
    assert!(registry.install_connection_loss_handler(observed.clone()));
    harness.registry = registry.clone();
    harness.worker = AgentWorker::new(
        AgentWorkerDependencies {
            store: harness.store.clone(),
            adapter: harness.adapter.clone(),
            skills: Arc::new(EmptyAgentSkillResolver),
            registry: registry.clone(),
            updates: harness.updates.clone(),
            operations: AgentOperationService::new(harness.store.clone(), registry.clone()),
        },
        AgentWorkerConfig::new("agent-worker", DEPLOYMENT_ID),
    );
    let target = Harness::target(&agent);
    let (input, updates, sender) = channels();
    registry
        .attach(
            agent.id.clone(),
            target.conversation_id.clone(),
            target.runtime_generation,
            fence - 1,
            agent.capabilities.clone(),
            input,
            updates,
            Arc::new(NoopRecorder),
        )
        .unwrap();
    // The stream closes while staged, before its task can observe EOF. Hold the
    // publication commit until the original durable recovery callback finishes.
    drop(sender);
    let promoted = registry.staged_attachment(&target, fence).unwrap().unwrap();
    registry.publish_and_activate(&promoted).unwrap();
    tokio::time::timeout(Duration::from_secs(1), observed.completed.notified())
        .await
        .expect("EOF recovery must finish before the publication commit");
    assert!(!registry.target_is_attached(&target));
    let after_eof = harness.stored(&agent.id).await;
    assert_eq!(after_eof.work.step, AgentWorkStep::PublishConnection);
    assert_eq!(after_eof.revision, agent.revision);
    (harness, agent, promoted)
}

#[tokio::test]
async fn eof_before_publication_commit_keeps_recovery_durable() {
    let (harness, agent, attachment) = publication_with_completed_eof().await;
    harness
        .worker
        .finish_connection_publication(agent.clone(), &attachment, None, harness.now)
        .await
        .unwrap();
    let recovering = harness.stored(&agent.id).await;
    assert_eq!(recovering.work.step, AgentWorkStep::PublishConnection);
    assert!(recovering.work.lease.is_none());
    assert!(!harness
        .registry
        .target_is_attached(&Harness::target(&recovering)));
    harness.run(&agent.id, SystemTime::now()).await;
    let ready = harness.stored(&agent.id).await;
    assert_eq!(ready.work.step, AgentWorkStep::None);
    assert_eq!(open_count(&harness).await, 2);
    assert_eq!(
        ready.conversation.conversation_id,
        agent.conversation.conversation_id
    );
    assert!(harness
        .registry
        .published_target_is_attached(&Harness::target(&ready)));
}

#[tokio::test]
async fn completed_publication_loss_cannot_reschedule_a_newer_connection() {
    let (harness, agent, old) = publication_with_completed_eof().await;
    harness
        .worker
        .finish_connection_publication(agent.clone(), &old, None, harness.now)
        .await
        .unwrap();
    harness.run(&agent.id, SystemTime::now()).await;
    let ready = harness.stored(&agent.id).await;
    let target = Harness::target(&ready);
    let replacement = harness
        .registry
        .attachment(&target, ready.work.next_fence - 1)
        .unwrap()
        .unwrap();
    harness.registry.recover_lost_publication(&old).await;
    // Also cover a newer claim completing after the local registry check but
    // before the callback's durable read.
    DurableAgentConnectionLossHandler::new(harness.store.clone())
        .publication_lost(old.identity())
        .await;
    let unchanged = harness.stored(&agent.id).await;
    assert_eq!(unchanged.revision, ready.revision);
    assert_eq!(unchanged.work.step, AgentWorkStep::None);
    assert_eq!(
        harness
            .registry
            .attachment(&target, ready.work.next_fence - 1)
            .unwrap()
            .unwrap(),
        replacement
    );
}

async fn pending_publication(harness: &Harness) -> Agent {
    let agent = harness.admit("staged-publication", vec![]).await;
    for _ in 0..4 {
        harness.run(&agent.id, harness.now).await;
    }
    let agent = harness.stored(&agent.id).await;
    assert_eq!(agent.work.step, AgentWorkStep::PublishConnection);
    agent
}

#[tokio::test]
async fn publication_after_durable_update_does_not_backdate_or_retry_the_connection() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let pending = pending_publication(&harness).await;
    let claimed = harness
        .store
        .claim_agent_work(
            &pending.id,
            "agent-worker",
            rfc3339_at(harness.now),
            rfc3339_at(harness.now + Duration::from_secs(30)),
        )
        .await
        .unwrap()
        .unwrap();
    let target = Harness::target(&claimed);
    let attachment = harness
        .registry
        .staged_attachment(&target, claimed.work.lease.as_ref().unwrap().fence)
        .unwrap()
        .unwrap();
    harness.registry.publish_and_activate(&attachment).unwrap();
    // Metadata or a replay acknowledgement can commit after the original claim.
    // Publication must retain that state and must not backdate its work record.
    let mut confirmed = claimed.clone();
    confirmed.title = Some("updated while reconnecting".into());
    confirmed.updated_at = rfc3339_at(harness.now + Duration::from_secs(5));
    confirmed.revision += 1;
    harness
        .store
        .save_agent(claimed.revision, confirmed.clone())
        .await
        .unwrap();
    harness
        .worker
        .finish_connection_publication(claimed, &attachment, None, harness.now)
        .await
        .unwrap();
    let published = harness.stored(&pending.id).await;
    assert_eq!(published.work.step, AgentWorkStep::None);
    assert!(published.work.lease.is_none());
    assert!(published.updated_at >= confirmed.updated_at);
    assert_eq!(published.title, confirmed.title);
    assert_eq!(
        published.conversation.settings,
        confirmed.conversation.settings
    );
    assert_eq!(
        published.conversation.conversation_id,
        confirmed.conversation.conversation_id
    );
    assert_eq!(published.work.receipts, confirmed.work.receipts);
    assert_eq!(open_count(&harness).await, 1);
    assert!(harness.registry.published_target_is_attached(&target));
}

async fn open_count(harness: &Harness) -> usize {
    harness
        .backend
        .requests()
        .await
        .iter()
        .filter(|(_, request)| matches!(request, layer3_adapter::wire::RequestFrame::OpenAgent(_)))
        .count()
}

#[tokio::test]
async fn initial_publication_uses_the_opened_stream_and_reconnects_after_loss() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let agent = pending_publication(&harness).await;
    assert_eq!(open_count(&harness).await, 1);
    harness.run(&agent.id, harness.now).await;
    let ready = harness.stored(&agent.id).await;
    assert_eq!(ready.work.step, AgentWorkStep::None);
    assert_eq!(open_count(&harness).await, 1);
    let target = Harness::target(&ready);
    assert!(harness.registry.published_target_is_attached(&target));
    let connection = harness
        .registry
        .attachment(&target, ready.work.next_fence - 1)
        .unwrap()
        .unwrap();
    harness
        .registry
        .connection_ended(connection.identity())
        .await;
    assert_eq!(
        harness.stored(&agent.id).await.work.step,
        AgentWorkStep::PublishConnection
    );
    harness.run(&agent.id, SystemTime::now()).await;
    assert_eq!(open_count(&harness).await, 2);
    assert!(harness.registry.published_target_is_attached(&target));
    assert_eq!(
        harness.stored(&agent.id).await.conversation.conversation_id,
        ready.conversation.conversation_id
    );
}

#[tokio::test]
async fn missing_staged_stream_reconnects_before_publication() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let agent = pending_publication(&harness).await;
    harness.registry.fence(
        &agent.id,
        agent.conversation.runtime_generation,
        agent.work.next_fence - 1,
    );
    harness.run(&agent.id, harness.now).await;
    assert_eq!(open_count(&harness).await, 2);
    let ready = harness.stored(&agent.id).await;
    assert_eq!(ready.work.step, AgentWorkStep::None);
    assert!(harness
        .registry
        .published_target_is_attached(&Harness::target(&ready)));
}

#[tokio::test]
async fn staged_stream_does_not_bypass_durable_open_receipt() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let mut agent = pending_publication(&harness).await;
    let revision = agent.revision;
    agent
        .work
        .receipts
        .iter_mut()
        .find(|receipt| receipt.step == AgentWorkStep::OpenConversation)
        .unwrap()
        .request_sha256 = "c".repeat(64);
    agent.revision += 1;
    harness
        .store
        .save_agent(revision, agent.clone())
        .await
        .unwrap();
    harness.run(&agent.id, harness.now).await;
    assert_eq!(open_count(&harness).await, 1);
    assert!(!harness
        .registry
        .published_target_is_attached(&Harness::target(&agent)));
    assert_ne!(
        harness.stored(&agent.id).await.work.step,
        AgentWorkStep::None
    );
}

#[tokio::test]
async fn promoted_staged_stream_reports_eof_with_its_publication_authority() {
    let registry = Arc::new(LiveAgentRegistry::default());
    let probe = Arc::new(ConnectionLossProbe::default());
    registry.install_connection_loss_handler(probe.clone());
    let id = AgentId::generate();
    let (input, updates, sender) = channels();
    let original = registry
        .attach(
            id.clone(),
            "retained".into(),
            7,
            3,
            RuntimeCapabilities::default(),
            input,
            updates,
            Arc::new(NoopRecorder),
        )
        .unwrap();
    let target = LiveAgentTarget {
        agent_id: id,
        conversation_id: "retained".into(),
        runtime_generation: 7,
    };
    assert!(registry.staged_attachment(&target, 2).is_err());
    assert!(registry
        .staged_attachment(
            &LiveAgentTarget {
                conversation_id: "different".into(),
                ..target.clone()
            },
            4
        )
        .is_err());
    let promoted = registry.staged_attachment(&target, 5).unwrap().unwrap();
    assert_eq!(promoted.identity().nonce, original.identity().nonce);
    registry.publish_and_activate(&promoted).unwrap();
    assert!(registry.staged_attachment(&target, 6).unwrap().is_none());
    assert!(registry.publish_and_activate(&original).is_err());
    drop(sender);
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if !probe.seen.lock().unwrap().is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("promoted stream EOF must be observed");
    assert_eq!(*probe.seen.lock().unwrap(), vec![promoted.identity()]);
    assert!(!registry.published_target_is_attached(&target));
}
