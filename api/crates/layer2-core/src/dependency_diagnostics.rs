//! Content-free attribution carried only through the current request future.
//!
//! Record the first known dependency before an error is projected to a public
//! contract. Only the outer boundary reports a failed request; recovered errors,
//! successful requests and cancelled futures do not increment failure counters.
use std::{
    cell::Cell,
    future::Future,
    sync::atomic::{AtomicU64, Ordering},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Authentication,
    FileContent,
    ModelList,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dependency {
    Local,
    Storage,
    IdentityProvider,
    Runtime,
    ModelGateway,
    AuthenticationAuthority,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Unavailable,
    Corrupt,
    Capacity,
    VerificationDeadline,
    Timeout,
    Connection,
    BadGateway,
    UpstreamUnavailable,
    RateLimited,
    UnexpectedStatus,
    Transport,
    BodyTimeout,
    InvalidEvidence,
    Configuration,
    AuthorityChanged,
    CredentialMissing,
    CredentialExpired,
    CredentialMismatch,
    ScopeMissing,
    ResponseTooLarge,
    ResponseBody,
    RuntimeLedgerHold,
    InputAuthority,
    PromptNotWritten,
    Unknown,
}

macro_rules! labels {
    ($kind:ty, $($variant:ident => $label:literal),+ $(,)?) => {
        impl $kind {
            pub fn label(self) -> &'static str {
                match self { $(Self::$variant => $label),+ }
            }
        }
    };
}
labels!(Operation, Authentication => "authentication", FileContent => "file_content", ModelList => "model_list");
labels!(Dependency, Local => "local", Storage => "storage", IdentityProvider => "identity_provider",
    Runtime => "runtime", ModelGateway => "model_gateway", AuthenticationAuthority => "authentication_authority");
labels!(Reason, Unavailable => "unavailable", Corrupt => "corrupt", Capacity => "capacity",
    VerificationDeadline => "verification_deadline",
    Timeout => "timeout", Connection => "connection", BadGateway => "http_502", UpstreamUnavailable => "http_503",
    RateLimited => "http_429", UnexpectedStatus => "http_other", Transport => "transport", BodyTimeout => "body_timeout",
    InvalidEvidence => "invalid_evidence", Configuration => "configuration", AuthorityChanged => "authority_changed",
    CredentialMissing => "credential_missing", CredentialExpired => "credential_expired",
    CredentialMismatch => "credential_mismatch", ScopeMissing => "scope_missing", ResponseTooLarge => "response_too_large",
    ResponseBody => "response_body", RuntimeLedgerHold => "runtime_ledger_hold", InputAuthority => "input_authority",
    PromptNotWritten => "prompt_not_written", Unknown => "unknown");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Failure {
    pub dependency: Dependency,
    pub reason: Reason,
}

impl Failure {
    pub const fn new(dependency: Dependency, reason: Reason) -> Self {
        Self { dependency, reason }
    }

    /// A bounded diagnostic only; never change the error or request's outcome.
    pub fn record(self) {
        let _ = CURRENT.try_with(|current| {
            if current.get().is_none() {
                current.set(Some(self));
            }
        });
    }
}

tokio::task_local! {
    static CURRENT: Cell<Option<Failure>>;
}

pub async fn capture<F: Future>(future: F) -> (F::Output, Option<Failure>) {
    CURRENT
        .scope(Cell::new(None), async {
            let result = future.await;
            (result, CURRENT.with(Cell::get))
        })
        .await
}

const REASONS: usize = Reason::Unknown as usize + 1;
const DEPENDENCIES: usize = Dependency::AuthenticationAuthority as usize + 1;
const COUNTERS: usize = (Operation::ModelList as usize + 1) * DEPENDENCIES * REASONS;
static FAILURES: [AtomicU64; COUNTERS] = [const { AtomicU64::new(0) }; COUNTERS];

/// API metrics use structured logs, like the existing Mongo/ledger counters.
/// Categories are finite and process-local; no dynamic label or inherited span.
pub fn report(operation: Operation, failure: Option<Failure>) {
    let failure = failure.unwrap_or(Failure::new(Dependency::Local, Reason::Unknown));
    let index = (operation as usize * DEPENDENCIES + failure.dependency as usize) * REASONS
        + failure.reason as usize;
    emit(&FAILURES[index], operation, failure);
}

fn emit(counter: &AtomicU64, operation: Operation, failure: Failure) {
    let previous = counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            Some(value.saturating_add(1))
        })
        .unwrap_or_else(|value| value);
    tracing::warn!(parent: None,
        metric = "cma_dependency_failures_total",
        operation = operation.label(), dependency = failure.dependency.label(),
        reason = failure.reason.label(), count = 1u64, failures_total = previous.saturating_add(1),
        "request dependency unavailable"
    );
}

#[cfg(test)]
mod tests;
