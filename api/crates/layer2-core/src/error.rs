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

    /// No such environment profile, or not visible to this subject.
    ///
    /// Conflated with "belongs to someone else" at the edge for the same reason
    /// as a session, and kept distinct here for the same reason too.
    #[error("environment profile not found")]
    EnvironmentProfileNotFound,

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

    /// The session's runtime does not provide something the request needs.
    ///
    /// Carries the ability's own name, because "not supported" without saying
    /// what is the answer a caller cannot act on. Distinct from
    /// `InvalidRequest`: the request is perfectly sensible, and would be
    /// honoured by a session placed on a runtime that offers this.
    #[error("capability not supported: {0}")]
    CapabilityUnsupported(String),

    /// The request was well-formed but describes something unusable.
    ///
    /// Distinct from `InvalidState`: nothing about the system prevents this,
    /// the caller simply sent something that cannot be stored or applied. The
    /// message names what is wrong and never quotes a value, which is the one
    /// part of a request that may be a credential.
    #[error("invalid request: {0}")]
    InvalidRequest(String),

    /// The owner has not granted this service, so no session may reach it.
    ///
    /// Carries the service's own identifier, because "not granted" without
    /// saying what is an answer nobody can act on. **Not transient**: retrying
    /// is pointless, and the thing to do is grant it in NyxID. That is what
    /// separates it from `ServiceCatalogUnavailable`, which is the same
    /// sentence about a different world.
    #[error("service not granted: {0}")]
    ServiceNotGranted(String),

    /// The service catalogue could not be asked.
    ///
    /// **Transient.** The owner's services are fine; we cannot list them right
    /// now. Retrying is correct, which is precisely what makes it a different
    /// answer from `ServiceNotGranted` — a caller who cannot tell them apart
    /// retries the one that will never succeed and gives up on the one that
    /// would have.
    ///
    /// Distinct from `RuntimeUnavailable` too: a caller told "runtime
    /// unavailable" looks at the sandbox provider, and this has nothing to do
    /// with one.
    #[error("service catalogue unavailable: {0}")]
    ServiceCatalogUnavailable(String),
}

impl CoreError {
    /// A stable, machine-readable code for this condition.
    ///
    /// Interfaces map these onto their own error surfaces. The strings are part
    /// of the contract and must not be reworded casually.
    pub fn code(&self) -> &'static str {
        match self {
            Self::SessionNotFound => "session_not_found",
            Self::EnvironmentProfileNotFound => "environment_profile_not_found",
            Self::Forbidden => "forbidden",
            Self::SessionTerminated => "session_terminated",
            Self::AlreadyAttached => "already_attached",
            Self::InvalidGeometry(_) => "invalid_geometry",
            Self::RuntimeUnavailable(_) => "runtime_unavailable",
            Self::InvalidState => "invalid_state",
            Self::CapabilityUnsupported(_) => "capability_unsupported",
            Self::InvalidRequest(_) => "invalid_request",
            Self::ServiceNotGranted(_) => "service_not_granted",
            Self::ServiceCatalogUnavailable(_) => "service_catalogue_unavailable",
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
            CoreError::EnvironmentProfileNotFound.code(),
            CoreError::Forbidden.code(),
            CoreError::SessionTerminated.code(),
            CoreError::AlreadyAttached.code(),
            CoreError::InvalidGeometry(TerminalGeometry::new(0, 1).expect_err("zero width fails"))
                .code(),
            CoreError::RuntimeUnavailable("x".into()).code(),
            CoreError::InvalidState.code(),
            CoreError::CapabilityUnsupported("terminal".into()).code(),
            CoreError::InvalidRequest("x".into()).code(),
            CoreError::ServiceNotGranted("api-github".into()).code(),
            CoreError::ServiceCatalogUnavailable("down".into()).code(),
        ];
        let mut sorted = codes.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), codes.len(), "codes must be unique: {codes:?}");
    }

    #[test]
    fn a_service_refusal_says_which_of_the_three_it_is() {
        // "You never granted this", "we cannot ask right now" and "this
        // deployment has no catalogue" are three answers, and a caller who
        // cannot tell them apart does the wrong thing for two of them. The
        // third is not an error at all and so has no variant here.
        assert_ne!(
            CoreError::ServiceNotGranted("api-github".into()).code(),
            CoreError::ServiceCatalogUnavailable("down".into()).code(),
        );
        // And neither is the sandbox provider being down, which is where a
        // caller told "runtime unavailable" would go looking.
        assert_ne!(
            CoreError::ServiceCatalogUnavailable("down".into()).code(),
            CoreError::RuntimeUnavailable("down".into()).code(),
        );
    }

    #[test]
    fn a_refusal_names_the_service_a_caller_has_to_act_on() {
        // "Not granted" without saying what is an answer nobody can act on.
        assert!(CoreError::ServiceNotGranted("api-github".into())
            .to_string()
            .contains("api-github"));
    }

    #[test]
    fn invalid_geometry_converts_from_the_geometry_error() {
        let source = TerminalGeometry::new(80, 0).expect_err("zero height fails");
        let err: CoreError = source.into();
        assert_eq!(err.code(), "invalid_geometry");
    }
}
