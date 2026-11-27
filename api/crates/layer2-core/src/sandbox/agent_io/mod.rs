//! Shared interaction contracts. Authority is supplied by core authorization;
//! client content and metadata never establish an owner, actor, or grant.
//!
//! Admission envelopes belong in a separate ledger. Retained Agent aggregates
//! remain stable for ordinary commands. Continuation records require a reader
//! that understands their provenance and retained deferred attempts.

mod admission;
mod admission_service;
mod answer_context;
pub use answer_context::{answer_context_for_log, require_answer_context, AnswerContext};
mod authorization;
mod authorization_audit;
mod binding;
mod capabilities;
mod claimed_service;
mod command;
mod continuation;
pub(in crate::sandbox) mod continuation_retry;
pub use continuation_retry::{DeferredCommandAttempt, SOURCE_DEFERRED_FAILURE};
mod event;
mod evidence;
pub(in crate::sandbox) mod execution_authority;
pub(in crate::sandbox) mod execution_dispatch;
mod external_event;
mod external_event_read;
pub use external_event_read::ExternalContextObservation;
mod input;
mod interruption;
mod retained_input;
pub use interruption::*;
mod lane;
mod ledger;
mod queued_cancellation;
mod queued_input;
pub use queued_input::*;
mod message_replay_binding;
mod reconcile_pacing;
mod replay_binding;
mod reservation_policy;
mod response_read;
mod revival_authority;
mod service;
mod status;
mod targeted_control;
pub(in crate::sandbox) mod trigger;
mod work;
mod worker;
mod worker_discovery;
mod worker_funding;
mod worker_store;
pub use reconcile_pacing::{CommandReconcileHead, MAX_COMMAND_RECONCILE_HEADS};
pub use worker_discovery::{CommandDiscovery, ScheduledAgentCommand};

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
pub use authorization_audit::{
    AgentCommandAuditStore, CommandAuthorizationAudit, CommandAuthorizationEffect,
};
pub use binding::bind_command_dispatch;
pub use capabilities::AgentInterfaceCapabilities;
pub use command::{AgentCommand, AgentInputOrder};
pub use continuation::{CommandSourceGuard, ExternalEventContinuation};
pub use event::{
    AgentEphemeralEvent, AgentEvent, AgentEventKind, AgentMessageRole, AGENT_IO_SCHEMA_VERSION,
};
pub use evidence::{CertifiedCommandSource, CommandEvidence};
pub use execution_authority::{CommandExecutionAuthorization, CommandExecutionFence};
pub use external_event::{external_event_key, ExternalEventContext, MAX_EXTERNAL_EVENT_BYTES};
pub use input::{AgentAttachmentReference, AgentContent, AgentInput};
mod preparation_diagnostics;
pub use preparation_diagnostics::{command_access_error_kind, observe_command_preparation};

pub use lane::{command_ready_for_claim, CommandLane};
pub use ledger::{
    command_key_digest, validate_command_reservation, AdmittedAgentCommand, AgentCommandStore,
    CommandLedgerError, CommandReservation, CommandReservationResult, MAX_COMMAND_QUEUE_CAPACITY,
};
pub use queued_cancellation::{
    queued_cancellation_eligible, AdmittedQueuedCancellation, QueuedCancellationBinding,
    QueuedCancellationError, QueuedCancellationObservation, QueuedCancellationOutcome,
    QueuedCancellationReservation, QueuedCancellationStatusView, QueuedCancellationSubmission,
    QueuedCancellationTarget,
};
pub use reservation_policy::validate_command_reservation_policy;
pub use response_read::{AgentResponseReading, ObservedResponseReadError};
pub use revival_authority::CommandRevivalAuthorization;
pub use status::{CommandProjectionView, CommandReceiptView, CommandStatusView};
pub use targeted_control::{
    AdmittedTargetedControl, ResponseControlObservation, ResponseControlRuntimeSupport,
    TargetedControlBinding, TargetedControlError, TargetedControlInspection,
    TargetedControlReservation, TargetedControlStatusView, TargetedControlSubmission,
    TargetedResponse,
};
pub use trigger::{
    is_bound_trigger_first, TriggerExecutionAuthorization, TriggerExecutionFence,
    TriggerExecutionProof, TriggerFirstMessageBinding,
};
pub use work::{
    CommandClaim, CommandClaimRequest, CommandDispatch, CommandFailure, CommandFailureCode,
    CommandLease, CommandObservation, CommandWorkState,
};
pub use worker::{
    AgentCommandExecution, AgentCommandWorker, CommandAttempt, CommandPreparationError,
    CommandReadiness,
};
pub use worker_store::{finish_command_work_page, AgentCommandWorkerStore};

#[cfg(test)]
mod event_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod external_event_tests;
