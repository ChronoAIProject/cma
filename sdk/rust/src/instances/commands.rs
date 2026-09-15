//! Explicit personal/shared lifecycle intents and safe durable receipts.
use super::Sandboxes;
use crate::{Error, IdempotencyKey, Result, resources::positive};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};

string_enum! { /// Explicit Sandbox lifecycle command.
    pub enum SandboxCommandKind { Pause => "pause", Revive => "revive", Retire => "retire" }
}
string_enum! { /// Result of applying a lifecycle intent to durable state.
    pub enum SandboxCommandDisposition { Scheduled => "scheduled", AlreadyScheduled => "already_scheduled", AlreadySatisfied => "already_satisfied" }
}
string_enum! { /// Terminal refusal before lifecycle scheduling.
    pub enum SandboxCommandFailure { AuthorityRevoked => "authority_revoked", QueueExpired => "queue_expired", TargetChanged => "target_changed", NotEligible => "not_eligible", ConnectionsRequired => "connections_required", CredentialSetupRequired => "credential_setup_required" }
}
/// Required revision precondition for a durable lifecycle intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxCommandRequest {
    /// Requested lifecycle action.
    pub command: SandboxCommandKind,
    /// Revision from a current authorized Sandbox detail read.
    pub expected_revision: u64,
}
/// Applied means the lifecycle transition committed; current Sandbox status
/// reports runtime progress. Rejected commands have no scheduled effect.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SandboxCommandState {
    /// Awaiting fresh authority and lifecycle scheduling.
    Queued,
    /// Durable application outcome.
    Applied {
        /// Server application time.
        at: String,
        /// Resulting Sandbox revision.
        sandbox_revision: u64,
        /// Whether a new transition was scheduled.
        disposition: SandboxCommandDisposition,
    },
    /// Terminal refusal.
    Rejected {
        /// Server rejection time.
        at: String,
        /// Typed refusal without owner connection details.
        reason: SandboxCommandFailure,
    },
}
/// Safe command receipt; no owner configuration or credential reference.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SandboxCommandReceipt {
    /// Stable command identity for recovery.
    pub command_id: String,
    /// Independently authorized Sandbox identity.
    pub sandbox_id: String,
    /// Original action.
    pub command: SandboxCommandKind,
    /// Original precondition.
    pub expected_revision: u64,
    /// Server admission time.
    pub admitted_at: String,
    /// Original nonrenewing queue deadline.
    pub queue_deadline: String,
    /// Receipt revision: one queued, two terminal.
    pub revision: u64,
    /// Current durable command state.
    pub state: SandboxCommandState,
}
impl SandboxCommandReceipt {
    fn validate(&self) -> Result<()> {
        let revision = if self.state == SandboxCommandState::Queued {
            1
        } else {
            2
        };
        if self.command_id.is_empty()
            || self.sandbox_id.is_empty()
            || self.expected_revision == 0
            || self.expected_revision > i64::MAX as u64
            || self.revision != revision
        {
            return Err(Error::protocol(
                "Sandbox command identity or state revision is invalid",
            ));
        }
        if let SandboxCommandState::Applied {
            sandbox_revision,
            disposition,
            ..
        } = self.state
        {
            let expected = self.expected_revision.checked_add(u64::from(
                disposition == SandboxCommandDisposition::Scheduled,
            ));
            if Some(sandbox_revision) != expected || sandbox_revision > i64::MAX as u64 {
                return Err(Error::protocol(
                    "Sandbox command outcome revision is inconsistent",
                ));
            }
        }
        Ok(())
    }
}
impl Sandboxes<'_> {
    /// Admit a durable personal/shared intent. Retry the same key and body to
    /// recover an unacknowledged receipt; do not replace the expected revision.
    pub async fn submit_command(
        &self,
        id: &str,
        body: &SandboxCommandRequest,
        key: &IdempotencyKey,
    ) -> Result<SandboxCommandReceipt> {
        positive(body.expected_revision)?;
        if body.expected_revision > i64::MAX as u64 {
            return Err(Error::validation(
                "Sandbox revision exceeds the supported range",
            ));
        }
        let receipt: SandboxCommandReceipt = self
            .client
            .command(
                Method::POST,
                &["sandboxes", id, "commands"],
                Some(body),
                key,
                StatusCode::ACCEPTED,
            )
            .await?;
        receipt.validate()?;
        if receipt.sandbox_id != id
            || receipt.command != body.command
            || receipt.expected_revision != body.expected_revision
        {
            return Err(Error::protocol(
                "Sandbox command receipt did not match the request",
            ));
        }
        Ok(receipt)
    }
    /// Read a receipt while its requesting actor retains target visibility.
    pub async fn read_command(&self, command_id: &str) -> Result<SandboxCommandReceipt> {
        let receipt: SandboxCommandReceipt = self
            .client
            .get(&["sandbox-commands", command_id], &[])
            .await?;
        receipt.validate()?;
        if receipt.command_id != command_id {
            return Err(Error::protocol(
                "Sandbox command identity did not match the request",
            ));
        }
        Ok(receipt)
    }
}
