use super::command_work_tests::{admit, fixture, request};
use crate::ids::AgentCommandId;
use crate::sandbox::agent_io::*;
use crate::sandbox::store::InMemorySandboxStore;
use crate::sandbox::AgentStatus;
use std::sync::{Arc, Mutex};

struct SelectiveExecution {
    store: Arc<InMemorySandboxStore>,
    blocked: Option<AgentCommandId>,
    change_during_check: bool,
    attempts: Mutex<Vec<AgentCommandId>>,
}

#[async_trait::async_trait]
impl AgentCommandExecution for SelectiveExecution {
    async fn check_queued(&self, command: &AdmittedAgentCommand) -> CommandReadiness {
        if self.blocked.as_ref() == Some(&command.receipt.envelope.command_id) {
            return CommandReadiness::Retry;
        }
        if self.change_during_check {
            let now = std::time::SystemTime::now();
            let claim = self
                .store
                .claim_next_command(CommandClaimRequest {
                    agent_id: command.receipt.envelope.agent_id.clone(),
                    holder: "other-replica".into(),
                    now: crate::rfc3339_at(now),
                    expires_at: crate::rfc3339_at(now + std::time::Duration::from_secs(30)),
                })
                .await
                .unwrap()
                .unwrap()
                .work
                .lease
                .unwrap()
                .claim;
            self.store
                .release_command_claim(&claim, crate::now_rfc3339())
                .await
                .unwrap();
        }
        CommandReadiness::Ready
    }

    async fn prepare_queued(
        &self,
        _: &AdmittedAgentCommand,
    ) -> Result<(), CommandPreparationError> {
        Ok(())
    }

    async fn execute(&self, command: &AdmittedAgentCommand) -> CommandAttempt {
        self.attempts
            .lock()
            .unwrap()
            .push(command.receipt.envelope.command_id.clone());
        CommandAttempt::Retry
    }
}

#[tokio::test]
async fn ready_later_candidate_cannot_claim_an_unchecked_earlier_command() {
    let (store, agent, parent) = fixture(AgentStatus::Ready).await;
    let first = admit(
        &store,
        &agent,
        &parent,
        "first",
        AgentCommand::message("first"),
    )
    .await;
    let later = admit(
        &store,
        &agent,
        &parent,
        "later",
        AgentCommand::message("later"),
    )
    .await;
    let execution = Arc::new(SelectiveExecution {
        store: store.clone(),
        blocked: Some(first.receipt.envelope.command_id.clone()),
        change_during_check: false,
        attempts: Mutex::new(Vec::new()),
    });
    let worker =
        AgentCommandWorker::new(store.clone(), execution.clone(), "nonowner".into()).unwrap();
    worker
        .run_one(&later.receipt.envelope.command_id)
        .await
        .unwrap();
    assert!(
        execution.attempts.lock().unwrap().is_empty(),
        "readiness for a later command must not admit an attempt of the earlier command"
    );
    for command in [&first, &later] {
        assert_eq!(
            store
                .get_command(&command.receipt.envelope.command_id)
                .await
                .unwrap()
                .as_ref(),
            Some(command)
        );
    }
    // The earlier command remains selectable; the denied attempt neither
    // steals its lease nor skips it to run the later message.
    let selected = store
        .claim_next_command(request(&agent, "owner", &crate::now_rfc3339()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(selected.receipt, first.receipt);
    assert_eq!(selected.work.lease.unwrap().claim.fence, 1);
}

#[tokio::test]
async fn readiness_cannot_survive_an_intervening_claim_and_release_revision() {
    let (store, agent, parent) = fixture(AgentStatus::Ready).await;
    let command = admit(
        &store,
        &agent,
        &parent,
        "changed",
        AgentCommand::message("one"),
    )
    .await;
    let execution = Arc::new(SelectiveExecution {
        store: store.clone(),
        blocked: None,
        change_during_check: true,
        attempts: Mutex::new(Vec::new()),
    });
    AgentCommandWorker::new(store.clone(), execution.clone(), "stale-observer".into())
        .unwrap()
        .run_one(&command.receipt.envelope.command_id)
        .await
        .unwrap();
    assert!(
        execution.attempts.lock().unwrap().is_empty(),
        "a changed revision requires a fresh readiness check"
    );
    let current = store
        .get_command(&command.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.revision, command.revision + 2);
    assert!(current.work.lease.is_none());
    assert!(current.work.dispatch.is_none());
    assert!(current.work.failure.is_none());
}
