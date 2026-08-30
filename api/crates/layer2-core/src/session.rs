//! Session lifecycle, recovery state, and the stop-reason vocabulary.

use serde::{Deserialize, Serialize};

use crate::ids::{EnvironmentProfileId, KernelId, SessionId, SubjectId};
use crate::options::AgentOptions;
use crate::terminal::TerminalGeometry;

/// Exact non-secret authority of one credential acknowledged by a session runtime.
///
/// The custody generation is assigned when CMA commits the provider key into
/// current or pending custody and remains unchanged when a pending key is
/// promoted. Provider key values and OAuth bearer material are deliberately not
/// represented here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCredentialAuthority {
    pub session_id: SessionId,
    pub custody_generation: u64,
    pub provider_key_id: String,
    pub provider_state_version: i64,
    pub provider_expires_at_epoch_milliseconds: u64,
    pub grant_fingerprint: String,
}

impl SessionCredentialAuthority {
    /// Reject incomplete, expired, or non-canonical authority evidence.
    pub fn is_valid_at(&self, now_epoch_milliseconds: u64) -> bool {
        !self.session_id.0.trim().is_empty()
            && self.custody_generation != 0
            && !self.provider_key_id.trim().is_empty()
            && self.provider_state_version > 0
            && self.provider_expires_at_epoch_milliseconds > now_epoch_milliseconds
            && self.grant_fingerprint.len() == 64
            && self
                .grant_fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }

    /// Whether this is the same credential, or a later credential with the
    /// identical provider grant committed by a later custody CAS.
    pub fn is_same_or_successor_of(&self, previous: &Self) -> bool {
        self == previous
            || (self.session_id == previous.session_id
                && self.custody_generation > previous.custody_generation
                && self.provider_key_id != previous.provider_key_id
                && self.grant_fingerprint == previous.grant_fingerprint)
    }
}

/// Durable provenance for a session created by one CMA Trigger Launch.
///
/// This is control-plane state, not part of the public [`Session`] projection.
/// It carries only identities and integrity metadata; the object reference is
/// never a credential and is resolved through an owner- and Launch-scoped
/// machine port before each runtime is provisioned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerSessionOrigin {
    pub launch_id: crate::trigger::LaunchId,
    pub revision_digest: String,
    pub sandbox_profile: String,
    pub github_service_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_catalog_service_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_user_service_slug: Option<String>,
    pub workspace_epoch: u64,
    pub source_revision: String,
    pub manifest_digest: String,
    pub object_reference: String,
    pub archive_sha256: String,
    /// Exact provider lifetime reviewed for this Launch. Runtime replacement
    /// reuses this value rather than a mutable deployment default.
    pub timeout_seconds: u32,
    /// Commercial policy and exact capacity terms accepted at Start.
    pub pricing_policy_reference: String,
    pub billing_mode: crate::BillingMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing: Option<crate::SessionPricing>,
}

