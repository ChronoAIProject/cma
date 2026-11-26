//! Fixed provider failure fields, independent of payload-bearing error messages.
use super::verification_error::ProviderVerificationError as Error;

#[derive(Clone, Copy)]
pub enum Endpoint {
    Discovery,
    Userinfo,
    Introspection,
    AgentKey,
    Verification,
}

impl Endpoint {
    pub fn label(self) -> &'static str {
        match self {
            Self::Discovery => "discovery",
            Self::Userinfo => "userinfo",
            Self::Introspection => "introspection",
            Self::AgentKey => "agent_key",
            Self::Verification => "verification",
        }
    }
}

#[derive(Clone, Copy)]
pub enum Stage {
    ResponseHeaders,
    Status,
    Body,
    Claims,
    Admission,
    Deadline,
    Configuration,
}

impl Stage {
    pub fn label(self) -> &'static str {
        match self {
            Self::ResponseHeaders => "response_headers",
            Self::Status => "status",
            Self::Body => "body",
            Self::Claims => "claims",
            Self::Admission => "admission",
            Self::Deadline => "deadline",
            Self::Configuration => "configuration",
        }
    }
}

#[derive(Clone, Copy)]
pub enum Reason {
    Timeout,
    Connection,
    Transport,
    HttpStatus,
    PartialResponse,
    InvalidBody,
    BodyTooLarge,
    BodyRead,
    ErrorDisposition,
    InvalidClaims,
    VerificationCapacity,
    FenceCapacity,
    VerificationDeadline,
    CallerDeadline,
    MissingConfiguration,
}

impl Reason {
    pub fn label(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Connection => "connection",
            Self::Transport => "transport",
            Self::HttpStatus => "http_status",
            Self::PartialResponse => "partial_response",
            Self::InvalidBody => "invalid_body",
            Self::BodyTooLarge => "body_too_large",
            Self::BodyRead => "body_read",
            Self::ErrorDisposition => "error_disposition",
            Self::InvalidClaims => "invalid_claims",
            Self::VerificationCapacity => "verification_capacity",
            Self::FenceCapacity => "verification_fence_capacity",
            Self::VerificationDeadline => "verification_deadline",
            Self::CallerDeadline => "caller_deadline",
            Self::MissingConfiguration => "missing_configuration",
        }
    }
}

pub fn record(
    endpoint: Endpoint,
    stage: Stage,
    reason: Reason,
    status: Option<u16>,
    error: &Error,
) {
    let disposition = match error {
        Error::Rejected(_) => "rejected",
        Error::Unavailable(_) => "unavailable",
        Error::Transient(_) => "transient",
        Error::RateLimited(_) => "rate_limited",
    };
    // Single-flight leaders emit the actual provider failure in their request
    // context. Shared callers retain the existing final dependency counter.
    // JSON formatters may attach the current span even for a parentless event.
    // Enter a root span at the event's level so its entire scope is content-free.
    let context = layer2_core::diagnostic_context::DiagnosticContext::current();
    let span = tracing::warn_span!(
        target: crate::diagnostics::PROVIDER_IDENTITY_TARGET,
        parent: None,
        "identity_verification"
    );
    if span.is_disabled() {
        return;
    }
    let _entered = span.enter();
    tracing::warn!(
        target: crate::diagnostics::PROVIDER_IDENTITY_TARGET,
        parent: &span,
        event = "identity_verification_failure",
        request_hash = context.request_hash.as_ref(),
        endpoint = endpoint.label(),
        stage = stage.label(),
        reason = reason.label(),
        status,
        disposition,
        "Identity provider verification failed"
    );
}

pub fn claims<T>(endpoint: Endpoint, result: Result<T, Error>) -> Result<T, Error> {
    result.inspect_err(|error| record(endpoint, Stage::Claims, Reason::InvalidClaims, None, error))
}

pub fn configuration<T>(endpoint: Endpoint, result: Result<T, Error>) -> Result<T, Error> {
    result.inspect_err(|error| {
        record(
            endpoint,
            Stage::Configuration,
            Reason::MissingConfiguration,
            None,
            error,
        )
    })
}
