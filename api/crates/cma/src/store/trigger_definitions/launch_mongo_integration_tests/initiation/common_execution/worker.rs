use super::*;
use crate::trigger::{
    LaunchResourceDriver, LaunchResourceEffect, LaunchResourceError, LaunchWorker,
    LaunchWorkerConfig,
};
use layer2_core::sandbox::store::{SandboxStore, WorktreeStore};

struct Driver(Arc<Fixture>);
#[async_trait::async_trait]
impl LaunchResourceDriver for Driver {
    async fn trigger_revision(
        &self,
        id: &TriggerId,
        digest: &str,
    ) -> Result<Option<TriggerRevision>, LaunchResourceError> {
        Ok(self
            .0
            .seed
            .store
            .trigger_revision(id, digest)
            .await
            .unwrap())
    }
    async fn frozen_profiles(
        &self,
        target: &TriggerTarget,
    ) -> Result<ResolvedTriggerProfiles, LaunchResourceError> {
        Ok(self
            .0
            .seed
            .profiles
            .read_trigger_profiles(target)
            .await
            .unwrap())
    }
    async fn orchestration_paused(&self) -> Result<bool, LaunchResourceError> {
        Ok(false)
    }
    async fn requirements(
        &self,
        _: &SubjectId,
        services: &[ServiceId],
    ) -> Result<Vec<ServiceRequirement>, LaunchResourceError> {
        Ok(services
            .iter()
            .map(|id| ServiceRequirement {
                service: id.clone(),
                state: ServiceRequirementState::Satisfied,
            })
            .collect())
    }
    async fn sandbox(
        &self,
        id: &layer2_core::SandboxId,
    ) -> Result<Option<Sandbox>, LaunchResourceError> {
        Ok(self.0.resources.get_sandbox(id).await.unwrap())
    }
    async fn workspace(
        &self,
        id: &layer2_core::WorkspaceId,
    ) -> Result<Option<Workspace>, LaunchResourceError> {
        Ok(self.0.resources.get_workspace(id).await.unwrap())
    }
    async fn agent(&self, id: &layer2_core::AgentId) -> Result<Option<Agent>, LaunchResourceError> {
        Ok(self.0.resources.get_agent(id).await.unwrap())
    }
    async fn matching_workspace(
        &self,
        _: &SubjectId,
        _: &layer2_core::SandboxId,
        _: &layer2_core::WorkspaceProfileId,
    ) -> Result<Option<Workspace>, LaunchResourceError> {
        panic!("resolved workspace must not be selected again")
    }
    async fn apply(&self, effect: LaunchResourceEffect) -> Result<String, LaunchResourceError> {
        let LaunchResourceEffect::CreateAgent {
            initiation,
            request,
            reuse_existing,
            ..
        } = effect
        else {
            panic!("only the retained Agent effect is pending")
        };
        let initiation = *initiation.unwrap();
        let launch = self
            .0
            .seed
            .store
            .launch(initiation.launch_id(), &self.0.identity.caller.subject)
            .await
            .unwrap()
            .unwrap();
        let mut admission = self.0.admission(&launch).await;
        admission.first_message = request.first_message.map(|text| {
            FirstMessageRecord::new(
                admission.creation_key.clone(),
                text,
                admission.event_at.clone(),
            )
            .unwrap()
        });
        Ok(self
            .0
            .resources
            .admit_agent_with_trigger_initiator(admission, initiation, reuse_existing)
            .await
            .unwrap()
            .creation()
            .agent
            .id
            .to_string())
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn trigger_common_real_launch_worker_settles_bound_effect_and_projects_shared_completion() {
    run_launch_after_admission(false).await;
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn operator_stop_keeps_an_admitted_launch_and_common_first_message_exactly_once() {
    run_launch_after_admission(true).await;
}

async fn run_launch_after_admission(stopped: bool) {
    let (f, launch, _) = pending().await;
    if stopped {
        use layer2_core::admission::AdmissionControlStore;
        crate::store::admission_control::MongoAdmissionControlStore::open(
            &f.seed.client,
            &f.seed.database,
        )
        .await
        .unwrap()
        .change(
            0,
            &SubjectId::new("operator"),
            "contain new work",
            true,
            100,
        )
        .await
        .unwrap();
    }
    let f = Arc::new(f);
    let worker = LaunchWorker::new(
        Arc::new(f.seed.store.clone()),
        Arc::new(Driver(f.clone())),
        LaunchWorkerConfig::new("launch-worker"),
    );
    // Reclaim the prior crashed effect without waiting on wall time.
    let now = std::time::SystemTime::now() + std::time::Duration::from_secs(61);
    assert!(worker.run_one(&launch.id, now).await.unwrap());
    let durable = f
        .seed
        .store
        .launch(&launch.id, &launch.owner)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(durable.work.step, LaunchWorkStep::WaitForAgent);
    assert!(durable.work.effect.is_none());
    assert!(durable.input_binding_sha256.is_some());
    let mut agent = f
        .resources
        .get_agent(durable.resolution.agent.id().unwrap())
        .await
        .unwrap()
        .unwrap();
    let receipt = agent
        .conversation
        .first_message
        .as_ref()
        .unwrap()
        .common_binding
        .as_ref()
        .unwrap()
        .receipt()
        .clone();
    agent.status = AgentStatus::Ready;
    agent.ready_at = Some(layer2_core::now_rfc3339());
    agent.conversation.conversation_id = Some("worker-common-input".into());
    agent.conversation.runtime_generation = Some(1);
    agent.work = AgentWorkState::new(AgentWorkStep::None, layer2_core::now_rfc3339());
    agent.revision += 1;
    let mut tree = f
        .resources
        .get_worktree(&agent.worktree_id)
        .await
        .unwrap()
        .unwrap();
    tree.status = WorktreeStatus::Ready;
    tree.revision += 1;
    f.resources
        .worktrees
        .replace_one(
            doc! {"_id":tree.id.as_str()},
            crate::store::worktrees::worktree_document(&tree).unwrap(),
        )
        .await
        .unwrap();
    f.resources
        .agents
        .replace_one(
            doc! {"_id":agent.id.as_str()},
            crate::store::agents::agent_document(&agent).unwrap(),
        )
        .await
        .unwrap();
    let runtime = Runtime::new(false, false);
    let operations = AgentOperationService::new(f.resources.clone(), runtime.clone());
    let claim = f
        .resources
        .claim_next_command(CommandClaimRequest {
            agent_id: agent.id.clone(),
            holder: "common-worker".into(),
            now: layer2_core::now_rfc3339(),
            expires_at: expiry(),
        })
        .await
        .unwrap()
        .unwrap();
    operations.execute_trigger_claim(&claim).await.unwrap();
    DurableAgentUpdateRecorder::new(f.resources.clone())
        .record_update(
            &agent.id,
            1,
            &mut AgentMessageAssembly::default(),
            layer3_adapter::ConversationUpdate::TurnEnded(layer3_adapter::PromptResponse::new(
                layer3_adapter::TurnStopReason::EndTurn,
            )),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    assert!(worker
        .run_scheduled(&launch.id, now + std::time::Duration::from_secs(1))
        .await
        .unwrap());
    let durable = f
        .seed
        .store
        .launch(&launch.id, &launch.owner)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        durable.first_message,
        FirstMessageDelivery::Completed,
        "state={:?}, work={:?}, error={:?}",
        durable.state,
        durable.work.step,
        durable.failure
    );
    assert_eq!(
        durable.first_message_evidence.as_ref().unwrap().agent_id,
        agent.id
    );
    assert_eq!(runtime.sends.load(Ordering::SeqCst), 1);
    let command = f
        .resources
        .reconcile_command(&receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(command.status, AgentResponseStatus::Completed);
    assert_eq!(command.receipt, receipt);
    assert!(!worker
        .run_scheduled(&launch.id, now + std::time::Duration::from_secs(2))
        .await
        .unwrap());
    assert_eq!(runtime.sends.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.resources
            .agent_commands
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    f.seed
        .client
        .database(&f.seed.database)
        .drop()
        .await
        .unwrap();
}
