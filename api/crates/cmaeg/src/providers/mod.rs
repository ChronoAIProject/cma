//! Allowlisted outbound provider clients used by CMAEG turns.

mod binding;
mod client;
mod client_types;
mod context;
mod context_retry;
pub(crate) mod trace_contract;
pub(crate) use context_retry::diagnostics::attach_request_hash as attach_context_request_hash;
pub(crate) use context_retry::ContextFailure;
mod conversation;
mod inspection;
pub use inspection::{ConversationInspection, DesiredProfileDrift};
mod frozen;
mod management_auth;
mod media_continuations;
mod permits;
mod preparation;
mod probe;
mod profile_transfer;
mod recovery;
mod registry;
mod reset;
mod responses;
mod sse;
mod stream;
mod transport;
mod worker_classifier;
pub(crate) use worker_classifier::WorkerClassifierClient;

#[cfg(test)]
mod trace_throughput;
#[cfg(test)]
mod worker_tracking_tests;

pub use client::ProviderClient;
pub use client_types::{
    Accepted, BindingRequest, BindingResponse, FieldRefusal, ProviderError, ToolsAdvertisement,
    TurnEvents, TurnRequest, TurnStream,
};
pub use context::EventContextSnapshot;
pub(crate) use frozen::FrozenSubmission;
pub use probe::AgentCardProbe;
pub(crate) use profile_transfer::ProfileTransferRequest;
pub use recovery::{RecoveryEvent, RecoveryIdentity, RecoverySnapshot, RecoveryStream};
pub use registry::{
    PreparationPaths, ProviderEntry, ProviderKind, ProviderPath, ProviderPaths, ProviderRegistry,
    ProviderUrlError, RecoveryPaths,
};
pub use reset::ConversationReset;
pub use responses::{
    parse_event, response_id_from_frame, OutputItem, ResponseError, ResponseObject, ResponsesEvent,
    ResponsesEventError,
};
pub use sse::{parse_sse, SseError, SseFrame};