impl TriggerSessionOrigin {
    pub fn github_binding(&self) -> Result<(&str, &str, &str), crate::TriggerValidationError> {
        let catalog_service_id = self.github_catalog_service_id.as_deref().ok_or_else(|| {
            crate::TriggerValidationError::field(
                "github_catalog_service_id",
                "is required for fresh Trigger GitHub authority",
            )
        })?;
        let user_service_slug = self.github_user_service_slug.as_deref().ok_or_else(|| {
            crate::TriggerValidationError::field(
                "github_user_service_slug",
                "is required for fresh Trigger GitHub authority",
            )
        })?;
        require_opaque_id("github_service_id", &self.github_service_id)?;
        require_opaque_id("github_catalog_service_id", catalog_service_id)?;
        if user_service_slug.is_empty()
            || user_service_slug.len() > 80
            || user_service_slug.starts_with('-')
            || user_service_slug.ends_with('-')
            || !user_service_slug
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(crate::TriggerValidationError::field(
                "github_user_service_slug",
                "is not a canonical NyxID service slug",
            ));
        }
        Ok((
            self.github_service_id.as_str(),
            catalog_service_id,
            user_service_slug,
        ))
    }

    /// Stable session identity for this Launch, shared by every retry.
    pub fn session_id(&self) -> Result<SessionId, crate::TriggerValidationError> {
        trigger_session_id(&self.launch_id)
    }

    /// Stable contributor-owned environment profile for this Launch.
    pub fn environment_profile_id(
        &self,
    ) -> Result<EnvironmentProfileId, crate::TriggerValidationError> {
        Ok(EnvironmentProfileId(format!(
            "envp_trg_{}",
            self.launch_uuid()?.simple()
        )))
    }

    pub fn validate(&self) -> Result<(), crate::TriggerValidationError> {
        self.launch_uuid()?;
        require_hex("revision_digest", &self.revision_digest, 64)?;
        require_text("sandbox_profile", &self.sandbox_profile, 128)?;
        require_text("github_service_id", &self.github_service_id, 255)?;
        if self.workspace_epoch == 0 {
            return Err(crate::TriggerValidationError::field(
                "workspace_epoch",
                "must be positive",
            ));
        }
        require_hex("source_revision", &self.source_revision, 40)?;
        require_hex("manifest_digest", &self.manifest_digest, 64)?;
        require_text("object_reference", &self.object_reference, 2_048)?;
        if self.timeout_seconds == 0 || self.timeout_seconds > 86_400 {
            return Err(crate::TriggerValidationError::field(
                "timeout_seconds",
                "must be between one second and one day",
            ));
        }
        require_text(
            "pricing_policy_reference",
            &self.pricing_policy_reference,
            255,
        )?;
        match self.billing_mode {
            crate::BillingMode::ZeroCost | crate::BillingMode::Sponsored
                if self.pricing.is_none() => {}
            crate::BillingMode::Contributor if self.pricing.is_some() => {}
            _ => {
                return Err(crate::TriggerValidationError::field(
                    "pricing",
                    "does not match the Trigger billing mode",
                ));
            }
        }
        require_hex("archive_sha256", &self.archive_sha256, 64)
    }

    fn launch_uuid(&self) -> Result<uuid::Uuid, crate::TriggerValidationError> {
        uuid::Uuid::parse_str(&self.launch_id.0)
            .map_err(|_| crate::TriggerValidationError::field("launch_id", "is not a UUID"))
    }
}

fn require_opaque_id(
    field: &'static str,
    value: &str,
) -> Result<(), crate::TriggerValidationError> {
    if value.is_empty()
        || value.len() > 256
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(crate::TriggerValidationError::field(field, "is not usable"));
    }
    Ok(())
}

/// Stable session identity reserved for one Trigger Launch.
///
/// This depends only on the Launch UUID, so terminal credential retirement can
/// fence a late or response-lost session setup even before `Launch.session` is
/// committed. It does not imply that a session or runtime ever existed.
pub fn trigger_session_id(
    launch_id: &crate::trigger::LaunchId,
) -> Result<SessionId, crate::TriggerValidationError> {
    let launch_uuid = uuid::Uuid::parse_str(&launch_id.0)
        .map_err(|_| crate::TriggerValidationError::field("launch_id", "is not a UUID"))?;
    Ok(SessionId(format!("sesn_trg_{}", launch_uuid.simple())))
}

fn require_text(
    field: &'static str,
    value: &str,
    maximum: usize,
) -> Result<(), crate::TriggerValidationError> {
    if value.trim().is_empty() || value.len() > maximum {
        return Err(crate::TriggerValidationError::field(
            field,
            format!("must contain between 1 and {maximum} bytes"),
        ));
    }
    Ok(())
}

fn require_hex(
    field: &'static str,
    value: &str,
    length: usize,
) -> Result<(), crate::TriggerValidationError> {
    if value.len() != length
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(crate::TriggerValidationError::field(
            field,
            format!("must be {length} lowercase hexadecimal characters"),
        ));
    }
    Ok(())
}

