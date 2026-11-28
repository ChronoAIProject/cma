use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::active_sandbox;
use layer2_core::sandbox::{
    AgentOperationService, AgentWorkAuthority, FirstMessageDelivery, FirstMessageDispatchOutcome,
    FirstMessagePurpose, FirstMessageRecord,
};
use layer2_core::{RuntimeCapabilities, RuntimeCapability};
use layer3_adapter::{ConversationInput, ConversationInputSender, PromptResponse, TurnStopReason};

struct Fixture {
    client: Client,
    database: String,
    store: Arc<MongoSandboxStore>,
    agent: Agent,
    source: AdmittedAgentCommand,
    worker: Arc<AgentCommandWorker>,
    execution: Arc<crate::commands::ResourceCommandExecution>,
    service: Arc<AgentCommandService>,
    owner: CallerContext,
    sends: Arc<std::sync::atomic::AtomicUsize>,
    effects: Arc<Mutex<Vec<ConversationInput>>>,
}

impl Fixture {
    async fn new(purpose: FirstMessagePurpose, desktop: bool, lose_ack: bool) -> Self {
        let (client, database) = database("onboarding_first_message").await;
        crate::store::migrations::prepare_bindings_for_test(&client.database(&database)).await;
        let store = MongoSandboxStore::open(&client, &database)
            .await
            .unwrap()
            .with_onboarding_command_admission(true);
        let sandbox = active_sandbox("sbx_onboarding");
        let workspace = ready_workspace(&sandbox, "ws_onboarding", "project");
        store.create_sandbox(sandbox).await.unwrap();
        store.create_workspace(workspace.clone()).await.unwrap();
        let mut request = agent_admission(&workspace, "", "onboarding");
        request.event_at = layer2_core::now_rfc3339();
        request.created_by = Some(ResourceActor::Human {
            subject: workspace.owner.clone(),
        });
        request.origin = AgentOrigin::Lifecycle {
            operation_id: "onboarding-operation".into(),
        };
        request.creation_key = format!(
            "lifecycle:{}",
            layer2_core::profiles::content_digest(&request.origin).unwrap()
        );
        request.capabilities = RuntimeCapabilities::new([
            RuntimeCapability::Conversation,
            RuntimeCapability::Interrupt,
        ]);
        let mut first = FirstMessageRecord::new(
            request.creation_key.clone(),
            "Inspect the project.",
            request.event_at.clone(),
        )
        .unwrap();
        first.purpose = Some(purpose);
        first.desktop = desktop;
        request.first_message = Some(first);
        let mut retry = request.clone();
        retry.agent_id = AgentId::generate();
        retry.worktree_id = WorktreeId::generate();
        let (one, two) = tokio::join!(store.admit_agent(request.clone()), store.admit_agent(retry));
        let creation = one.unwrap().creation().clone();
        assert_eq!(two.unwrap().creation(), &creation);
        let binding = creation
            .agent
            .conversation
            .first_message
            .as_ref()
            .unwrap()
            .common_binding
            .as_ref()
            .unwrap();
        assert!(binding.is_onboarding());
        assert_eq!(
            binding.receipt().envelope.actor,
            request.created_by.unwrap()
        );

        // Reopening with the default switch proves durable selection wins.
        let store = Arc::new(MongoSandboxStore::open(&client, &database).await.unwrap());
        let source = store
            .get_command(&binding.receipt().envelope.command_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(source.receipt, *binding.receipt());
        assert_eq!(source.receipt.agent_order, 1);
        assert_eq!(
            store.agent_commands.count_documents(doc! {}).await.unwrap(),
            1
        );
        let now = layer2_core::now_rfc3339();
        let mut agent = creation.agent;
        let mut worktree = creation.worktree;
        worktree.status = WorktreeStatus::Ready;
        worktree.revision += 1;
        store
            .worktrees
            .replace_one(
                doc! { "_id": worktree.id.as_str() },
                crate::store::worktrees::worktree_document(&worktree).unwrap(),
            )
            .await
            .unwrap();
        agent.status = AgentStatus::Ready;
        agent.ready_at = Some(now.clone());
        agent.conversation.conversation_id = Some("onboarding-conversation".into());
        agent.conversation.runtime_generation = Some(1);
        agent.work = AgentWorkState::new(AgentWorkStep::DispatchFirstMessage, now.clone());
        let expiry = layer2_core::rfc3339_at(
            std::time::SystemTime::now() + std::time::Duration::from_secs(60),
        );
        agent.work.claim("startup".into(), &now, expiry).unwrap();
        agent.revision += 1;
        agent.updated_at = now.clone();
        store
            .agents
            .update_one(
                doc! { "_id": agent.id.as_str() },
                doc! { "$set": crate::store::agents::agent_replacement_fields(&agent).unwrap() },
            )
            .await
            .unwrap();
        let effects = Arc::new(Mutex::new(Vec::new()));
        let received = effects.clone();
        let input = ConversationInputSender::spawn(4, move |input| {
            let lost = lose_ack && matches!(input, ConversationInput::RecoverablePrompt { .. });
            received.lock().unwrap().push(input);
            async move {
                if lost {
                    Err(layer3_adapter::AdapterError::Rejected(
                        "the native writer lost its receipt".into(),
                    ))
                } else {
                    Ok(())
                }
            }
        });
        let (execution, sends) =
            super::super::execution_tests::fixture_interruptible_command_execution(
                &store, &agent, input,
            );
        let operations: &AgentOperationService = &execution.operations;
        assert!(matches!(
            operations
                .dispatch_queued_first_message(
                    &agent.id,
                    AgentWorkAuthority::new("startup".into(), 1).unwrap(),
                )
                .await
                .unwrap(),
            FirstMessageDispatchOutcome::CommonCommand(_)
        ));
        assert_eq!(sends.load(Ordering::SeqCst), 0);
        agent.work = AgentWorkState::new(AgentWorkStep::None, now);
        agent.revision += 1;
        store
            .agents
            .update_one(
                doc! { "_id": agent.id.as_str() },
                doc! { "$set": crate::store::agents::agent_replacement_fields(&agent).unwrap() },
            )
            .await
            .unwrap();
        let worker =
            AgentCommandWorker::new(store.clone(), execution.clone(), "onboarding-worker".into())
                .unwrap();
        let access = Arc::new(ResourceAccessService::new(
            store.clone(),
            Arc::new(Authority),
        ));
        let service = AgentCommandService::new(store.clone(), access, Arc::new(Authority));
        let owner = owner_context(agent.owner.clone());
        Self {
            client,
            database,
            store,
            agent,
            source,
            worker,
            execution,
            service,
            owner,
            sends,
            effects,
        }
    }

    async fn start(&self, assembly: &mut AgentMessageAssembly) {
        let id = &self.source.receipt.envelope.command_id;
        self.worker.run_one(id).await.unwrap();
        self.worker.run_one(id).await.unwrap();
        assert_eq!(self.sends.load(Ordering::SeqCst), 1);
        self.update(
            assembly,
            ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(ContentChunk::new(
                ContentBlock::from("First reply"),
            ))),
        )
        .await;
    }

