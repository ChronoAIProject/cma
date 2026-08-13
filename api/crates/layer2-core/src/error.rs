//! The core's error vocabulary.
//!
//! Each variant names a distinct condition so that an interface can render it
//! in its own idiom. Interfaces translate; they must never re-derive the
//! distinction between, say, "not yours" and "does not exist".

use crate::terminal::InvalidGeometry;

/// Errors surfaced by the core.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CoreError {
    /// No such session, or not visible to this subject.
    ///
    /// Deliberately conflated with "belongs to someone else" at the edge:
    /// telling an unauthorized caller that an id exists is itself a disclosure.
    /// The distinction is preserved here so audit logging can tell them apart.
    #[error("session not found")]
    SessionNotFound,

    /// The session exists but belongs to another subject.
    #[error("forbidden")]
    Forbidden,

    /// The session has terminated and cannot be operated on.
    #[error("session terminated")]
    SessionTerminated,

    /// Another client already holds the exclusive terminal attachment.
    #[error("already attached")]
    AlreadyAttached,

    /// A terminal geometry with a zero dimension was supplied.
    #[error("invalid geometry: {0}")]
    InvalidGeometry(#[from] InvalidGeometry),

    /// The runtime backing this session is not reachable.
    #[error("runtime unavailable: {0}")]
    RuntimeUnavailable(String),

    /// The operation is not valid in the session's current state.
    #[error("invalid state for operation")]
    InvalidState,
}

impl CoreError {
    /// A stable, machine-readable code for this condition.
    ///
    /// Interfaces map these onto their own error surfaces. The strings are part
    /// of the contract and must not be reworded casually.
    pub fn code(&self) -> &'static str {
        match self {
            Self::SessionNotFound => "session_not_found",
            Self::Forbidden => "forbidden",
            Self::SessionTerminated => "session_terminated",
            Self::AlreadyAttached => "already_attached",
            Self::InvalidGeometry(_) => "invalid_geometry",
            Self::RuntimeUnavailable(_) => "runtime_unavailable",
            Self::InvalidState => "invalid_state",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::TerminalGeometry;

    #[test]
    fn every_variant_has_a_distinct_code() {
        let codes = [
            CoreError::SessionNotFound.code(),
            CoreError::Forbidden.code(),
            CoreError::SessionTerminated.code(),
            CoreError::AlreadyAttached.code(),
            CoreError::InvalidGeometry(TerminalGeometry::new(0, 1).expect_err("zero width fails"))
                .code(),
            CoreError::RuntimeUnavailable("x".into()).code(),
            CoreError::InvalidState.code(),
        ];
        let mut sorted = codes.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), codes.len(), "codes must be unique: {codes:?}");
    }

    #[test]
    fn invalid_geometry_converts_from_the_geometry_error() {
        let source = TerminalGeometry::new(80, 0).expect_err("zero height fails");
        let err: CoreError = source.into();
        assert_eq!(err.code(), "invalid_geometry");
    }
}