/// A named step in bringing a session up.
///
/// Ordered, and each knows its position, so a client can show how far along
/// something is without hard-coding the sequence — adding a step here changes
/// what every client displays rather than requiring each to be taught.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvisioningStage {
    /// The request arrived and the session exists.
    Accepted,
    /// A runtime has been asked for.
    RuntimeRequested,
    /// The provider gave us one.
    RuntimeCreated,
    /// The session's skills are being put on its disk.
    ///
    /// Its own step because it is the one a person waits on that is neither
    /// the provider nor the agent: skills are fetched and written before the
    /// agent starts, and a session that appeared to hang here used to look
    /// like a slow sandbox.
    SkillsInstalled,
    /// Connecting to its terminal.
    Attaching,
    /// Usable.
    Ready,
}

impl ProvisioningStage {
    /// Every stage, in the order they happen.
    pub const ALL: [Self; 6] = [
        Self::Accepted,
        Self::RuntimeRequested,
        Self::RuntimeCreated,
        Self::SkillsInstalled,
        Self::Attaching,
        Self::Ready,
    ];

    /// How far through the sequence this stage sits, counting from one.
    pub fn position(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0) + 1
    }

    /// How many stages there are in total.
    pub fn total() -> usize {
        Self::ALL.len()
    }
}

/// Lifecycle state of a session. `Idle` ordinarily means the runtime was
/// reclaimed while the session remains valid and revivable. The one exception
/// is `Idle` with [`StopReason::RequiresAction`]: that is a turn parked on its
/// owner while the runtime stays blocked on the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// Runtime is being provisioned.
    Provisioning,
    /// Runtime is up and accepting work.
    Ready,
    /// A turn is in progress.
    Running,
    /// Being carried onto a different runtime.
    ///
    /// Neither usable nor stopped. The session is intact and will come back on
    /// its own, but the runtime under it is being replaced, so anything typed
    /// now would go to a sandbox that is about to stop existing. Distinct from
    /// `Provisioning` because nothing was asked for and nothing is starting
    /// from nothing — there is a conversation being carried across.
    Rotating,
    /// No work is progressing. Usually reclaimed and revivable; an approval
    /// wait keeps its blocked runtime and is distinguished by its stop reason.
    Idle,
    /// Terminal state.
    Terminated,
}

impl SessionStatus {
    /// Every status that wants a runtime without consulting a stop reason.
    pub const WANTING_RUNTIME: [Self; 4] = [
        Self::Provisioning,
        Self::Ready,
        Self::Running,
        Self::Rotating,
    ];

    /// Whether no further work can occur under this session id.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Terminated)
    }

    /// Whether this session is mid-handover between runtimes.
    pub fn is_rotating(self) -> bool {
        matches!(self, Self::Rotating)
    }

    /// Whether this status normally has a runtime running for it.
    ///
    /// Session-level reconciliation uses [`Session::wants_runtime`] so it can
    /// distinguish a stopped idle session from an idle approval wait.
    pub fn wants_runtime(self) -> bool {
        matches!(
            self,
            Self::Provisioning | Self::Ready | Self::Running | Self::Rotating
        )
    }

    /// Whether a client should accept input for this session.
    ///
    /// Rotation is the case worth naming: the session is healthy, it is not
    /// stopped, and input must still be refused, because the runtime it would
    /// reach is being torn down.
    pub fn is_interactive(self) -> bool {
        matches!(self, Self::Ready | Self::Running)
    }

    /// Whether the owner can pick this session up again.
    ///
    /// Stopping is reversible and ending is not; a person needs to know which
    /// one they are looking at without having to remember what they did.
    pub fn is_revivable(self) -> bool {
        matches!(self, Self::Idle)
    }
}

/// Why a session stopped or paused.
///
/// `RequiresAction` is the trap: it means the runtime is waiting on the client
/// and is explicitly **not** terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The turn finished normally.
    EndTurn,
    /// The owner asked for termination.
    UserTerminated,
    /// Reclaimed after being idle too long.
    IdleTimeout,
    /// Hit the maximum lifetime.
    LifetimeCap,
    /// Waiting on the client for an approval or a tool result.
    RequiresAction,
    /// Credentials or authorization were withdrawn.
    Revoked,
    /// A spend or usage ceiling was reached.
    BudgetReached,
    /// Stopped by a failure.
    Error,
}

