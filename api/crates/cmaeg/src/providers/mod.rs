//! Allowlisted outbound provider clients used by CMAEG turns.

mod client;
mod client_types;
mod context;
mod probe;
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
pub use probe::AgentCardProbe;
pub use registry::{
    ProviderEntry, ProviderKind, ProviderPath, ProviderPaths, ProviderRegistry, ProviderUrlError,
};
pub use responses::{
    parse_event, response_id_from_frame, OutputItem, ResponseError, ResponseObject, ResponsesEvent,
    ResponsesEventError,
};
pub use sse::{parse_sse, SseError, SseFrame};
