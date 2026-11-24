use super::*;
use layer3_adapter::ConversationInputSender;

#[path = "targeted_control_budget_tests.rs"]
mod budget;
#[path = "cancel_admission_wake_tests.rs"]
mod cancel_wake;
#[path = "targeted_control_commit_tests.rs"]
mod commit;
#[path = "targeted_control_public_tests.rs"]
mod public;

struct Runtime(ConversationInputSender);
#[async_trait]
impl AgentRuntimeRegistry for Runtime {
    async fn resolve(
        &self,
        _: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        Ok(AgentRuntimeConnection {
            capabilities: capabilities(),
            input: self.0.clone(),
        })
    }
    async fn fence(&self, _: &AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        Ok(())
    }
}

fn capabilities() -> RuntimeCapabilities {
    RuntimeCapabilities::new([
        RuntimeCapability::Conversation,
        RuntimeCapability::Interrupt,
        RuntimeCapability::Steer,
    ])
}

async fn fixture() -> (
    Arc<InMemorySandboxStore>,
    Arc<AgentCommandService>,
    Arc<Authority>,
    TargetedControlSubmission,
) {
    let (store, service, authority, mut agent) = setup(Authority::new()).await;
    let previous = agent.revision;
    agent.status = AgentStatus::Ready;
    agent.ready_at = Some(NOW.into());
    agent.conversation.conversation_id = Some("targeted-conversation".into());
    agent.conversation.runtime_generation = Some(1);
    agent.capabilities = capabilities();
    agent.revision += 1;
    store.save_agent(previous, agent.clone()).await.unwrap();
    let admitted = service
        .admit(
            &context("owner"),
            "selected-credential",
            CommandSubmissionSource::Api,
            submission("original-response"),
        )
        .await
        .unwrap();
    let now = SystemTime::now();
    let claimed = store
        .claim_next_command(CommandClaimRequest {
            agent_id: agent.id.clone(),
            holder: "targeted-worker".into(),
            now: crate::rfc3339_at(now),
            expires_at: crate::rfc3339_at(now + Duration::from_secs(30)),
        })
        .await
        .unwrap()
        .unwrap();
    let runtime = Arc::new(Runtime(ConversationInputSender::spawn(4, |_| async {
        Ok(())
    })));
    AgentOperationService::new(store.clone(), runtime)
        .execute_claimed_command(&context("owner").caller, &claimed)
        .await
        .unwrap();
    let source = store
        .get_command(&admitted.receipt.command_id)
        .await
        .unwrap()
        .unwrap();
    let current = store.get_agent(&agent.id).await.unwrap().unwrap();
    let request = TargetedControlSubmission {
        agent_id: agent.id,
        target: TargetedResponse {
            response_id: source.receipt.response_id.unwrap(),
            turn_sequence: source.work.dispatch.unwrap().turn_sequence,
        },
        expected_agent_revision: current.revision,
        command: AgentCommand::Interrupt,
        idempotency_key: "explicit-control".into(),
    };
    (store, service, authority, request)
}

#[tokio::test]
async fn targeted_service_replays_original_intent_under_fresh_agent_revision() {
    let (store, service, _, request) = fixture().await;
    let original = service
        .admit_targeted_control(
            &context("owner"),
            "selected-credential",
            CommandSubmissionSource::Api,
            request.clone(),
        )
        .await
        .unwrap();
    assert_eq!(original.target, request.target);
    assert_eq!(original.control.receipt.response_id, None);
    let mut agent = store.get_agent(&request.agent_id).await.unwrap().unwrap();
    let previous = agent.revision;
    agent.revision += 1;
    agent.conversation.active_turn = Some(100);
    store.save_agent(previous, agent).await.unwrap();
    assert_eq!(
        service
            .admit_targeted_control(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::UserInterface,
                request.clone()
            )
            .await
            .unwrap(),
        original
    );
    assert_eq!(
        service
            .lookup_targeted_control(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::Api,
                request
            )
            .await
            .unwrap(),
        Some(original)
    );
}

#[tokio::test]
async fn history_membership_and_forged_actors_cannot_admit_or_recover_controls() {
    let (store, service, authority, request) = fixture().await;
    share(&store, None).await;
    let calls = authority.calls.load(Ordering::SeqCst);
    let mut forged = context("owner");
    forged.actor = context("reader").actor;
    let mut delegated = context("owner");
    delegated.actor = AgentActor::Api {
        subject: SubjectId::new("owner"),
        delegation_id: Some("forged".into()),
    };
    for caller in [context("reader"), context("unrelated"), forged, delegated] {
        for command in [
            AgentCommand::Interrupt,
            AgentCommand::Steer {
                input: AgentInput::text("steer"),
            },
            AgentCommand::AnswerApproval {
                approval_id: "approval".into(),
                decision: AgentApprovalDecision::Allow,
            },
        ] {
            let mut attempt = request.clone();
            attempt.command = command;
            assert!(service
                .admit_targeted_control(
                    &caller,
                    "selected-credential",
                    CommandSubmissionSource::Api,
                    attempt.clone()
                )
                .await
                .is_err());
            assert!(service
                .lookup_targeted_control(
                    &caller,
                    "selected-credential",
                    CommandSubmissionSource::Api,
                    attempt
                )
                .await
                .is_err());
        }
    }
    assert_eq!(authority.calls.load(Ordering::SeqCst), calls);
    assert_eq!(
        store
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
}

#[tokio::test]
async fn targeted_policy_change_during_custody_io_refuses_without_admission() {
    let (store, _, _, request) = fixture().await;
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let mut authority = Authority::new();
    authority.gate = Some((entered.clone(), release.clone()));
    let authority = Arc::new(authority);
    let access = Arc::new(ResourceAccessService::new(store.clone(), authority.clone()));
    let service = AgentCommandService::new(store.clone(), access, authority);
    let work = tokio::spawn(async move {
        service
            .admit_targeted_control(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::Api,
                request,
            )
            .await
    });
    entered.notified().await;
    share(&store, None).await;
    release.notify_one();
    assert!(matches!(
        work.await.unwrap(),
        Err(CommandAdmissionError::Access(_))
    ));
    assert_eq!(
        store
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
}