impl StopReason {
    /// Whether this reason means the session can never run again.
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::RequiresAction)
    }
}

/// Why a session's runtime stopped, as far as the service could establish it.
///
/// Distinct from [`StopReason`], which is why a *session* stopped or paused --
/// something its owner or its agent did. This is why the machine underneath it
/// went away, which is nobody's decision and usually nobody's fault.
///
/// The core's own vocabulary. It is mapped from what the runtime layer
/// reports, in the same shape `StartFailure::from_provisioning` already maps an
/// adapter error, so no layer below this one is named in the durable log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeLossCause {
    /// It was stopped for using more memory than it was allowed.
    OutOfMemory,
    /// It ran to the end and exited without reporting a failure.
    Ended,
    /// It exited reporting a failure.
    Failed,
    /// The machine under it went away.
    Displaced,
    /// Something was said about it and this build cannot read it.
    ///
    /// A different fact from saying nothing, and worth its own name: one means
    /// the record is silent, the other means the record has a word in it that
    /// this build has not been taught. Only the second is a thing to go and fix.
    Unrecognised,
}

impl From<layer3_adapter::RuntimeLossReason> for RuntimeLossCause {
    fn from(reason: layer3_adapter::RuntimeLossReason) -> Self {
        use layer3_adapter::RuntimeLossReason as R;
        match reason {
            R::OutOfMemory => Self::OutOfMemory,
            R::Completed => Self::Ended,
            R::Failed => Self::Failed,
            R::Displaced => Self::Displaced,
            R::Unrecognised => Self::Unrecognised,
        }
    }
}

/// Operational health, reported alongside lifecycle so operators read this
/// control plane the same way they read neighbouring ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryState {
    /// Nothing unusual.
    Normal,
    /// Reclaimed, revivable.
    Idle,
    /// Being restored. Routine during a revive.
    Recovering,
    /// Running with reduced capability.
    Degraded,
    /// The control plane cannot observe the runtime right now.
    Unknown,
    /// Permanently withdrawn from service.
    Retired,
    /// Configuration is not usable.
    Invalid,
}

impl RecoveryState {
    /// Whether a client should render this as a failure.
    ///
    /// `Recovering` and `Unknown` are routine: the first happens on every
    /// revive, the second means observation is temporarily unavailable. Neither
    /// means the session is broken, and painting them as errors is the single
    /// most common client bug this predicate exists to prevent.
    pub fn is_failure(self) -> bool {
        matches!(self, Self::Degraded | Self::Retired | Self::Invalid)
    }
}

/// A credential delegated by the caller while creating a session.
///
/// Browser credentials track the subject's renewable durable token. Bearer
/// credentials are request-bound and must remain pinned to this exact value
/// when the session is recovered later.
#[derive(Clone)]
pub struct CallerCredential {
    value: String,
    request_bound: bool,
}

impl CallerCredential {
    /// Keep this exact request credential for the lifetime of the session.
    pub fn request_bound(value: String) -> Self {
        Self {
            value,
            request_bound: true,
        }
    }

    /// Whether recovery must use this exact value rather than subject state.
    pub fn is_request_bound(&self) -> bool {
        self.request_bound
    }
}

impl From<String> for CallerCredential {
    fn from(value: String) -> Self {
        Self {
            value,
            request_bound: false,
        }
    }
}

impl From<&str> for CallerCredential {
    fn from(value: &str) -> Self {
        value.to_string().into()
    }
}

impl std::ops::Deref for CallerCredential {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl AsRef<str> for CallerCredential {
    fn as_ref(&self) -> &str {
        &self.value
    }
}

impl std::fmt::Debug for CallerCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "CallerCredential(<redacted {} bytes>, request_bound={})",
            self.value.len(),
            self.request_bound
        )
    }
}

