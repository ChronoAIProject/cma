//! Shared interaction contracts. Authority is supplied by core authorization;
//! client content and metadata never establish an owner, actor, or grant.
//!
//! Admission envelopes belong in a separate ledger. Retained Agent aggregates
//! and event variants remain readable by previous releases during rollback.

mod admission;
mod capabilities;
mod command;
mod event;
mod input;
mod ledger;
mod service;

pub use admission::{
    AdmissionReceipt, AgentActor, AgentAuthorizationRevision, AgentCommandEnvelope,
    AgentCommandSource, AgentOwner, AgentResponseStatus, AgentSourceKind,
};
pub use capabilities::AgentInterfaceCapabilities;
pub use command::{AgentCommand, AgentInputOrder};
pub use event::{
    AgentEphemeralEvent, AgentEvent, AgentEventKind, AgentMessageRole, AGENT_IO_SCHEMA_VERSION,
};
pub use input::{AgentAttachmentReference, AgentContent, AgentInput};
pub use ledger::{
    command_key_digest, validate_command_reservation, AdmittedAgentCommand, AgentCommandStore,
    CommandLedgerError, CommandReservation, CommandReservationResult, MAX_COMMAND_QUEUE_CAPACITY,
};

#[cfg(test)]
mod event_tests;
#[cfg(test)]
mod tests;
