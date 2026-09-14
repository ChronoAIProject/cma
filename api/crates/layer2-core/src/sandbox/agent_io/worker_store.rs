use async_trait::async_trait;

use super::{
    AdmittedAgentCommand, AgentCommandStore, CommandClaim, CommandClaimRequest, CommandFailureCode,
    CommandLedgerError,
};

#[async_trait]
pub trait AgentCommandWorkerStore: AgentCommandStore {
    /// One command at a time owns this Agent's dispatch lane. A lease takeover
    /// may reclaim an unfenced command, never one already bound to execution.
    async fn claim_next_command(
        &self,
        request: CommandClaimRequest,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError>;

    async fn renew_command_claim(
        &self,
        claim: &CommandClaim,
        now: String,
        expires_at: String,
    ) -> Result<(), CommandLedgerError>;

    async fn release_command_claim(
        &self,
        claim: &CommandClaim,
        now: String,
    ) -> Result<(), CommandLedgerError>;

    async fn reject_command(
        &self,
        claim: &CommandClaim,
        code: CommandFailureCode,
        now: String,
    ) -> Result<AdmittedAgentCommand, CommandLedgerError>;
}
