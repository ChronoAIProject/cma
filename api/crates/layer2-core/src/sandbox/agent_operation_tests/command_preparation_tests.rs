use super::*;

struct Preparation {
    inner: Arc<Execution>,
    failure: CommandPreparationError,
    hang: bool,
    claim: bool,
}
#[async_trait::async_trait]
impl AgentCommandExecution for Preparation {
    async fn check_queued(&self, command: &AdmittedAgentCommand) -> CommandReadiness {
        self.inner.check_queued(command).await
    }
    async fn prepare_queued(
        &self,
        command: &AdmittedAgentCommand,
    ) -> Result<(), CommandPreparationError> {
        if self.hang {
            return std::future::pending().await;
        }
        if self.claim {
            let mut agent = self
                .inner
                .store
                .get_agent(&command.receipt.envelope.agent_id)
                .await
                .unwrap()
                .unwrap();
            let revision = agent.revision;
            agent.status = AgentStatus::Ready;
            agent.revision += 1;
            self.inner
                .store
                .save_agent(revision, agent.clone())
                .await
                .unwrap();
            self.inner
                .store
                .claim_next_command(request(&agent, "competitor", &crate::now_rfc3339()))
                .await
                .unwrap()
                .unwrap();
        }
        Err(self.failure)
    }
    async fn execute(&self, command: &AdmittedAgentCommand) -> CommandAttempt {
        self.inner.execute(command).await
    }
}

#[tokio::test(start_paused = true)]
async fn preparation_refusal_is_terminal_but_transient_errors_and_timeout_keep_original_queue() {
    for (failure, hang) in [
        (
            CommandPreparationError::Rejected(CommandFailureCode::AuthorityRevoked),
            false,
        ),
        (
            CommandPreparationError::Rejected(CommandFailureCode::CredentialSetupRequired),
            false,
        ),
        (
            CommandPreparationError::Rejected(CommandFailureCode::ConnectionsRequired),
            false,
        ),
        (CommandPreparationError::AuthorityUnavailable, false),
        (CommandPreparationError::StateChanged, false),
        (CommandPreparationError::DependencyUnavailable, false),
        (CommandPreparationError::TimedOut, true),
    ] {
        let (store, agent, parent) = fixture(AgentStatus::Paused).await;
        let admitted = admit(
            &store,
            &agent,
            &parent,
            "prepare",
            AgentCommand::message("one"),
        )
        .await;
        let (sender, inputs) = recording_sender(Ok(()));
        let operations = AgentOperationService::new(
            store.clone(),
            TestRuntimeRegistry::new(full_capabilities(), sender),
        );
        let execution = Arc::new(Preparation {
            inner: Execution::new(store.clone(), operations, Behavior::Execute),
            failure,
            hang,
            claim: false,
        });
        let worker =
            AgentCommandWorker::new(store.clone(), execution, "preparation".into()).unwrap();
        let delay = worker
            .run_scheduled(&admitted.receipt.envelope.command_id)
            .await
            .unwrap();
        let current = store
            .get_command(&admitted.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.receipt, admitted.receipt);
        assert_eq!(current.queue_deadline, admitted.queue_deadline);
        assert!(current.work.lease.is_none() && current.work.dispatch.is_none());
        match failure {
            CommandPreparationError::Rejected(code) => {
                let expected = match code {
                    CommandFailureCode::CredentialSetupRequired
                    | CommandFailureCode::ConnectionsRequired => {
                        CommandFailureCode::ExecutionFailed
                    }
                    code => code,
                };
                assert_eq!(current.work.failure.unwrap().code, expected);
                assert_eq!(current.status, AgentResponseStatus::Failed);
                assert_eq!(delay, Duration::ZERO);
            }
            _ => {
                assert_eq!(current, admitted);
                assert!(!delay.is_zero());
            }
        }
        assert!(inputs.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn another_workers_claim_fences_a_late_preparation_refusal() {
    let (store, agent, parent) = fixture(AgentStatus::Paused).await;
    let admitted = admit(
        &store,
        &agent,
        &parent,
        "stale-refusal",
        AgentCommand::message("one"),
    )
    .await;
    let (sender, inputs) = recording_sender(Ok(()));
    let operations = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    let execution = Arc::new(Preparation {
        inner: Execution::new(store.clone(), operations, Behavior::Execute),
        failure: CommandPreparationError::Rejected(CommandFailureCode::ConnectionsRequired),
        hang: false,
        claim: true,
    });
    let worker = AgentCommandWorker::new(store.clone(), execution, "stale".into()).unwrap();
    assert_eq!(
        worker
            .run_scheduled(&admitted.receipt.envelope.command_id)
            .await,
        Err(CommandLedgerError::StaleClaim)
    );
    let current = store
        .get_command(&admitted.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.work.lease.unwrap().claim.holder, "competitor");
    assert!(current.work.failure.is_none());
    assert!(inputs.lock().unwrap().is_empty());
}
