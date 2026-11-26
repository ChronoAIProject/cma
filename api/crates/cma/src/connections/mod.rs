//! Owner-bound connections, completed before resource admission.

mod browser;
mod native_codex;
mod prerequisite;
mod runtime;
pub(crate) mod subscription_store;
mod subscription_tokens;
mod subscriptions;
mod targets;

#[cfg(test)]
pub(crate) use browser::tests::authorization_fixture;
pub(crate) use browser::{router, BrowserConnections};
pub(crate) use prerequisite::ConnectionPrerequisite;
pub(crate) use runtime::RuntimeSubscriptions;
pub(crate) use subscriptions::CodexSubscriptions;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum SubscriptionError {
    #[error("Sign in with Codex subscription to continue")]
    SignInRequired,
    #[error("Codex subscription sign-in expired; please try again")]
    Expired,
    #[error("Codex subscription sign-in was cancelled")]
    Cancelled,
    #[error("Codex subscription connection is temporarily unavailable")]
    Unavailable,
    #[error("Codex subscription returned an unusable account")]
    InvalidAccount,
    #[error("Codex subscription connection changed; please retry")]
    Changed,
}
