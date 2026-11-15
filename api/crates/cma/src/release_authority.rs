//! Explicit release selection and effect custody, independent of replica mode.
//!
//! No constructor creates collections or enables a writer. Operators migrate
//! schemas separately; ordinary startup cannot bootstrap a missing head.
mod controls;
mod effects;
mod model;
mod native_poll;
mod proof;
mod store;
mod transitions;
mod validation;

pub(crate) use model::*;
pub(crate) use native_poll::{
    NativePollCommand, NativePollExecution, NativePollLane, NativePollOutcome, NativePollReceipt,
};
pub(crate) use proof::*;
pub(crate) use store::AuthorityStore;

#[cfg(test)]
pub(crate) use native_poll::testing as native_poll_testing;

pub(crate) const HEADS: &str = "native_runtime_release_heads";
pub(crate) const OPERATIONS: &str = "native_runtime_release_operations";
pub(crate) const EFFECTS: &str = "native_runtime_effect_permits";
pub(crate) const STORE_COMMANDS: &str = "native_runtime_store_commands";
pub(crate) const HEAD_SCHEMA_VERSION: i64 = 2;
pub(crate) const COLLECTIONS: &[&str] = &[HEADS, OPERATIONS, EFFECTS, STORE_COMMANDS];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum AuthorityError {
    #[error("release authority is unavailable")]
    Unavailable,
    #[error("release authority has an unknown commit outcome; reconcile the same identity")]
    Uncertain,
    #[error("release authority conflicts with the recorded operation")]
    Conflict,
    #[error("release authority record or request is invalid")]
    Invalid,
    #[error("release effect admission is closed")]
    Closed,
    #[error("release authority evidence is incomplete")]
    EvidenceRequired,
}

pub(crate) type Result<T> = std::result::Result<T, AuthorityError>;

#[cfg(test)]
mod tests;
