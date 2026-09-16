use super::*;
use layer2_core::resource_access::{CallerContext, ResourceAccessService};

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
            authority_id: "actual-compat-fixture-login".into(),
            policy_revision: 1,
            grant_revision: None,
        })
    }
}

pub(super) async fn exercise(h: &Harness, counts_control: bool) {
    assert_eq!(h.scenario.mode, "capacity");
    let control = h.command().await;
    let original = h.control_row().await;
    assert_eq!(control.status, AgentResponseStatus::WaitingAction);
    assert!(control.work.dispatch.is_none());
    let service = AgentCommandService::new(
        h.resources.clone(),
        Arc::new(ResourceAccessService::new(
            h.resources.clone(),
            Arc::new(Authority),
        )),
        Arc::new(FixtureCommandAuthority),
    );
    let expected = if counts_control { 63 } else { 64 };
    for index in 0..=expected {
        let key = format!("compat-capacity-{index}");
        let result = service
            .admit(
                &caller(),
                "fixture-login",
                CommandSubmissionSource::UserInterface,
                CommandSubmission {
                    agent_id: control.receipt.envelope.agent_id.clone(),
                    command: AgentCommand::message(&key),
                    idempotency_key: key,
                    expected_agent_revision: None,
                    requested_model: None,
                },
            )
            .await;
        if index == expected {
            assert!(
                matches!(
                    result,
                    Err(CommandAdmissionError::Ledger(CommandLedgerError::QueueFull))
                ),
                "capacity must stop at {expected}: {result:?}"
            );
        } else {
            assert_eq!(result.unwrap().status, AgentResponseStatus::Queued);
        }
    }
    assert_eq!(h.control_row().await, original);
    assert_eq!(h.effect_count().await, 0);
    h.record_result(if counts_control { "compatible_capacity" } else { "old_capacity" }, json!({"queued_ui_messages":expected,"held_controls":1,"accounts_for_held_control":counts_control,"effects":0})).await;
}
