use crate::resource_access::{ResourceAccessError, ResourceAccessService};
use crate::sandbox::{AgentMessageOutcome, AgentOperationError, AgentOperationService};
use crate::Caller;

use super::{
    AdmittedAgentCommand, AgentAuthorizationRevision, AgentCommandEnvelope,
    CommandExecutionAuthorization,
};

/// Capture a nonsecret reference to the exact durable personal credential
/// selected at admission. Bearers without matching custody must be refused.
#[async_trait::async_trait]
pub trait PersonalCommandAdmissionAuthority: Send + Sync {
    async fn personal_command_authority(
        &self,
        caller: &Caller,
        credential: &str,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError>;

    /// Service management requires direct owner authority. Implementations may
    /// retain a verified independent owner credential without borrowing a login.
    async fn owner_service_access_authority(
        &self,
        context: &crate::resource_access::CallerContext,
        credential: &str,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        if context.credential_source != crate::resource_access::CredentialSource::Retained {
            return Err(CommandAuthorizationError::Revoked);
        }
        self.personal_command_authority(&context.caller, credential)
            .await
    }

    /// Independent clients must already hold the requested connection authority.
    /// Interactive owners retain the existing explicit connection workflow.
    async fn confirm_owner_service_scope(
        &self,
        context: &crate::resource_access::CallerContext,
        _credential: &str,
        _services: &[String],
    ) -> Result<(), crate::sandbox::agent_access::AgentAccessError> {
        if context.credential_source != crate::resource_access::CredentialSource::Retained {
            return Err(crate::sandbox::agent_access::AgentAccessError::OwnerAuthorityRequired);
        }
        Ok(())
    }
}

/// Select durable execution custody for a currently authorized resource. The
/// actor remains the authenticated principal; resource access by itself cannot
/// supply somebody else's credential or establish deferred execution custody.
#[async_trait::async_trait]
pub trait AgentCommandAdmissionAuthority: Send + Sync {
    async fn command_admission_authority(
        &self,
        context: &crate::resource_access::CallerContext,
        credential: &str,
        owner: &super::AgentOwner,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommandAuthorizationError {
    #[error("the authority that admitted this command is no longer available")]
    Revoked,
    #[error("current command authority could not be verified")]
    Unavailable,
}

/// Revalidate the actual retained credential/grant before a deferred effect.
/// Implementations must verify the principal and current provider capabilities;
/// copying an envelope's owner into a Caller is not authorization.
#[async_trait::async_trait]
pub trait AgentCommandAuthorizer: Send + Sync {
    async fn authorize_command(
        &self,
        envelope: &AgentCommandEnvelope,
    ) -> Result<Caller, CommandAuthorizationError>;

    /// Verify retained custody and current resource authority for the exact
    /// command. Shared implementations must select the actor's retained sign-in,
    /// verify its current credential and recheck exact custody after resource IO.
    /// The returned proof must not retain a raw credential.
    async fn authorize_resource_command(
        &self,
        access: &ResourceAccessService,
        envelope: &AgentCommandEnvelope,
    ) -> Result<CommandExecutionAuthorization, ResourceCommandAuthorizationError> {
        let caller = self.authorize_command(envelope).await?;
        require_personal_actor(&caller, envelope)?;
        Ok(access
            .authorize_personal_command_execution(&caller, envelope)
            .await?)
    }

    async fn authorize_command_revival(
        &self,
        access: &ResourceAccessService,
        command: &AdmittedAgentCommand,
    ) -> Result<std::sync::Arc<super::CommandRevivalAuthorization>, ResourceCommandAuthorizationError>
    {
        let caller = self.authorize_command(&command.receipt.envelope).await?;
        require_personal_actor(&caller, &command.receipt.envelope)?;
        Ok(access
            .authorize_personal_command_revival(&caller, command)
            .await?)
    }
}

fn require_personal_actor(
    caller: &Caller,
    envelope: &AgentCommandEnvelope,
) -> Result<(), CommandAuthorizationError> {
    if envelope.owner
        != (super::AgentOwner::Person {
            subject: caller.subject.clone(),
        })
        || !matches!(&envelope.actor, super::AgentActor::Human { subject }
            | super::AgentActor::Api { subject, delegation_id: None } if subject == &caller.subject)
    {
        return Err(CommandAuthorizationError::Revoked);
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum ResourceCommandAuthorizationError {
    #[error(transparent)]
    Custody(#[from] CommandAuthorizationError),
    #[error(transparent)]
    Resource(#[from] ResourceAccessError),
}

#[derive(Debug, thiserror::Error)]
pub enum DeferredCommandError {
    #[error(transparent)]
    Authorization(#[from] CommandAuthorizationError),
    #[error(transparent)]
    Execution(#[from] AgentOperationError),
}

impl AgentOperationService {
    /// Fresh authority followed by the existing atomic dispatch fence. This
    /// entry point never treats a recovered response as permission to resend.
    pub async fn execute_authorized_claim(
        &self,
        authorizer: &dyn AgentCommandAuthorizer,
        command: &AdmittedAgentCommand,
    ) -> Result<AgentMessageOutcome, DeferredCommandError> {
        command
            .validate()
            .map_err(|_| AgentOperationError::Corrupt("command ledger is invalid".into()))?;
        if command.work.dispatch.is_some() || command.status.is_terminal() {
            return Err(AgentOperationError::Conflict.into());
        }
        let caller = authorizer
            .authorize_command(&command.receipt.envelope)
            .await?;
        // execute_claimed_command rechecks time after potentially slow provider
        // IO. The transaction also proves the claim still owns the durable lane.
        Ok(self.execute_claimed_command(&caller, command).await?)
    }
}
