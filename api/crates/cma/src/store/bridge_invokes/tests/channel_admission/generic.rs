use super::*;

struct FixtureCommandAuthority;

#[async_trait::async_trait]
impl AgentCommandAdmissionAuthority for FixtureCommandAuthority {
    async fn command_admission_authority(
        &self,
        context: &CallerContext,
        _: &str,
        owner: &AgentOwner,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        assert_eq!(context.actor, caller().actor);
        assert_eq!(
            owner,
            &AgentOwner::Person {
                subject: context.caller.subject.clone()
            }
        );
        Ok(AgentAuthorizationRevision {
            authority_id: "actual-fixture-login".into(),
            policy_revision: 1,
            grant_revision: None,
        })
    }
}

pub(super) async fn ui_command(f: &Fixture, key: &str) -> CommandStatusView {
    command_service(f)
        .admit(
            &caller(),
            "fixture-login",
            CommandSubmissionSource::UserInterface,
            CommandSubmission {
                agent_id: f.target.agent_id.clone(),
                command: AgentCommand::message(key),
                idempotency_key: key.into(),
                expected_agent_revision: None,
                requested_model: None,
            },
        )
        .await
        .unwrap()
}

pub(super) fn command_service(f: &Fixture) -> Arc<AgentCommandService> {
    AgentCommandService::new(
        f.resources.clone(),
        Arc::new(ResourceAccessService::new(
            f.resources.clone(),
            f.authority.clone(),
        )),
        Arc::new(FixtureCommandAuthority),
    )
}

pub(super) async fn assert_waiting_view(f: &Fixture, command: &AdmittedAgentCommand) {
    let view = command_service(f)
        .command_status(&caller(), None, &command.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(view.status, AgentResponseStatus::WaitingAction);
    assert_eq!(view.turn_sequence, None);
    assert_eq!(view.failure, None);
}

pub(super) async fn ordinary_claim(f: &Fixture) -> Option<AdmittedAgentCommand> {
    let now = std::time::SystemTime::now();
    f.resources
        .claim_next_command(CommandClaimRequest {
            agent_id: f.target.agent_id.clone(),
            holder: "ordinary-worker".into(),
            now: layer2_core::rfc3339_at(now),
            expires_at: layer2_core::rfc3339_at(now + std::time::Duration::from_secs(30)),
        })
        .await
        .unwrap()
}

pub(super) async fn assert_ordinary_claim(f: &Fixture, id: layer2_core::ids::AgentCommandId) {
    let next = ordinary_claim(f).await.unwrap();
    assert_eq!(next.receipt.envelope.command_id, id);
    f.resources
        .reject_command(
            &next.work.lease.as_ref().unwrap().claim,
            CommandFailureCode::AuthorityUnavailable,
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
}

pub(super) struct NoExecution;
#[async_trait::async_trait]
impl AgentCommandExecution for NoExecution {
    async fn check_queued(&self, _: &AdmittedAgentCommand) -> CommandReadiness {
        panic!("held Channel must not reach authority or runtime checks")
    }
    async fn prepare_queued(&self, _: &AdmittedAgentCommand) {
        panic!("held Channel must not revive or prepare")
    }
    async fn execute(&self, _: &AdmittedAgentCommand) -> CommandAttempt {
        panic!("held Channel must not execute")
    }
}