/// What a caller asks for when creating a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSpec {
    /// How the agent should behave, chosen at creation and fixed for the
    /// session's life.
    ///
    /// Filled in from the profile the session names, like the kernel: what a
    /// caller sent here would be discarded, which is why a caller is not asked.
    pub options: AgentOptions,
    /// Terminal geometry to start the runtime at.
    #[serde(default)]
    pub geometry: Option<TerminalGeometry>,
    /// A reusable environment bundle to start the session with.
    ///
    /// Absent does not mean bare: what the platform mandates is applied either
    /// way, so this selects what to add rather than whether to have anything
    /// at all.
    #[serde(default)]
    pub environment_profile: Option<EnvironmentProfileId>,
    /// The caller's own credential, for asking the platform **as them**.
    ///
    /// Used to answer questions that depend on who is asking — which services
    /// they have connected, which skills they can see — and to mint the
    /// session's own key, which is done on their authority.
    ///
    /// **Never handed to the runtime.** It used to be: an interface put the
    /// caller's own access token here and provisioning, finding the field
    /// already filled, skipped minting entirely and gave the agent its owner's
    /// whole identity. Every interactively created session ran that way, while
    /// the machinery that mints a session-scoped key ran only for sessions the
    /// controller recovered — the one path a person never takes.
    ///
    /// The core stores and forwards this without interpreting it. It is never
    /// serialized back out, never logged, and never rendered by an interface.
    #[serde(skip)]
    pub caller_credential: Option<CallerCredential>,
    /// The session's own credential, minted for it alone.
    ///
    /// Filled by the core during provisioning and never by a caller — an
    /// interface that set this would be choosing what the agent may spend,
    /// which is the decision the mint exists to make. Scoped to what the
    /// session was granted, so a compromised agent reaches those services and
    /// nothing else its owner can.
    #[serde(skip)]
    pub runtime_credential: Option<String>,
    /// The plane the caller asked for, when they asked for one.
    ///
    /// The exception to the rule above, and it is narrow: everything in
    /// `options` describes how the *agent* behaves and belongs to the profile,
    /// so that two sessions started on one bundle are the same session twice.
    /// A plane is not that — it is how a person means to interact, which is
    /// the caller's business and their interface's.
    ///
    /// Honoured only when no profile was named; a profile that names a mode
    /// still decides. `None` means the caller expressed nothing and the
    /// deployment's own default applies, which is what an interface with no
    /// way to say sends.
    #[serde(default)]
    pub requested_mode: Option<crate::options::SessionMode>,
    /// The rate card the caller was quoted against, when they were quoted.
    ///
    /// `None` means the caller made no claim about a price and is not refused
    /// for one. Clients written before pricing existed therefore keep working.
    #[serde(default)]
    pub quoted_rate_card: Option<crate::RateCardId>,
}

