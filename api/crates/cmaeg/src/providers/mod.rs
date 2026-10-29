//! Allowlisted outbound provider clients used by CMAEG turns.

mod client;
mod client_types;
mod context;
mod frozen;
mod management_auth;
mod preparation;
mod probe;
mod recovery;
mod registry;
mod responses;
mod sse;
mod stream;
mod transport;

pub use client::ProviderClient;
pub use client_types::{
    Accepted, BindingRequest, BindingResponse, FieldRefusal, ProviderError, ToolsAdvertisement,
    TurnRequest, TurnStream,
};
pub use context::EventContextSnapshot;
pub(crate) use frozen::FrozenSubmission;
pub use probe::AgentCardProbe;
pub use recovery::{RecoveryEvent, RecoveryIdentity, RecoverySnapshot, RecoveryStream};
pub use registry::{
    PreparationPaths, ProviderEntry, ProviderKind, ProviderPath, ProviderPaths, ProviderRegistry,
    ProviderUrlError, RecoveryPaths,
};
pub use responses::{
    parse_event, response_id_from_frame, OutputItem, ResponseError, ResponseObject, ResponsesEvent,
    ResponsesEventError,
};
pub use sse::{parse_sse, SseError, SseFrame};
