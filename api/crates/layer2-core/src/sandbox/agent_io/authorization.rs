use crate::sandbox::{AgentMessageOutcome, AgentOperationError, AgentOperationService};
use crate::Caller;

use super::{AdmittedAgentCommand, AgentAuthorizationRevision, AgentCommandEnvelope};

/// Capture a nonsecret reference to the exact durable personal credential
/// selected at admission. Bearers without matching custody must be refused.
#[async_trait::async_trait]
pub trait PersonalCommandAdmissionAuthority: Send + Sync {
    async fn personal_command_authority(
        &self,
        caller: &Caller,
        credential: &str,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError>;
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