/// A session and everything the core knows about it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// How this session's agent was configured.
    ///
    /// Persisted with the session rather than only being passed at creation,
    /// because reviving a stopped session has to produce the session someone
    /// had. Rebuilding it on defaults would silently swap the model out from
    /// under them.
    #[serde(default)]
    pub options: AgentOptions,
    /// Stable identifier.
    pub id: SessionId,
    /// The principal that owns it. Every operation is scoped to this subject.
    pub owner: SubjectId,
    /// Which runtime backs it.
    pub kernel: KernelId,
    /// Lifecycle position.
    pub status: SessionStatus,
    /// Operational health.
    pub recovery_state: RecoveryState,
    /// Why it stopped, when it has stopped.
    pub stop_reason: Option<StopReason>,
    /// Current terminal geometry.
    pub geometry: TerminalGeometry,
    /// Highest sequence number appended for this session.
    pub last_seq: u64,
    /// The environment bundle this session was started with, if any.
    ///
    /// Held with the session rather than only in the request that created it,
    /// for the same reason as its options: a runtime replaced later has to be
    /// built the same way, and a session that quietly lost its tools would be
    /// a different session wearing the same identifier.
    #[serde(default)]
    pub environment_profile: Option<EnvironmentProfileId>,
    /// Which runtime version is serving this session.
    ///
    /// The control plane knows which version it expects; this says which one
    /// the session actually has, and a session whose version is behind is
    /// carried onto the current one rather than left on an image nobody
    /// maintains any more.
    ///
    /// Absent on sessions created before versions were tracked, which is read
    /// as "not the current one" — the safe direction, since the alternative is
    /// leaving a session on an unknown image forever.
    #[serde(default)]
    pub runtime_version: Option<String>,
    /// How many times this session has been carried onto a newer runtime.
    #[serde(default)]
    pub rotations: u32,
    /// When the snapshot the current runtime resumed from was taken.
    ///
    /// Part of the session rather than bookkeeping about its runtime, because
    /// it is something its owner is told: it says the point their work was
    /// carried across from, so anything after it can be recognised as missing
    /// rather than puzzled over.
    #[serde(default)]
    pub rotated_at: Option<String>,
    /// What this session was granted when it was created.
    ///
    /// Held with the session for the same reason its options are: it decides
    /// what the session can be asked to do, and recomputing it later against a
    /// runtime whose abilities have since changed would silently make a live
    /// session into a different one.
    ///
    /// Empty on sessions recorded before this existed, and settled the first
    /// time such a session is placed or picked back up — `settle_capabilities`
    /// derives it against the runtime it actually has and writes it down.
    ///
    /// The empty state is therefore transient rather than safe. It was
    /// described as safe on the assumption that a session holding nothing
    /// simply claims nothing; in fact every gate that consults the set refuses,
    /// including the one behind each keystroke, so a session left empty is
    /// listed, openable and dead.
    #[serde(default)]
    pub capabilities: crate::capabilities::SessionCapabilities,
    /// What this session's credential may reach, decided when it was created.
    ///
    /// Held with the session rather than resolved from its profile whenever
    /// somebody asks. A profile can be edited after a session started from it,
    /// and the session keeps the scope it was provisioned with — so reading
    /// the profile to learn a session's reach gives the wrong answer precisely
    /// when it matters, which is after somebody has widened or narrowed the
    /// bundle a live session was started from.
    ///
    /// `None` on sessions recorded before this existed, which is not the same
    /// fact as an empty grant: the first is "we did not write it down" and the
    /// second is "this session may reach nothing". They are told apart because
    /// the first still has to fall back to its profile, and the second must
    /// not.
    #[serde(default)]
    pub granted_services: Option<Vec<crate::environment::ServiceId>>,
    /// The skills this session was given, settled when it was created.
    ///
    /// Recorded for the same reason its services are: the profile it came from
    /// can be edited afterwards and this session's tools do not follow. It is
    /// also the only honest answer to "what does this session have" — the
    /// profile says what a *new* session would get.
    ///
    /// `None` means the grant predates this field, not that the session has no
    /// skills; the two are told apart everywhere this is read.
    pub granted_skills: Option<Vec<crate::environment::GrantedSkill>>,
    /// What this session was priced at, settled when it was created.
    ///
    /// `None` is not zero and not free. It means this session was never priced,
    /// either because it predates pricing or because its deployment had no
    /// price list. That is a different fact from a rate of nothing, and the two
    /// are told apart everywhere this is read.
    ///
    /// The card id makes the charge auditable against the list; the copied rate
    /// makes it explainable when the list cannot be read. The list is platform
    /// state a migration can lose; this record is what the owner is billed
    /// against, so it carries both.
    #[serde(default)]
    pub pricing: Option<crate::SessionPricing>,
    /// Creation timestamp, RFC 3339 in UTC.
    pub created_at: String,
}

impl Session {
    /// Whether this session is entitled to keep a runtime right now.
    ///
    /// Most lifecycle states answer this from the status alone. An approval
    /// wait is the deliberate exception: it is rendered as idle because the
    /// turn is parked on its owner, but its runtime is still blocked on the
    /// answer and must survive reconciliation and control-plane restarts.
    pub fn wants_runtime(&self) -> bool {
        self.status.wants_runtime()
            || (self.status == SessionStatus::Idle
                && self.stop_reason == Some(StopReason::RequiresAction))
    }

