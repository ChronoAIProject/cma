use crate::sandbox::{AgentMessageOutcome, AgentOperationError, AgentOperationService};
use crate::{now_rfc3339, Caller};

use super::{AdmittedAgentCommand, AgentActor, AgentOwner, CommandExecutionAuthorization};

impl AgentOperationService {
    /// Execute under current resource policy while retaining the authenticated
    /// actor. Both the dispatch transaction and the ordered writer check this
    /// same nonrenewing proof; outcome recording needs no renewed permission.
    pub async fn execute_resource_claim(
        &self,
        authority: &CommandExecutionAuthorization,
        command: &AdmittedAgentCommand,
    ) -> Result<AgentMessageOutcome, AgentOperationError> {
        authority
            .require_source_guard()
            .map_err(|_| AgentOperationError::Forbidden)?;
        command
            .validate()
            .map_err(|_| AgentOperationError::Corrupt("command ledger is invalid".into()))?;
        authority
            .fence()
            .require_envelope(&command.receipt.envelope)
            .map_err(|_| AgentOperationError::Forbidden)?;
        let claim = command
            .work
            .lease
            .as_ref()
            .map(|lease| &lease.claim)
            .ok_or(AgentOperationError::Conflict)?;
        command
            .require_claim(claim, &now_rfc3339())
            .map_err(|_| AgentOperationError::Conflict)?;
        if command.work.dispatch.is_some() {
            return Err(AgentOperationError::Conflict);
        }
        let service = Self {
            settings_authority: None,
            command_claim: Some(claim.clone()),
            execution_authority: Some(authority.clone()),
            trigger_authority: None,
            instruction_policy: self.instruction_policy.clone(),
            connections: self.connections.clone(),
            store: self.store.clone(),
            runtimes: self.runtimes.clone(),
        };
        service
            .execute_command(
                authority.caller(),
                &command.receipt.envelope.agent_id,
                command
                    .effective_command()
                    .map_err(|_| AgentOperationError::Conflict)?,
            )
            .await
    }

    /// Execute one claimed personal command using the authenticated caller.
    /// Organization and channel execution require their own fresh authorization.
    /// The conversation store checks the durable input and claim atomically.
    pub async fn execute_claimed_command(
        &self,
        caller: &Caller,
        command: &AdmittedAgentCommand,
    ) -> Result<AgentMessageOutcome, AgentOperationError> {
        command
            .validate()
            .map_err(|_| AgentOperationError::Corrupt("command ledger is invalid".into()))?;
        let envelope = &command.receipt.envelope;
        if envelope.continuation.is_some()
            || envelope.owner
                != (AgentOwner::Person {
                    subject: caller.subject.clone(),
                })
            || !matches!(&envelope.actor,
                AgentActor::Human { subject } | AgentActor::Api { subject, delegation_id: None }
                if subject == &caller.subject)
        {
            return Err(AgentOperationError::Forbidden);
        }
        let claim = command
            .work
            .lease
            .as_ref()
            .map(|lease| &lease.claim)
            .ok_or(AgentOperationError::Conflict)?;
        command
            .require_claim(claim, &now_rfc3339())
            .map_err(|_| AgentOperationError::Conflict)?;
        if command.work.dispatch.is_some() {
            return Err(AgentOperationError::Conflict);
        }
        let service = Self {
            settings_authority: None,
            command_claim: Some(claim.clone()),
            execution_authority: None,
            trigger_authority: None,
            instruction_policy: self.instruction_policy.clone(),
            connections: self.connections.clone(),
            store: self.store.clone(),
            runtimes: self.runtimes.clone(),
        };
        service
            .execute_command(
                caller,
                &envelope.agent_id,
                command
                    .effective_command()
                    .map_err(|_| AgentOperationError::Conflict)?,
            )
            .await
    }
}
