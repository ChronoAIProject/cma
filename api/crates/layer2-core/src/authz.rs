//! What a caller is allowed to do.
//!
//! The core models capabilities, not roles. Roles are an identity-provider
//! concept and their names, nesting, and wildcards belong to whoever issues
//! them; the core only needs to know whether this caller may take this action.
//! The process that verifies the credential is responsible for translating.
//!
//! That separation is what stops the provider's vocabulary leaking in here, and
//! it means a deployment can rename or restructure roles without touching a
//! single authorization decision.

use std::collections::BTreeSet;

use crate::error::CoreError;
use crate::ids::SubjectId;

/// A distinct action the core can be asked to perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Capability {
    /// Provision a new session.
    CreateSession,
    /// Read and attach to sessions the caller owns.
    UseOwnSessions,
    /// End a session the caller owns.
    TerminateOwnSession,
    /// Read every session, regardless of owner.
    ReadAllSessions,
    /// Read the runtime inventory: which sandboxes exist and their state.
    ReadRuntimeInventory,
    /// Read the durable event log of any session.
    ReadEventLog,
}

impl Capability {
    /// Whether this capability grants sight of other people's work.
    ///
    /// Kept as a predicate rather than a comment so an audit surface can list
    /// the privileged set without enumerating it by hand and drifting.
    pub fn is_privileged(self) -> bool {
        matches!(
            self,
            Self::ReadAllSessions | Self::ReadRuntimeInventory | Self::ReadEventLog
        )
    }
}

/// The set of capabilities a caller holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Authorization {
    granted: BTreeSet<Capability>,
}

impl Authorization {
    /// An authorization holding nothing.
    ///
    /// The default, deliberately: a caller whose capabilities could not be
    /// determined gets none, so a failure to translate denies rather than
    /// silently admits.
    pub fn none() -> Self {
        Self::default()
    }

    /// Build from a set of capabilities.
    pub fn from_capabilities(caps: impl IntoIterator<Item = Capability>) -> Self {
        Self {
            granted: caps.into_iter().collect(),
        }
    }

    /// Whether this caller holds a capability.
    pub fn allows(&self, capability: Capability) -> bool {
        self.granted.contains(&capability)
    }

    /// Whether this caller holds any privileged capability.
    pub fn is_privileged(&self) -> bool {
        self.granted.iter().any(|c| c.is_privileged())
    }

    /// Everything granted, for diagnostics and audit.
    pub fn capabilities(&self) -> impl Iterator<Item = Capability> + '_ {
        self.granted.iter().copied()
    }

    /// Fail unless the caller holds `capability`.
    pub fn require(&self, capability: Capability) -> Result<(), CoreError> {
        if self.allows(capability) {
            return Ok(());
        }
        Err(CoreError::Forbidden)
    }
}

/// Who is asking, and what they may do.
///
/// Carried together because every authorization decision needs both: identity
/// alone cannot answer "may they", and capabilities alone cannot answer "whose
/// session is this".
#[derive(Debug, Clone)]
pub struct Caller {
    /// The authenticated principal.
    pub subject: SubjectId,
    /// What that principal may do.
    pub authorization: Authorization,
}

impl Caller {
    /// Build a caller.
    pub fn new(subject: SubjectId, authorization: Authorization) -> Self {
        Self {
            subject,
            authorization,
        }
    }

    /// Whether this caller may act on a resource owned by `owner`.
    ///
    /// Ownership is the ordinary path. A privileged reader may see another
    /// subject's session, which is why the admin dashboard works at all, but
    /// this returns true only for capabilities that are reads: nothing here
    /// lets one subject act *as* another.
    pub fn may_read_owned_by(&self, owner: &SubjectId) -> bool {
        &self.subject == owner || self.authorization.allows(Capability::ReadAllSessions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subject(id: &str) -> SubjectId {
        SubjectId(id.to_string())
    }

    #[test]
    fn an_empty_authorization_allows_nothing() {
        // The default must deny: a caller whose capabilities could not be
        // resolved is not a caller with full access.
        let a = Authorization::none();
        for c in [
            Capability::CreateSession,
            Capability::UseOwnSessions,
            Capability::ReadAllSessions,
            Capability::ReadRuntimeInventory,
        ] {
            assert!(!a.allows(c), "{c:?} must not be granted by default");
        }
        assert!(!a.is_privileged());
    }

    #[test]
    fn require_reports_forbidden_for_a_missing_capability() {
        let a = Authorization::from_capabilities([Capability::UseOwnSessions]);
        assert!(a.require(Capability::UseOwnSessions).is_ok());
        assert_eq!(
            a.require(Capability::CreateSession),
            Err(CoreError::Forbidden)
        );
    }

    #[test]
    fn only_cross_subject_reads_count_as_privileged() {
        let ordinary = Authorization::from_capabilities([
            Capability::CreateSession,
            Capability::UseOwnSessions,
            Capability::TerminateOwnSession,
        ]);
        assert!(
            !ordinary.is_privileged(),
            "daily use must not be privileged"
        );

        let admin = Authorization::from_capabilities([Capability::ReadRuntimeInventory]);
        assert!(admin.is_privileged());
    }

    #[test]
    fn an_owner_may_read_their_own_session() {
        let c = Caller::new(
            subject("a"),
            Authorization::from_capabilities([Capability::UseOwnSessions]),
        );
        assert!(c.may_read_owned_by(&subject("a")));
    }

    #[test]
    fn an_ordinary_caller_cannot_read_someone_elses_session() {
        let c = Caller::new(
            subject("a"),
            Authorization::from_capabilities([
                Capability::CreateSession,
                Capability::UseOwnSessions,
                Capability::TerminateOwnSession,
            ]),
        );
        assert!(
            !c.may_read_owned_by(&subject("b")),
            "daily-use capabilities must not see another subject's work"
        );
    }

    #[test]
    fn a_privileged_reader_may_read_another_subjects_session() {
        let c = Caller::new(
            subject("admin"),
            Authorization::from_capabilities([Capability::ReadAllSessions]),
        );
        assert!(c.may_read_owned_by(&subject("someone-else")));
    }

    #[test]
    fn reading_everything_does_not_imply_acting_on_it() {
        // The admin surface is a monitor, not a second pair of hands. A reader
        // holds no capability that mutates another subject's session.
        let admin = Authorization::from_capabilities([
            Capability::ReadAllSessions,
            Capability::ReadRuntimeInventory,
            Capability::ReadEventLog,
        ]);
        assert!(!admin.allows(Capability::CreateSession));
        assert!(!admin.allows(Capability::TerminateOwnSession));
    }
}
