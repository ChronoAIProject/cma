//! Explicit release selection and effect custody, independent of replica mode.
//!
//! No constructor creates collections or enables a writer. Operators migrate
//! schemas separately; ordinary startup cannot bootstrap a missing head.
mod controls;
mod effects;
mod model;
mod proof;
mod store;
mod transitions;
mod validation;

pub(crate) use model::*;
pub(crate) use proof::*;
pub(crate) use store::AuthorityStore;

pub(crate) const HEADS: &str = "native_runtime_release_heads";
pub(crate) const OPERATIONS: &str = "native_runtime_release_operations";
pub(crate) const EFFECTS: &str = "native_runtime_effect_permits";
pub(crate) const COLLECTIONS: &[&str] = &[HEADS, OPERATIONS, EFFECTS];

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
