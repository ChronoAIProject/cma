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
    /// Read and write the caller's own reusable environment bundles.
    ///
    /// Its own capability rather than part of session use: a bundle holds
    /// values a caller is invited to treat as secret, and outlives every
    /// session that referenced it.
    ManageOwnEnvironmentProfiles,
    /// Read every session, regardless of owner.
    ReadAllSessions,
    /// Read the runtime inventory: which sandboxes exist and their state.
    ReadRuntimeInventory,
    /// Read the durable event log of any session.
    ReadEventLog,
    /// Read the record of what the service has been asked to do.
    ///
    /// Its own capability rather than part of reading every session: the
    /// record names every subject that has ever called, and is the one place
    /// an operator's own actions are visible to somebody else.
    ReadActivityLog,
    /// Read the directory of subjects that have used the service.
    ReadUserDirectory,
    /// End or pause a session whatever subject owns it.
    ///
    /// Deliberately separate from ending your own. Every caller who can use a
    /// session holds the first; only an operator holds this, so a role that
    /// gains everyday use can never quietly acquire the ability to stop
    /// somebody else's work.
    ControlAnySession,
}

impl Capability {
    /// Whether this capability reaches past the caller's own work.
    ///
    /// Kept as a predicate rather than a comment so an audit surface can list
    /// the privileged set without enumerating it by hand and drifting.
    pub fn is_privileged(self) -> bool {
        matches!(
            self,
            Self::ReadAllSessions
                | Self::ReadRuntimeInventory
                | Self::ReadEventLog
                | Self::ReadActivityLog
                | Self::ReadUserDirectory
                | Self::ControlAnySession
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

    /// Whether this caller may read the durable event log of `owner`'s session.
    ///
    /// A separate grant from seeing the session, because the log says more than
    /// the list does: it is every state change, every configuration a session
    /// was given and every reason one ended, in order. Reading who is running
    /// what should not carry reading everything they have done, or the two
    /// capabilities the provider issues separately collapse into one.
    pub fn may_read_event_log_owned_by(&self, owner: &SubjectId) -> bool {
        &self.subject == owner || self.authorization.allows(Capability::ReadEventLog)
    }

    /// Fail unless this caller may end or pause *some* session.
    ///
    /// The cheap half of the check, made before anything is read. A caller
    /// holding neither grant is refused without a lookup, so an identifier
    /// cannot be probed for existence by somebody entitled to nothing at all.
    pub fn require_any_control(&self) -> Result<(), CoreError> {
        if self.authorization.allows(Capability::TerminateOwnSession)
            || self.authorization.allows(Capability::ControlAnySession)
        {
            return Ok(());
        }
        Err(CoreError::Forbidden)
    }

    /// Fail unless this caller may end or pause a session owned by `owner`.
    ///
    /// The other half, made once the owner is known. Acting on your own work
    /// needs the everyday grant; acting on somebody else's needs the operator
    /// one, and holding the everyday grant never stands in for it.
    pub fn require_control_of(&self, owner: &SubjectId) -> Result<(), CoreError> {
        if &self.subject == owner && self.authorization.allows(Capability::TerminateOwnSession) {
            return Ok(());
        }
        self.authorization.require(Capability::ControlAnySession)
    }

    /// Fail unless this caller *is* the owner and may control their own work.
    ///
    /// Stricter than [`Self::require_control_of`], and deliberately so. An
    /// operator grant stands in for stopping somebody else's session, because
    /// stopping is something an operator has to be able to do. Changing which
    /// plane a session is driven on is not: it changes what the owner is
    /// looking at, and nobody else has a reason to.
    pub fn require_own_control_of(&self, owner: &SubjectId) -> Result<(), CoreError> {
        if &self.subject != owner {
            return Err(CoreError::Forbidden);
        }
        self.authorization.require(Capability::TerminateOwnSession)
    }

    /// Whether this caller is acting on work that is not theirs.
    ///
    /// Worth naming because it is the thing a record of actions exists for:
    /// the same operation means something different when the subject taking it
    /// is not the one whose work it ends.
    pub fn is_acting_for_another(&self, owner: &SubjectId) -> bool {
        &self.subject != owner
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
    fn only_capabilities_that_cross_subjects_count_as_privileged() {
        let ordinary = Authorization::from_capabilities([
            Capability::CreateSession,
            Capability::UseOwnSessions,
            Capability::TerminateOwnSession,
            Capability::ManageOwnEnvironmentProfiles,
        ]);
        assert!(
            !ordinary.is_privileged(),
            "daily use must not be privileged"
        );

        for c in [
            Capability::ReadAllSessions,
            Capability::ReadRuntimeInventory,
            Capability::ReadEventLog,
            Capability::ReadActivityLog,
            Capability::ReadUserDirectory,
            Capability::ControlAnySession,
        ] {
            assert!(
                Authorization::from_capabilities([c]).is_privileged(),
                "{c:?} reaches past the caller and must count as privileged"
            );
        }
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
    fn reading_every_session_does_not_imply_reading_every_event_log() {
        // The two are issued as separate permissions, so one standing in for
        // the other would mean the narrower grant silently carried the wider
        // one and the distinction the provider draws bought nothing.
        let c = Caller::new(
            subject("admin"),
            Authorization::from_capabilities([Capability::ReadAllSessions]),
        );
        assert!(c.may_read_owned_by(&subject("someone-else")));
        assert!(
            !c.may_read_event_log_owned_by(&subject("someone-else")),
            "listing sessions must not open their histories"
        );
    }

    #[test]
    fn an_owner_reads_their_own_event_log_without_a_privileged_grant() {
        let c = Caller::new(
            subject("a"),
            Authorization::from_capabilities([Capability::UseOwnSessions]),
        );
        assert!(c.may_read_event_log_owned_by(&subject("a")));
        assert!(!c.may_read_event_log_owned_by(&subject("b")));
    }

    #[test]
    fn the_event_log_grant_is_what_opens_another_subjects_history() {
        let c = Caller::new(
            subject("admin"),
            Authorization::from_capabilities([Capability::ReadEventLog]),
        );
        assert!(c.may_read_event_log_owned_by(&subject("someone-else")));
    }

    #[test]
    fn reading_everything_does_not_imply_acting_on_it() {
        // The admin surface is a monitor, not a second pair of hands. A reader
        // holds no capability that mutates another subject's session.
        let admin = Authorization::from_capabilities([
            Capability::ReadAllSessions,
            Capability::ReadRuntimeInventory,
            Capability::ReadEventLog,
            Capability::ReadActivityLog,
            Capability::ReadUserDirectory,
        ]);
        assert!(!admin.allows(Capability::CreateSession));
        assert!(!admin.allows(Capability::TerminateOwnSession));
        assert!(!admin.allows(Capability::ControlAnySession));
        let reader = Caller::new(subject("admin"), admin);
        assert_eq!(
            reader.require_control_of(&subject("someone-else")),
            Err(CoreError::Forbidden),
            "watching every session must not confer stopping one"
        );
    }

    #[test]
    fn an_owner_may_control_their_own_session() {
        let c = Caller::new(
            subject("a"),
            Authorization::from_capabilities([Capability::TerminateOwnSession]),
        );
        assert!(c.require_any_control().is_ok());
        assert!(c.require_control_of(&subject("a")).is_ok());
    }

    #[test]
    fn an_owner_may_not_control_somebody_elses_session() {
        // The check that must never weaken: an ordinary caller stays confined
        // to their own work however the operator surface grows.
        let c = Caller::new(
            subject("a"),
            Authorization::from_capabilities([
                Capability::CreateSession,
                Capability::UseOwnSessions,
                Capability::TerminateOwnSession,
            ]),
        );
        assert_eq!(
            c.require_control_of(&subject("b")),
            Err(CoreError::Forbidden)
        );
    }

    #[test]
    fn an_operator_may_control_any_session_including_their_own() {
        let c = Caller::new(
            subject("operator"),
            Authorization::from_capabilities([Capability::ControlAnySession]),
        );
        assert!(c.require_any_control().is_ok());
        assert!(c.require_control_of(&subject("someone-else")).is_ok());
        // Their own too, even without the everyday grant: an operator that
        // could act on everyone but themselves would be a curiosity, not a
        // safety property.
        assert!(c.require_control_of(&subject("operator")).is_ok());
    }

    #[test]
    fn a_caller_with_no_control_grant_is_refused_before_anything_is_read() {
        // The point of the cheap half: a caller entitled to nothing is turned
        // away without a lookup, so an identifier cannot be probed by handing
        // it to this operation and watching which refusal comes back.
        let c = Caller::new(
            subject("a"),
            Authorization::from_capabilities([Capability::UseOwnSessions]),
        );
        assert_eq!(c.require_any_control(), Err(CoreError::Forbidden));
    }

    #[test]
    fn acting_for_another_is_recognised_by_subject() {
        let c = Caller::new(
            subject("operator"),
            Authorization::from_capabilities([Capability::ControlAnySession]),
        );
        assert!(c.is_acting_for_another(&subject("someone-else")));
        assert!(!c.is_acting_for_another(&subject("operator")));
    }
}
