use crate::sandbox::store::{Page, PageRequest};
use async_trait::async_trait;

use super::{
    AdmittedAgentCommand, AgentCommandStore, CommandClaim, CommandClaimRequest, CommandFailureCode,
    CommandLedgerError,
};

#[async_trait]
pub trait AgentCommandWorkerStore: AgentCommandStore {
    /// Bounded, stable discovery of nonterminal ledger work. This is an internal
    /// scheduler hint, never permission to execute or a claim on the lane.
    async fn command_work_page(
        &self,
        page: &PageRequest,
    ) -> Result<Page<AdmittedAgentCommand>, CommandLedgerError>;

    /// Read-only, nontransactional hints for at most one discovery page (50
    /// Agents). Missing or unreadable rows are absent, never evidence of loss.
    async fn command_reconcile_heads(
        &self,
        agent_ids: &[crate::AgentId],
    ) -> Result<Vec<super::CommandReconcileHead>, CommandLedgerError>;

    /// Refusal only: compare the observed revision and prove the command is
    /// still queued, unbound and without a live lease in the same atomic write.
    async fn reject_unclaimed_command(
        &self,
        id: &crate::ids::AgentCommandId,
        expected_revision: u64,
        code: CommandFailureCode,
        now: String,
    ) -> Result<(), CommandLedgerError>;

    /// Rebuild delivery and response state from the authoritative Agent and its
    /// complete log in one snapshot. Scheduling heads never replace this proof;
    /// every state transition still uses this path. No caller-supplied completion
    /// is accepted.
    async fn reconcile_command(
        &self,
        id: &crate::ids::AgentCommandId,
    ) -> Result<AdmittedAgentCommand, CommandLedgerError>;

    /// One command at a time owns this Agent's dispatch lane. A lease takeover
    /// may reclaim an unfenced command, never one already bound to execution.
    async fn claim_next_command(
        &self,
        request: CommandClaimRequest,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError>;

    /// Claim the authoritative next command only if its identity and revision
    /// still match the record whose readiness was checked. Compare inside the
    /// same transaction/lock as FIFO selection, before changing either record.
    /// A mismatch returns no claim; it must never skip earlier eligible work.
    async fn claim_observed_command(
        &self,
        request: CommandClaimRequest,
        command_id: &crate::ids::AgentCommandId,
        expected_revision: u64,
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

pub fn finish_command_work_page(
    mut items: Vec<AdmittedAgentCommand>,
    page: &PageRequest,
) -> Result<Page<AdmittedAgentCommand>, CommandLedgerError> {
    PageRequest::new(page.limit, page.cursor.clone()).map_err(|_| CommandLedgerError::Invalid)?;
    if items.len() > usize::from(page.limit) + 1 {
        return Err(CommandLedgerError::Corrupt);
    }
    let mut previous = page.cursor.as_deref();
    for item in &items {
        item.validate()?;
        let id = item.receipt.envelope.command_id.as_str();
        if item.status.is_terminal() || previous.is_some_and(|previous| previous >= id) {
            return Err(CommandLedgerError::Corrupt);
        }
        previous = Some(id);
    }
    let next = (items.len() > usize::from(page.limit)).then(|| {
        items[usize::from(page.limit) - 1]
            .receipt
            .envelope
            .command_id
            .to_string()
    });
    items.truncate(usize::from(page.limit));
    Ok(Page { items, next })
}