    async fn update(&self, assembly: &mut AgentMessageAssembly, update: ConversationUpdate) {
        DurableAgentUpdateRecorder::new(self.store.clone())
            .record_update(
                &self.agent.id,
                1,
                assembly,
                update,
                layer2_core::now_rfc3339(),
            )
            .await
            .unwrap();
        self.store
            .reconcile_command(&self.source.receipt.envelope.command_id)
            .await
            .unwrap();
    }

    async fn observe(&self) -> ResponseControlObservation {
        self.service
            .observe_response_controls(
                &self.owner,
                Some("fixture"),
                &self.agent.id,
                self.source.receipt.response_id.as_ref().unwrap(),
            )
            .await
            .unwrap()
    }

    async fn close(self) {
        self.client.database(&self.database).drop().await.unwrap();
    }
}

fn owner_context(subject: SubjectId) -> CallerContext {
    CallerContext {
        credential_source: CredentialSource::Retained,
        caller: Caller::new(
            subject.clone(),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        credential: CredentialContext {
            issuer: "https://identity.test".into(),
            subject: subject.clone(),
            credential_id: "fixture".into(),
        },
        actor: ResourceActor::Human { subject },
        memberships: vec![],
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn onboarding_first_message_completes_with_its_original_receipt() {
    for (purpose, desktop) in [
        (FirstMessagePurpose::Task, false),
        (FirstMessagePurpose::Setup, true),
    ] {
        let f = Fixture::new(purpose, desktop, false).await;
        let mut assembly = AgentMessageAssembly::default();
        f.start(&mut assembly).await;
        assert!(f.observe().await.target_active);
        f.update(
            &mut assembly,
            ConversationUpdate::TurnEnded(PromptResponse::new(TurnStopReason::EndTurn)),
        )
        .await;
        let observed = f.observe().await;
        assert_eq!(
            observed.response.receipt,
            CommandStatusView::from_record(&f.source).receipt
        );
        assert_eq!(observed.response.status, AgentResponseStatus::Completed);
        assert!(!observed.target_active);
        let agent = f.store.get_agent(&f.agent.id).await.unwrap().unwrap();
        let first = agent.conversation.first_message.unwrap();
        assert_eq!(first.state, FirstMessageDelivery::Completed);
        assert_eq!(first.purpose, Some(purpose));
        assert_eq!(first.desktop, desktop);
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn onboarding_first_message_stop_addresses_only_the_bound_turn() {
    for (purpose, desktop) in [
        (FirstMessagePurpose::Setup, false),
        (FirstMessagePurpose::Task, true),
    ] {
        let f = Fixture::new(purpose, desktop, false).await;
        let mut assembly = AgentMessageAssembly::default();
        f.start(&mut assembly).await;
        let current = f.store.get_agent(&f.agent.id).await.unwrap().unwrap();
        let parent = f
            .store
            .get_sandbox(&current.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        let successor = super::super::record(
            f.store
                .reserve_command(super::super::work_tests::live_request(
                    &current,
                    &parent,
                    "successor",
                ))
                .await
                .unwrap(),
        );
        let observed = f.observe().await;
        assert!(observed.target_active && observed.runtime_support.cancel);
        let target = observed.target.unwrap();
        assert_eq!(
            target.response_id,
            *f.source.receipt.response_id.as_ref().unwrap()
        );
        assert_eq!(Some(target.turn_sequence), current.conversation.active_turn);
        let request = TargetedControlSubmission {
            agent_id: f.agent.id.clone(),
            target: target.clone(),
            expected_agent_revision: observed.agent_revision,
            command: AgentCommand::Interrupt,
            idempotency_key: "first-message-stop".into(),
        };
        let mut stale = request.clone();
        stale.expected_agent_revision -= 1;
        assert!(f
            .service
            .admit_response_control(&f.owner, "fixture", CommandSubmissionSource::Api, stale)
            .await
            .is_err());
        assert!(f
            .service
            .admit_response_control(
                &owner_context(SubjectId("other-owner".into())),
                "fixture",
                CommandSubmissionSource::Api,
                request.clone(),
            )
            .await
            .is_err());
        let control = f
            .service
            .admit_response_control(
                &f.owner,
                "fixture",
                CommandSubmissionSource::Api,
                request.clone(),
            )
            .await
            .unwrap();
        assert_eq!(control.target, target);
        f.worker
            .run_one(&control.control.receipt.command_id)
            .await
            .unwrap();
        assert_eq!(f.sends.load(Ordering::SeqCst), 2);
        assert!(matches!(
            f.effects.lock().unwrap().as_slice(),
            [
                ConversationInput::RecoverablePrompt { .. },
                ConversationInput::Cancel
            ]
        ));
        f.update(
            &mut assembly,
            ConversationUpdate::TurnEnded(PromptResponse::new(TurnStopReason::Cancelled)),
        )
        .await;
        let final_response = f.observe().await;
        assert!(!final_response.target_active);
        assert_eq!(
            final_response.response.status,
            AgentResponseStatus::Cancelled
        );
        let mut late = request;
        late.expected_agent_revision = final_response.agent_revision;
        late.idempotency_key = "late-first-message-stop".into();
        assert!(f
            .service
            .admit_response_control(&f.owner, "fixture", CommandSubmissionSource::Api, late)
            .await
            .is_err());
        assert_eq!(
            f.store
                .get_command(&successor.receipt.envelope.command_id)
                .await
                .unwrap(),
            Some(successor)
        );
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn onboarding_first_message_reconciles_lost_native_receipt_across_workers() {
    let f = Fixture::new(FirstMessagePurpose::Task, false, true).await;
    let id = &f.source.receipt.envelope.command_id;
    f.worker.run_one(id).await.unwrap();
    let old = f.store.get_command(id).await.unwrap().unwrap();
    assert_eq!(old.receipt, f.source.receipt);
    assert!(old.work.dispatch.is_some());
    let replacement = AgentCommandWorker::new(
        f.store.clone(),
        f.execution.clone(),
        "replacement-worker".into(),
    )
    .unwrap();
    replacement.run_one(id).await.unwrap();
    let mut assembly = AgentMessageAssembly::default();
    f.update(
        &mut assembly,
        ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(ContentChunk::new(
            ContentBlock::from("Reply after the lost acknowledgement"),
        ))),
    )
    .await;
    f.update(
        &mut assembly,
        ConversationUpdate::TurnEnded(PromptResponse::new(TurnStopReason::EndTurn)),
    )
    .await;
    replacement.run_one(id).await.unwrap();
    let result = f.store.get_command(id).await.unwrap().unwrap();
    assert_eq!(result.receipt, f.source.receipt);
    assert_eq!(result.status, AgentResponseStatus::Completed);
    assert_eq!(f.sends.load(Ordering::SeqCst), 1);
    f.close().await;
}
