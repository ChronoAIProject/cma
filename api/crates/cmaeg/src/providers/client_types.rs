//! Public request, response, and error types for provider calls.

use std::{
    collections::BTreeMap,
    fmt,
    time::{Duration, Instant},
};

use cmaeg_core::{
    channel::{ConformanceLevel, Secret},
    ids::{BindingId, ConversationId, IdempotencyKey},
    profile::EventWorkerProfile,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use utoipa::ToSchema;

use super::responses::ResponsesEvent;

/// Provider transport and protocol failures with retry semantics preserved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    UnknownProvider(String),
    InvalidRequest(String),
    Busy { retry_after: Option<Duration> },
    Capacity,
    ConversationMissing,
    Rejected { status: u16, code: String },
    Unavailable(String),
    ResponseMalformed(String),
    Sse(String),
}

impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownProvider(provider) => {
                write!(formatter, "provider {provider:?} is not allowlisted")
            }
            Self::InvalidRequest(detail) => write!(formatter, "invalid provider request: {detail}"),
            Self::Busy { retry_after } => {
                write!(formatter, "provider is busy (retry_after={retry_after:?})")
            }
            Self::Capacity => formatter.write_str("provider concurrency is exhausted"),
            Self::ConversationMissing => formatter.write_str("provider conversation is missing"),
            Self::Rejected { status, code } => write!(
                formatter,
                "provider rejected request with HTTP {status} ({code})"
            ),
            Self::Unavailable(detail) => write!(formatter, "provider is unavailable: {detail}"),
            Self::ResponseMalformed(detail) => {
                write!(formatter, "provider response is malformed: {detail}")
            }
            Self::Sse(detail) => write!(formatter, "provider SSE failed: {detail}"),
        }
    }
}

impl std::error::Error for ProviderError {}

/// A binding request carrying the opaque agent key only to the provider call.
#[derive(Debug, Clone)]
pub struct BindingRequest {
    pub schema_version: u16,
    pub target: cmaeg_core::channel::ProviderTarget,
    pub profile: EventWorkerProfile,
    pub profile_digest: String,
    pub agent_key: Secret,
    pub tools: ToolsAdvertisement,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolsAdvertisement {
    pub native_mcp_url: String,
    pub nyxid_service_slug: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
pub struct BindingResponse {
    pub binding_id: BindingId,
    pub level: ConformanceLevel,
    #[serde(
        default,
        deserialize_with = "super::trace_contract::read_echo",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub honoured: Vec<String>,
    #[serde(default)]
    pub refused: Vec<FieldRefusal>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
pub struct FieldRefusal {
    pub field: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct TurnRequest {
    /// Set only from this binding's retained provider capability acknowledgement.
    pub nyxid_trace: bool,
    pub timing: Option<crate::telemetry::timing::IngressTiming>,
    pub conversation_id: ConversationId,
    pub input: Vec<Value>,
    pub idempotency_key: IdempotencyKey,
    pub metadata: BTreeMap<String, String>,
    pub agent_key: Secret,
    pub event_context: Option<super::EventContextSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accepted {
    pub thread_reply: bool,
    pub response_id: String,
    pub at: Instant,
}

/// The accepted signal and ordered response events for one streaming turn.
#[derive(Debug)]
pub struct TurnStream {
    pub accepted: oneshot::Receiver<Accepted>,
    pub events: TurnEvents,
    pub(crate) trace:
        Option<std::sync::Arc<std::sync::OnceLock<crate::telemetry::turn_trace::Association>>>,
    /// First successful socket write of this attempt, excluding provider processing.
    pub request_started: Instant,
    pub(crate) recovery: Option<super::recovery::AcceptedRecovery>,
}

#[derive(Debug)]
pub struct TurnEvents(pub(super) mpsc::Receiver<StreamEvent>);

impl TurnEvents {
    pub async fn recv(&mut self) -> Option<ResponsesEvent> {
        self.0.recv().await.map(|event| event.event)
    }

    pub(crate) async fn recv_observed(&mut self) -> Option<StreamEvent> {
        self.0.recv().await
    }
}

#[derive(Debug)]
pub(crate) struct StreamEvent {
    pub event: ResponsesEvent,
    pub bound_to_accepted: bool,
}

/// A provider entry's active HTTP pool and concurrency gate.
#[derive(Debug, Clone)]
pub(crate) struct ProviderHandle {
    pub(crate) entry: super::ProviderEntry,
    pub(crate) client: reqwest::Client,
    pub(crate) responses: std::sync::Arc<super::transport::ResponseTransport>,
    pub(crate) inflight: std::sync::Arc<super::permits::Gate>,
}

/// The per-request metadata bounds required by provider contracts.
pub(crate) const MAX_METADATA_KEYS: usize = 16;
pub(crate) const MAX_METADATA_KEY_BYTES: usize = 64;
pub(crate) const MAX_METADATA_VALUE_BYTES: usize = 512;
pub(crate) const MAX_SEMAPHORE_WAIT: Duration = Duration::from_millis(50);