    /// Whether `subject` may operate on this session.
    ///
    /// Ownership is the whole authorization model at this layer: there is no
    /// sharing, no delegation, and no role hierarchy in this version.
    pub fn is_owned_by(&self, subject: &SubjectId) -> bool {
        &self.owner == subject
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_action_is_not_terminal() {
        assert!(!StopReason::RequiresAction.is_terminal());
        assert!(StopReason::EndTurn.is_terminal());
        assert!(StopReason::Error.is_terminal());
    }

    #[test]
    fn routine_recovery_states_are_not_failures() {
        assert!(!RecoveryState::Recovering.is_failure());
        assert!(!RecoveryState::Unknown.is_failure());
        assert!(!RecoveryState::Normal.is_failure());
        assert!(!RecoveryState::Idle.is_failure());
    }

    #[test]
    fn genuinely_broken_states_are_failures() {
        assert!(RecoveryState::Degraded.is_failure());
        assert!(RecoveryState::Retired.is_failure());
        assert!(RecoveryState::Invalid.is_failure());
    }

    #[test]
    fn only_terminated_is_a_terminal_status() {
        assert!(SessionStatus::Terminated.is_terminal());
        for s in [
            SessionStatus::Provisioning,
            SessionStatus::Ready,
            SessionStatus::Running,
            SessionStatus::Rotating,
            SessionStatus::Idle,
        ] {
            assert!(!s.is_terminal(), "{s:?} must not be terminal");
        }
    }

    #[test]
    fn wanting_runtime_membership_matches_the_desired_state_predicate() {
        for status in [
            SessionStatus::Provisioning,
            SessionStatus::Ready,
            SessionStatus::Running,
            SessionStatus::Rotating,
            SessionStatus::Idle,
            SessionStatus::Terminated,
        ] {
            assert_eq!(
                SessionStatus::WANTING_RUNTIME.contains(&status),
                status.wants_runtime(),
                "membership and predicate disagree for {status:?}"
            );
        }
    }

    #[test]
    fn only_an_approval_wait_makes_an_idle_session_keep_its_runtime() {
        let mut session = Session {
            id: SessionId::generate(),
            owner: SubjectId("subject-a".into()),
            kernel: KernelId::new("example-runtime"),
            status: SessionStatus::Idle,
            recovery_state: RecoveryState::Normal,
            stop_reason: None,
            options: AgentOptions::default(),
            environment_profile: None,
            geometry: TerminalGeometry::default(),
            last_seq: 0,
            runtime_version: None,
            rotations: 0,
            rotated_at: None,
            capabilities: Default::default(),
            granted_services: None,
            granted_skills: None,
            pricing: None,
            created_at: "2026-01-01T00:00:00Z".into(),
        };

        assert!(
            !session.wants_runtime(),
            "an ordinarily idle session is stopped"
        );
        session.stop_reason = Some(StopReason::UserTerminated);
        assert!(
            !session.wants_runtime(),
            "a user-stopped session stays stopped"
        );
        session.stop_reason = Some(StopReason::RequiresAction);
        assert!(
            session.wants_runtime(),
            "the runtime waiting for an approval must remain entitled"
        );
    }

    #[test]
    fn ownership_is_exact_match_only() {
        let owner = SubjectId("subject-a".into());
        let session = Session {
            id: SessionId::generate(),
            owner: owner.clone(),
            kernel: KernelId::new("example-runtime"),
            status: SessionStatus::Ready,
            recovery_state: RecoveryState::Normal,
            stop_reason: None,
            options: AgentOptions::default(),
            environment_profile: None,
            geometry: TerminalGeometry::default(),
            last_seq: 0,
            runtime_version: None,
            rotations: 0,
            rotated_at: None,
            capabilities: Default::default(),
            granted_services: None,
            granted_skills: None,
            pricing: None,
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        assert!(session.is_owned_by(&owner));
        assert!(!session.is_owned_by(&SubjectId("subject-b".into())));
    }
}
