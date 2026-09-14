//! Shared interaction contracts. Authority is supplied by core authorization;
//! client content and metadata never establish an owner, actor, or grant.
//!
//! Admission envelopes belong in a separate ledger. Retained Agent aggregates
//! and event variants remain readable by previous releases during rollback.

mod admission;
mod admission_service;
mod authorization;
mod binding;
mod capabilities;
mod claimed_service;
mod command;
mod event;
mod evidence;
pub(in crate::sandbox) mod execution_authority;
pub(in crate::sandbox) mod execution_dispatch;
mod input;
mod lane;
mod ledger;
mod reservation_policy;
mod response_read;
mod service;
mod status;
mod work;
mod worker;
mod worker_store;

pub use admission::{
    AdmissionReceipt, AgentActor, AgentAuthorizationRevision, AgentCommandEnvelope,
    AgentCommandSource, AgentOwner, AgentResponseStatus, AgentSourceKind,
};
pub use admission_service::{
    AgentCommandService, CommandAdmissionError, CommandSubmission, CommandSubmissionSource,
};
pub use authorization::{
    AgentCommandAdmissionAuthority, AgentCommandAuthorizer, CommandAuthorizationError,
    DeferredCommandError, PersonalCommandAdmissionAuthority, ResourceCommandAuthorizationError,
};
pub use binding::bind_command_dispatch;
pub use capabilities::AgentInterfaceCapabilities;
pub use command::{AgentCommand, AgentInputOrder};
pub use event::{
    AgentEphemeralEvent, AgentEvent, AgentEventKind, AgentMessageRole, AGENT_IO_SCHEMA_VERSION,
};
pub use evidence::CommandEvidence;
pub use execution_authority::{CommandExecutionAuthorization, CommandExecutionFence};
pub use input::{AgentAttachmentReference, AgentContent, AgentInput};
pub use lane::{command_ready_for_claim, CommandLane};
pub use ledger::{
    command_key_digest, validate_command_reservation, AdmittedAgentCommand, AgentCommandStore,
    CommandLedgerError, CommandReservation, CommandReservationResult, MAX_COMMAND_QUEUE_CAPACITY,
};
pub use reservation_policy::validate_command_reservation_policy;
pub use response_read::AgentResponseReading;
pub use status::{CommandReceiptView, CommandStatusView};
pub use work::{
    CommandClaim, CommandClaimRequest, CommandDispatch, CommandFailure, CommandFailureCode,
    CommandLease, CommandObservation, CommandWorkState,
};
pub use worker::{AgentCommandExecution, AgentCommandWorker, CommandAttempt, CommandReadiness};
pub use worker_store::{finish_command_work_page, AgentCommandWorkerStore};

#[cfg(test)]
mod event_tests;
#[cfg(test)]
mod tests;
