//! The core's error vocabulary.
//!
//! Each variant names a distinct condition so that an interface can render it
//! in its own idiom. Interfaces translate; they must never re-derive the
//! distinction between, say, "not yours" and "does not exist".

use crate::size::InvalidSessionSize;
use crate::terminal::InvalidGeometry;
use crate::{InvalidPressurePolicy, OutOfBounds};

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

    /// The session is being carried onto a replacement runtime.
    ///
    /// **Transient.** The handover completes on its own, after which the same
    /// operation may be retried. Distinct from `SessionTerminated`: refusing
    /// input here prevents it from reaching the runtime being torn down.
    #[error("session rotating")]
    SessionRotating,

    /// Another client already holds the exclusive terminal attachment.
    #[error("already attached")]
    AlreadyAttached,

    /// A terminal geometry with a zero dimension was supplied.
    #[error("invalid geometry: {0}")]
    InvalidGeometry(#[from] InvalidGeometry),

    /// A pressure policy whose values would make its signal unreliable.
    #[error("invalid pressure policy: {0}")]
    InvalidPressurePolicy(#[from] InvalidPressurePolicy),

    /// A size this deployment will not provision.
    ///
    /// Distinct from `InvalidRequest`: the request is well formed and would be
    /// honoured by a deployment whose administrator had set different bounds.
    /// Distinct from `SessionLimitReached`: nothing is in conflict and stopping
    /// something else does not help. Retrying unchanged never succeeds.
    #[error("size out of bounds: {0}")]
    SizeOutOfBounds(#[from] OutOfBounds),

    /// This owner already holds as many live sessions as the deployment permits.
    ///
    /// Both numbers are fields so clients can act without parsing prose. This
    /// is neither `Forbidden`, which interfaces conceal as not-found, nor an
    /// invalid request: stopping or pausing work makes the same ask valid.
    #[error("{}", session_limit_message(*limit, *live))]
    SessionLimitReached { limit: u32, live: u32 },

    /// This host already contains its pinned maximum number of live threads.
    /// Checked while holding host allocation authority and before provider work.
    #[error("{live} of {limit} chat threads are already open for this owner")]
    ChatThreadLimit { limit: u32, live: u32 },

    /// This deployment has no room left for a session that size.
    ///
    /// Distinct from a per-owner limit: the caller is not over an allowance of
    /// their own and cannot necessarily clear this by stopping their own work.
    /// Distinct from `RuntimeUnavailable`: nothing is broken.
    #[error("no room for a session of {requested_cpu_millis}m CPU and {requested_memory_mib}MiB: {free_cpu_millis}m CPU and {free_memory_mib}MiB free")]
    DeploymentAtCapacity {
        requested_cpu_millis: u64,
        requested_memory_mib: u64,
        free_cpu_millis: u64,
        free_memory_mib: u64,
    },

    /// A session size with a zero figure was supplied.
    #[error("invalid session size: {0}")]
    InvalidSessionSize(#[from] InvalidSessionSize),

    /// The runtime backing this session is not reachable.
    ///
    /// Nothing answered. This is the only failure that means a session's
    /// runtime is gone, and it is what licenses replacing one.
    #[error("runtime unavailable: {0}")]
    RuntimeUnavailable(String),

    /// The runtime answered, and would not do what was asked.
    ///
    /// Deliberately not `RuntimeUnavailable`. A reply proves something is
    /// there to have sent it, so replacing the sandbox would destroy a working
    /// runtime — which is exactly what happened on every rollout until this
    /// was told apart: a bridge refused a second greeting, the refusal was
    /// read as absence, and a live session was rebuilt empty beside the pod
    /// still holding its conversation.
    #[error("runtime refused: {0}")]
    RuntimeRefused(String),

    /// The operation is not valid in the session's current state.
    #[error("{}", invalid_state_message(detail.as_deref()))]
    InvalidState { detail: Option<String> },

    /// Ordinary owner input is fenced by a Trigger's initial prompt.
    #[error("the initial Trigger prompt currently owns session input ({0:?})")]
    InitialPromptReserved(crate::trigger::PromptDeliveryState),

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

    /// The skill catalogue could not produce a complete answer.
    ///
    /// **Transient.** This includes transport failures and an upstream page
    /// walk whose cursor or bound cannot prove completeness. Distinct from a
    /// missing runtime: replacing a session's sandbox cannot repair the skill
    /// registry and would destroy working state for no reason.
    #[error("skill catalogue unavailable: {0}")]
    SkillCatalogUnavailable(String),

    /// The quote a caller accepted names a rate card no longer in force.
    ///
    /// Both ids are fields so an interface can tell the caller exactly what
    /// changed without parsing this sentence. This keys on cards, never on the
    /// settings revision: editing unrelated platform prose changes a revision
    /// and does not change a price.
    #[error("the price changed: quoted against rate card {quoted}, now {in_force}")]
    PriceChanged { quoted: String, in_force: String },

    /// A settings save would have edited or removed a rate that has already
    /// priced something.
    ///
    /// Distinct from `SettingsConflict`: reading the current revision and
    /// retrying the same edit will not make a sealed card mutable. The detail
    /// names the card and the part of its history the request tried to change.
    #[error("the price list is append-only: {0}")]
    PriceListNotAppendOnly(String),

    /// The platform's settings were changed by somebody else since this
    /// caller read them.
    ///
    /// Not a failure of theirs and not a failure of the service. Two people
    /// with the settings page open is the ordinary case for a value that
    /// changes rarely and is opened by whoever is asking about it, and a
    /// last-write-wins save would discard one of them silently. Its own
    /// variant so a client can tell "re-read and try again" from "this will
    /// not work", which are different things to say to somebody.
    #[error("the platform's settings changed since they were read: {0}")]
    SettingsConflict(String),

    /// The platform's settings could not be read or written.
    ///
    /// Distinct from a refusal: the caller may well be entitled and the
    /// service simply cannot answer. Retrying is the right response, which is
    /// why it is not conflated with anything that means "stop".
    #[error("the platform's settings are unavailable: {0}")]
    SettingsUnavailable(String),

    /// The deployment cannot currently provide the managed chat surface.
    ///
    /// One stable code with a typed cause lets interfaces distinguish a dark
    /// or unsupported deployment from transient provisioning/storage failure.
    #[error("chat unavailable: {0}")]
    ChatUnavailable(crate::ChatUnavailableCause),
}

impl CoreError {
    pub fn invalid_state() -> Self {
        Self::InvalidState { detail: None }
    }

    pub fn chat_lifecycle_managed(operation: &str) -> Self {
        Self::InvalidState {
            detail: Some(format!(
                "chat thread lifecycle is platform-managed; owner {operation} is not allowed"
            )),
        }
    }

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
            Self::SessionRotating => "session_rotating",
            Self::AlreadyAttached => "already_attached",
            Self::InvalidGeometry(_) => "invalid_geometry",
            Self::InvalidPressurePolicy(_) => "invalid_pressure_policy",
            Self::SizeOutOfBounds(_) => "size_out_of_bounds",
            Self::SessionLimitReached { .. } => "session_limit_reached",
            Self::ChatThreadLimit { .. } => "chat_thread_limit",
            Self::DeploymentAtCapacity { .. } => "deployment_at_capacity",
            Self::InvalidSessionSize(_) => "invalid_session_size",
            Self::RuntimeUnavailable(_) => "runtime_unavailable",
            Self::RuntimeRefused(_) => "runtime_refused",
            Self::InvalidState { .. } => "invalid_state",
            Self::InitialPromptReserved(_) => "initial_prompt_reserved",
            Self::CapabilityUnsupported(_) => "capability_unsupported",
            Self::InvalidRequest(_) => "invalid_request",
            Self::ServiceNotGranted(_) => "service_not_granted",
            Self::ServiceCatalogUnavailable(_) => "service_catalogue_unavailable",
            Self::SkillCatalogUnavailable(_) => "skill_catalogue_unavailable",
            Self::PriceChanged { .. } => "price_changed",
            Self::PriceListNotAppendOnly(_) => "price_list_not_append_only",
            Self::SettingsConflict(_) => "settings_conflict",
            Self::SettingsUnavailable(_) => "settings_unavailable",
            Self::ChatUnavailable(_) => "chat_unavailable",
        }
    }
}

fn invalid_state_message(detail: Option<&str>) -> &str {
    detail.unwrap_or("invalid state for operation")
}

fn session_limit_message(limit: u32, live: u32) -> String {
    if live == limit {
        format!(
            "{live} of {limit} sessions are already running for this owner; stop one before starting another"
        )
    } else {
        format!(
            "{live} sessions are already running for this owner and the limit is {limit}; nothing new starts until fewer than {limit} are running"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::TerminalGeometry;
    use crate::{MemoryPressurePolicy, SessionSize, SizeDimension};

    /// Every variant, and the compiler is what says so.
    ///
    /// The list below used to be maintained by hand and was already three
    /// variants short — `RuntimeRefused` and both settings failures were
    /// missing, so the uniqueness test passed while covering twelve of
    /// fifteen. A test that looks like a guard and is not one is worse than no
    /// test: it is the reason nobody checked.
    ///
    /// This match exists to fail compilation. A variant added to `CoreError`
    /// cannot reach `main` until somebody has come here and given it a sample,
    /// which is the same trick `wire_name` uses in `entitlements.rs` and for
    /// the same reason.
    fn every_variant() -> Vec<CoreError> {
        let all = vec![
            CoreError::SessionNotFound,
            CoreError::EnvironmentProfileNotFound,
            CoreError::Forbidden,
            CoreError::SessionTerminated,
            CoreError::SessionRotating,
            CoreError::AlreadyAttached,
            CoreError::InvalidGeometry(TerminalGeometry::new(0, 1).expect_err("zero width fails")),
            CoreError::InvalidPressurePolicy(
                MemoryPressurePolicy::new(800, 780, 5, 15, 120).expect_err("narrow band fails"),
            ),
            CoreError::SizeOutOfBounds(OutOfBounds {
                dimension: SizeDimension::CpuMillis,
                requested: 5_000,
                minimum: 250,
                maximum: 4_000,
            }),
            CoreError::SessionLimitReached { limit: 3, live: 3 },
            CoreError::ChatThreadLimit {
                limit: 10,
                live: 10,
            },
            CoreError::DeploymentAtCapacity {
                requested_cpu_millis: 1_000,
                requested_memory_mib: 1_024,
                free_cpu_millis: 500,
                free_memory_mib: 512,
            },
            CoreError::InvalidSessionSize(SessionSize::new(0, 512).expect_err("zero CPU fails")),
            CoreError::RuntimeUnavailable("x".into()),
            CoreError::RuntimeRefused("x".into()),
            CoreError::invalid_state(),
            CoreError::InitialPromptReserved(crate::PromptDeliveryState::DispatchFenced),
            CoreError::CapabilityUnsupported("terminal".into()),
            CoreError::InvalidRequest("x".into()),
            CoreError::ServiceNotGranted("api-github".into()),
            CoreError::ServiceCatalogUnavailable("down".into()),
            CoreError::SkillCatalogUnavailable("down".into()),
            CoreError::PriceChanged {
                quoted: "rate_old".into(),
                in_force: "rate_new".into(),
            },
            CoreError::PriceListNotAppendOnly("card rate_example changed field 'rate'".into()),
            CoreError::SettingsConflict("x".into()),
            CoreError::SettingsUnavailable("x".into()),
            CoreError::ChatUnavailable(crate::ChatUnavailableCause::Disabled),
        ];
        // Exhaustive on purpose. Adding a variant to `CoreError` breaks this
        // match, and the fix is to add it to the list above — which is the
        // whole point.
        for e in &all {
            match e {
                CoreError::SessionNotFound
                | CoreError::EnvironmentProfileNotFound
                | CoreError::Forbidden
                | CoreError::SessionTerminated
                | CoreError::SessionRotating
                | CoreError::AlreadyAttached
                | CoreError::InvalidGeometry(_)
                | CoreError::InvalidPressurePolicy(_)
                | CoreError::SizeOutOfBounds(_)
                | CoreError::SessionLimitReached { .. }
                | CoreError::ChatThreadLimit { .. }
                | CoreError::DeploymentAtCapacity { .. }
                | CoreError::InvalidSessionSize(_)
                | CoreError::RuntimeUnavailable(_)
                | CoreError::RuntimeRefused(_)
                | CoreError::InvalidState { .. }
                | CoreError::InitialPromptReserved(_)
                | CoreError::CapabilityUnsupported(_)
                | CoreError::InvalidRequest(_)
                | CoreError::ServiceNotGranted(_)
                | CoreError::ServiceCatalogUnavailable(_)
                | CoreError::SkillCatalogUnavailable(_)
                | CoreError::PriceChanged { .. }
                | CoreError::PriceListNotAppendOnly(_)
                | CoreError::SettingsConflict(_)
                | CoreError::SettingsUnavailable(_)
                | CoreError::ChatUnavailable(_) => {}
            }
        }
        all
    }

    #[test]
    fn every_variant_has_a_distinct_code() {
        let codes: Vec<&'static str> = every_variant().iter().map(CoreError::code).collect();
        let mut sorted = codes.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), codes.len(), "codes must be unique: {codes:?}");
        // And none of them blank, which a hand-written arm can silently be.
        assert!(codes.iter().all(|c| !c.is_empty()), "{codes:?}");
    }

    #[test]
    fn a_price_list_refusal_names_the_card_and_field_that_changed() {
        let error = CoreError::PriceListNotAppendOnly(
            "card rate_example changed field 'effective_from'".into(),
        );
        let rendered = error.to_string();
        assert!(rendered.contains("rate_example"), "got {rendered}");
        assert!(rendered.contains("effective_from"), "got {rendered}");
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
        assert_ne!(
            CoreError::SkillCatalogUnavailable("down".into()).code(),
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

    #[test]
    fn invalid_size_converts_from_the_size_error() {
        let source = SessionSize::new(500, 0).expect_err("zero memory fails");
        let err: CoreError = source.into();
        assert_eq!(err.code(), "invalid_session_size");
    }

    #[test]
    fn a_limit_refusal_explains_at_limit_and_already_over_differently() {
        assert_eq!(
            CoreError::SessionLimitReached { limit: 3, live: 3 }.to_string(),
            "3 of 3 sessions are already running for this owner; stop one before starting another"
        );
        assert_eq!(
            CoreError::SessionLimitReached { limit: 3, live: 4 }.to_string(),
            "4 sessions are already running for this owner and the limit is 3; nothing new starts until fewer than 3 are running"
        );
    }
}
