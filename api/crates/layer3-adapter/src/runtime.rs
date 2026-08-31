//! What a runtime is, and what it is told when one is brought up.
//!
//! Split from the trait so that file stays within the project's size limit, on
//! a real division: the trait says what a kernel can be asked to do, and this
//! says what a runtime is made of and what it is given.
//!
//! Everything here is declared rather than taken from the core, because this
//! layer must not depend on the core — the dependency runs the other way. The
//! two are deliberately separate vocabularies that the caller maps between:
//! the core says what a person asked for, and this says what a runtime is
//! given.

use std::collections::BTreeMap;
use std::num::{NonZeroU32, NonZeroU64};

use tokio::sync::mpsc;

/// One credential delivery bound to its exact durable custody generation.
///
/// The value remains opaque outside the issuer and runtime proxy. The optional
/// generation is non-secret correlation metadata: sources without durable
/// provider-key custody leave it absent, while sponsored runtimes require it
/// so their bearer and model-gateway envelope can never describe different
/// credential generations.
#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeCredential {
    value: String,
    custody_generation: Option<NonZeroU64>,
}

impl RuntimeCredential {
    /// A credential whose source cannot prove a durable custody generation.
    pub fn new(value: String) -> Self {
        Self {
            value,
            custody_generation: None,
        }
    }

    /// A credential paired with the exact generation committed to custody.
    pub fn with_custody_generation(value: String, custody_generation: NonZeroU64) -> Self {
        Self {
            value,
            custody_generation: Some(custody_generation),
        }
    }

    /// Expose the opaque value only at a delivery or authentication boundary.
    pub fn expose(&self) -> &str {
        &self.value
    }

    /// Exact durable custody generation paired with this value, when known.
    pub fn custody_generation(&self) -> Option<NonZeroU64> {
        self.custody_generation
    }

    /// Consume this delivery while preserving only its opaque value.
    pub fn into_value(self) -> String {
        self.value
    }
}

impl From<String> for RuntimeCredential {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for RuntimeCredential {
    fn from(value: &str) -> Self {
        Self::new(value.to_string())
    }
}

impl From<&RuntimeCredential> for RuntimeCredential {
    fn from(value: &RuntimeCredential) -> Self {
        value.clone()
    }
}

impl std::fmt::Debug for RuntimeCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeCredential")
            .field(
                "value",
                &format_args!("<redacted {} bytes>", self.value.len()),
            )
            .field("custody_generation", &self.custody_generation)
            .finish()
    }
}

#[cfg(test)]
mod runtime_credential_tests {
    use super::*;

    #[test]
    fn credential_and_control_debug_are_redacted() {
        let credential = RuntimeCredential::with_custody_generation(
            "secret-runtime-bearer".into(),
            NonZeroU64::new(8).unwrap(),
        );
        let frame = TerminalInput::Credential {
            operation_id: "operation-1".into(),
            credential: credential.clone(),
        };

        for rendered in [format!("{credential:?}"), format!("{frame:?}")] {
            assert!(!rendered.contains("secret-runtime-bearer"));
            assert!(rendered.contains("custody_generation"));
        }
    }
}

/// Size of the runtime's terminal, in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermSize {
    /// Columns.
    pub cols: u16,
    /// Rows.
    pub rows: u16,
}

/// How much a runtime reserves from its provider.
///
/// Declared here rather than taken from the core for the reason everything in
/// this file is (see the module doc): the dependency runs the other way, and
/// the caller maps between the two vocabularies.
///
/// Numbers, never a provider's quantity syntax. Rendering these into the
/// strings a particular provider understands happens in exactly one place, in
/// the hosted kernel, which is the only code here that owns those words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeCapacity {
    /// Processor time, in thousandths of one core.
    pub cpu_millis: NonZeroU32,
    /// Working memory, in mebibytes.
    pub memory_mib: NonZeroU32,
}

/// Why a runtime stopped serving, in this layer's own words.
///
/// The provider says this in its own vocabulary; this is the closed set the
/// layers above are allowed to see. Mapping happens in `hosted/`, where the
/// provider's words already live, so no provider string crosses this boundary
/// -- CLAUDE.md section 3. A word this build does not know becomes
/// `Unrecognised` rather than travelling upward verbatim: an unrecognised word
/// is a fact worth reporting, and the word itself is the provider's business.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeLossReason {
    /// Stopped for using more memory than it was allowed.
    OutOfMemory,
    /// Ran to the end and exited without reporting a failure.
    Completed,
    /// Exited reporting a failure.
    Failed,
    /// The machine under it went away, or the platform took it back.
    Displaced,
    /// The provider named something this build does not recognise.
    Unrecognised,
}

/// Provider-visible runtime generation metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeGeneration {
    /// The metadata is absent because the runtime predates generation fencing.
    Legacy,
    /// A valid non-zero durable generation.
    Recorded(NonZeroU64),
    /// Metadata was present but was zero, malformed, or outside the range.
    Malformed,
}

/// One runtime the provider is running, as it describes it.
///
/// Carries who created it as well as what it is for. A provider is shared: one
/// key can see everything provisioned against it, including runtimes belonging
/// to a different control plane entirely. Without that distinction a second
/// control plane — a staging one, one mid-rollout, one being verified — reads
/// every session it has never heard of as an orphan and tears it down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeListing {
    /// How to address this runtime.
    pub handle: RuntimeHandle,
    /// The session it was provisioned for, when it says.
    pub session: Option<String>,
    /// Shared chat host correlation, absent on ordinary per-session runtimes.
    pub chat_host: Option<String>,
    /// Durable generation of the runtime within its session, when recorded.
    ///
    /// Handles are provider-owned and opaque, so a handle alone cannot prove
    /// that a census row belongs to the authority a control plane last read.
    /// Legacy absence and malformed metadata remain distinct so neither can be
    /// guessed into current authority.
    pub generation: RuntimeGeneration,
    /// The control plane that created it, when it says.
    ///
    /// `None` means it predates this being recorded, which is a different
    /// thing from belonging to somebody else and is treated differently.
    pub created_by: Option<String>,
    /// Whether the provider says this runtime is still serving.
    ///
    /// Listed and serving are not the same thing, and conflating them is how a
    /// session survives its own sandbox: a sandbox whose container has exited
    /// is still a sandbox, so it is still listed, and a reconciliation that
    /// reads presence as liveness finds record and reality agreeing and does
    /// nothing. Observed on production — a session reported `running` for
    /// nineteen minutes after the kernel had OOM-killed its container.
    ///
    /// `false` is a positive statement that it has stopped, not an absence of
    /// news. It is safe to act on because provisioning **waits** for a sandbox
    /// to report itself running before recording it: anything a record points
    /// at was serving once, so not serving now means it stopped rather than
    /// that it has yet to start.
    pub serving: bool,
    /// Why it stopped, where the provider said and this build recognised it.
    ///
    /// `None` in three different situations and they are deliberately not told
    /// apart here: the runtime is still serving, the provider named nothing,
    /// or the adapter cannot answer this question at all. What they share is
    /// that there is no verdict to give, and inventing one would be worse than
    /// the absence.
    ///
    /// Never read while `serving` is true. The provider sets a reason on a
    /// healthy sandbox too, so reading it unconditionally would invent a death
    /// for every live runtime in the list.
    pub lost_because: Option<RuntimeLossReason>,
}

/// What the runtime should already have when the agent starts.
///
/// The caller states intent; how it is applied — environment entries, an
/// install step, a fetch — is the implementing adapter's business. Nothing
/// here interprets a value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeEnvironment {
    /// Variables to set for the agent process, by name.
    ///
    /// Values are opaque and frequently secret: an adapter forwards them and
    /// never logs one.
    pub variables: BTreeMap<String, String>,
    /// Software the runtime should carry, as the installer names it.
    pub packages: Vec<String>,
    /// Skill identifiers the runtime should load.
    pub skills: Vec<String>,
    /// Standing instructions the session's owner wrote for their own sessions.
    ///
    /// Appended to `RuntimeSpec::platform_briefing`, never merged into it: the
    /// two sources have different owners and neither may silently overwrite
    /// the other.
    pub agent_notes: String,
}

impl RuntimeEnvironment {
    /// Add these variables to a map an adapter controls, refusing collisions.
    ///
    /// The adapter's own entries always win. They are how the runtime finds its
    /// upstream, its proxy and its credential, so a caller who could overwrite
    /// one could redirect the agent's traffic — or the credential attached to
    /// it — somewhere of their choosing. A refusal is logged rather than
    /// silently applied, because a variable that vanishes without explanation
    /// is the kind of thing an operator hunts for twice.
    pub fn apply_to(&self, controlled: &mut BTreeMap<String, String>) {
        for (key, value) in &self.variables {
            if controlled.contains_key(key) {
                tracing::warn!(
                    variable = %key,
                    "environment variable refused: the runtime controls this name"
                );
                continue;
            }
            controlled.insert(key.clone(), value.clone());
        }
    }
}

/// What a caller asks for when provisioning a runtime.
#[derive(Debug, Clone)]
pub struct RuntimeSpec {
    /// Identifier of the session the runtime serves, for correlation only.
    pub session_id: String,
    /// Shared chat host owning this runtime, when it is not session-owned.
    ///
    /// Kept separate from `session_id` so reconciliation never mistakes one
    /// shared provider object for an orphaned per-session runtime.
    pub chat_host: Option<String>,
    /// Durable generation reserved for this runtime within the session.
    ///
    /// Written into provider-visible metadata. Reconciliation can therefore
    /// distinguish an obsolete runtime from the current authority even when a
    /// stale census and a newer session record name the same session.
    /// `None` is used only by a compatibility-stage control plane. A
    /// generation-aware provision must reserve a non-zero value first.
    pub generation: Option<NonZeroU64>,
    /// Durable reservation that authorised this provider create.
    ///
    /// Written beside the session and generation in provider-visible metadata.
    /// A process that dies after the provider accepted the create but before it
    /// registered the returned handle can therefore discover every sandbox
    /// created for the exact reservation, choose one candidate, and schedule
    /// any duplicate creates for fenced cleanup. Coordinated provisioning
    /// always supplies both this and `generation`; compatibility provisioning
    /// supplies neither.
    pub reservation_token: Option<String>,
    /// Geometry to allocate the terminal at.
    pub size: TermSize,
    /// How much this runtime should reserve, or `None` for the deployment's.
    ///
    /// **Not `size`.** That is the terminal's geometry, immediately above, and
    /// was here first. This is how large the box is; that is how large the
    /// screen inside it is, and conflating them would be a resize that changed
    /// somebody's bill.
    ///
    /// `None` is not zero and not a default invented here: it means this
    /// caller stated nothing about capacity, and the sandbox is shaped the way
    /// this deployment shapes every sandbox. That is what every caller sent
    /// before this field existed, so `None` reproduces today's behaviour
    /// exactly.
    pub capacity: Option<RuntimeCapacity>,
    /// Provider-enforced runtime lifetime, or `None` for the deployment's
    /// default. Trigger sessions pin this in their durable Launch authority.
    pub timeout_seconds: Option<u64>,
    /// Credential the runtime should present when calling out on the owner's
    /// behalf, if the caller has one to delegate.
    ///
    /// Opaque here: this layer forwards it and never inspects or logs it.
    pub subject_credential: Option<String>,
    /// How the agent should behave.
    ///
    /// Stated as intent rather than as a rendered configuration, so each
    /// adapter can express it in whatever its own runtime understands.
    pub options: RuntimeOptions,
    /// An earlier state of this session for the agent to resume from.
    ///
    /// Present when a session is being carried across runtimes rather than
    /// started. The bytes are the agent's own conversation as its snapshot
    /// produced them — opaque here and to the caller, since only the agent
    /// knows how to read its own history back.
    ///
    /// Passed through the control plane rather than fetched by the runtime, so
    /// the sandbox needs no storage credential of its own.
    pub restore_from: Option<Vec<u8>>,
    /// A complete, versioned workspace archive to verify and materialize
    /// before the agent starts.
    ///
    /// Opaque to the kernel adapter. The shared hosted supervisor wire owns
    /// the archive format and the bot pod independently verifies every field.
    pub workspace: Option<WorkspacePackage>,
    /// Identity of the control plane asking, recorded on the runtime.
    ///
    /// So that a provider shared between control planes can be told apart, and
    /// no control plane reclaims a runtime it did not create.
    pub created_by: Option<String>,
    /// Skills to put on the pod's disk before its agent starts.
    ///
    /// Carried as bytes rather than as names. A name is only useful to
    /// something that can resolve it, and the pod deliberately cannot: it
    /// holds no registry credential and knows no registry. The control plane
    /// resolves and this delivers, which is the same division the conversation
    /// carry already uses.
    ///
    /// Where they land is composed here, from the kernel's own root — the
    /// pod is told paths and writes them, and never learns what a skill is.
    pub skills: Vec<SkillPackage>,
    /// The deployment's standing instructions, already rendered for this
    /// session by the core that owns both the setting and its recorded grants.
    ///
    /// Distinct from `environment.agent_notes`: an administrator controls this
    /// document and the session's owner controls those notes. The adapter may
    /// compose them, but must never make one authority overwrite the other.
    pub platform_briefing: String,
    /// What the runtime should already carry when the agent starts.
    pub environment: RuntimeEnvironment,
    /// What this session's credential may reach, by name.
    ///
    /// Names only. A service identifier is not sensitive and a credential is,
    /// and nothing here may put one where a transcript or a log could pick it
    /// up — which is why this carries the list and never the token minted
    /// against it.
    ///
    /// Told to the agent rather than left to be inferred. An agent that does
    /// not know it may act on a service will reason about why it cannot, and
    /// produce a confident wrong explanation — the worst available failure,
    /// because it looks like an answer.
    ///
    /// Empty means this session reaches nothing, which is a real answer. It is
    /// not the same as the runtime never having been told, and the two are
    /// kept apart where this is rendered.
    pub reaches: Vec<String>,
    /// Whether this runtime must reject platform GitHub calls until it has an
    /// authenticated, launch-scoped policy.
    ///
    /// Provider-neutral intent: Layer 3 transports an opaque runtime policy
    /// and never interprets its provider-shaped contents.
    pub runtime_policy_required: bool,
    /// Whether model calls must remain fenced until CMA installs one exact
    /// sponsored grant authority on the live runtime.
    ///
    /// This is independent of `runtime_policy_required`: the latter governs
    /// contributor-owned service access, while this flag prevents the model
    /// proxy from using that contributor authority for sponsored spend.
    pub sponsored_model_required: bool,
    /// Whether this runtime must support preparing a Git-backed workspace
    /// source before any agent process starts.
    pub workspace_source_required: bool,
    /// Minimum number of independently supervised conversation processes the
    /// runtime must support, or `None` when the caller needs only the legacy
    /// single-conversation shape.
    ///
    /// A runtime asked for named conversation slots must also advertise the
    /// coherent workspace checkpoint capability. Shared chat hosts cannot be
    /// paused safely if their several writers cannot be fenced as one unit.
    pub conversation_slots_required: Option<u8>,
}

#[derive(Debug, Clone)]
pub struct WorkspacePackage {
    pub epoch: u64,
    pub source_revision: String,
    /// Immutable GitHub baseline used for all diffs in this Launch, even when
    /// the archive itself is a later complete checkpoint.
    pub baseline_manifest_digest: String,
    pub manifest_digest: String,
    pub archive_sha256: String,
    pub archive: Vec<u8>,
}

/// One fenced request to capture a complete repository workspace.
///
/// The workspace epoch identifies the Launch-owned writer authority. The
/// checkpoint version is monotonic within that epoch and makes an ambiguous
/// transport retry address the exact same sealed artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceCheckpointRequest {
    pub epoch: u64,
    pub version: u64,
    pub source_revision: String,
    pub baseline_manifest_digest: String,
    /// Include the prepared clone's Git administration files and worktree
    /// pointer files. Ordinary imported workspaces leave this false.
    pub include_git_metadata: bool,
    pub disposition: WorkspaceCheckpointDisposition,
}

/// Whether a coherent capture reopens the runtime or leaves its writers
/// fenced for the immediately following provider teardown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WorkspaceCheckpointDisposition {
    #[default]
    ResumeAfterCapture,
    HoldForTeardown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceWriterState {
    Resumed,
    HeldForTeardown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceEntryKind {
    Regular,
    Symlink,
}

/// Provider-neutral entry metadata returned with a sealed checkpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceEntryPackage {
    pub path: String,
    pub kind: WorkspaceEntryKind,
    pub mode: u32,
    pub size: u64,
    pub content_sha256: String,
    pub provider_object_id: Option<String>,
    pub symlink_target: Option<String>,
}

/// Complete, runtime-sealed repository state and its quiescence receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceCheckpointPackage {
    pub epoch: u64,
    pub version: u64,
    pub source_revision: String,
    pub manifest_digest: String,
    pub archive_sha256: String,
    pub entries: Vec<WorkspaceEntryPackage>,
    pub total_bytes: u64,
    pub quiesced_writers: u32,
    pub writer_state: WorkspaceWriterState,
    pub writer_resume_token: Option<String>,
    pub includes_git_metadata: bool,
    pub archive: Vec<u8>,
}

/// A Git repository the runtime should materialize beneath its workspace.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceSourceRequest {
    /// Credential-free HTTPS repository URL.
    #[serde(rename = "url")]
    pub repository_url: String,
    /// Branch, tag, or other Git selector to clone.
    pub reference: String,
    /// Single path component beneath the runtime workspace.
    pub directory: String,
    /// Runtime-generation-scoped authority used by coherent checkpoints.
    pub workspace_epoch: u64,
}

/// Verified immutable authority established by preparing one Git source.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceSourceReceipt {
    pub workspace_epoch: u64,
    pub source_revision: String,
    pub baseline_manifest_digest: String,
}

/// One independently writable worktree to add to a prepared workspace source.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceTreeRequest {
    pub name: String,
    pub base: String,
}

/// The runtime-resolved path of an added workspace tree.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceTreeReceipt {
    pub path: String,
}

/// Bounded result of one administrator-owned workspace setup command.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceSetupStep {
    pub command_index: usize,
    pub exit_code: i32,
    pub output_excerpt: String,
}

/// Results returned in execution order for a workspace setup sequence.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceSetupReceipt {
    pub steps: Vec<WorkspaceSetupStep>,
}

/// What a runtime should be told about how to behave.
///
/// Declared here rather than taken from the core, because this layer must not
/// depend on the core — the dependency runs the other way. The two are
/// deliberately separate vocabularies that the caller maps between: the core
/// says what a person asked for, and this says what a runtime is given.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeOptions {
    /// Which plane the runtime is being started for.
    ///
    /// Some kernels are a different program depending on the answer — a
    /// terminal application on one plane and a protocol bridge on the other —
    /// so a runtime cannot be brought up without it.
    pub mode: RuntimeMode,
    /// Model to select, or `None` for the deployment's default.
    pub model: Option<String>,
    /// How much deliberation to ask for, already in its wire form.
    pub reasoning_effort: Option<String>,
    /// How much the agent may do unattended.
    pub permission: RuntimePermission,
    /// Whether the agent authenticates with the owner's own vendor account
    /// instead of being handed a delegated credential.
    pub own_credential: bool,
    /// Whether this conversation must use a deployment-owned restricted
    /// service-gateway policy rather than ordinary interactive permissions.
    ///
    /// The adapter maps this provider-neutral requirement into its own named
    /// mode. It never means unrestricted filesystem or network access.
    pub restricted_service_gateway: bool,
    /// Directory supplied verbatim to the hosted conversation protocol.
    ///
    /// `None` preserves the adapter's deployment-wide workspace default.
    pub working_directory: Option<String>,
    /// Opaque supervisor slot identity for this conversation.
    ///
    /// This is the control plane's session id, not the agent protocol's
    /// conversation id in `resume_conversation`. It is needed before a new ACP
    /// session has a protocol id and keeps several bridges in one runtime
    /// independent. `None` retains the legacy unnamed slot.
    pub conversation_slot: Option<String>,
    /// A conversation to rejoin rather than begin.
    ///
    /// Set when a control plane is taking back over a runtime that kept
    /// running without it — a restart, not a rotation. The runtime still holds
    /// the conversation; what was lost is the control plane's knowledge of what
    /// it is called, which is why this is read from the record rather than
    /// remembered.
    ///
    /// A kernel that cannot rejoin fails rather than opening a second
    /// conversation: the first would still be running, the agent's context
    /// would be gone, and nothing would say so.
    pub resume_conversation: Option<String>,
}

/// Which plane a runtime is being started for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RuntimeMode {
    /// A human-facing terminal. The default, and what every runtime was.
    #[default]
    Terminal,
    /// A structured conversation.
    Conversation,
}

/// How much the agent may do without asking.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RuntimePermission {
    /// Ask before reaching outside the workspace. The safe default.
    #[default]
    Guarded,
    /// Work within the workspace unattended.
    Autonomous,
    /// No approvals and no confinement inside the sandbox.
    Unrestricted,
}

/// An opaque reference to a provisioned runtime.
///
/// The string is meaningful only to the adapter that minted it. Callers store
/// and return it; they must never parse it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeHandle(pub String);

/// What a runtime acknowledged after installing one opaque policy envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePolicyReceipt {
    pub generation: u64,
    pub fence: u64,
    pub revoked: bool,
}

/// Canonical HTTPS origin of the CMA-owned sponsored model gateway.
///
/// An origin is carried explicitly during authority installation. It is never
/// inferred from another service URL, and it contains no path on which URL
/// resolution could silently change the selected gateway route.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct SponsoredModelGatewayOrigin(String);

impl SponsoredModelGatewayOrigin {
    pub fn parse(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        let parsed = url::Url::parse(&value).map_err(|_| "gateway origin is not a URL")?;
        if parsed.scheme() != "https"
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.path() != "/"
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || parsed.origin().ascii_serialization() != value
        {
            return Err("gateway origin must be one exact canonical HTTPS origin");
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Provider-neutral authority installed only in a sponsored runtime's local
/// model proxy. Every field is non-secret; the separate session credential is
/// held by the supervisor and supplied independently at request time.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SponsoredModelRuntimeAuthority {
    pub version: u32,
    pub gateway_origin: SponsoredModelGatewayOrigin,
    pub runtime_audience: String,
    pub session_id: String,
    pub session_credential_generation: u64,
    pub launch_id: String,
    pub grant_id: String,
    pub grant_generation: u64,
    pub policy_generation: u64,
    pub maximum_output_tokens_per_request: u64,
    pub expires_at: String,
    pub capability_fingerprint: String,
}

pub const SPONSORED_MODEL_RUNTIME_AUTHORITY_VERSION: u32 = 2;
/// The single policy resource represented by the sponsored Responses data plane.
pub const SPONSORED_MODEL_RESOURCE: &str = "responses";

/// Exact acknowledgement returned by the runtime after installing sponsored
/// model authority. The control plane compares every field to its command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SponsoredModelAuthorityReceipt {
    pub runtime_audience: String,
    pub session_id: String,
    pub session_credential_generation: u64,
    pub launch_id: String,
    pub grant_id: String,
    pub grant_generation: u64,
    pub policy_generation: u64,
    pub maximum_output_tokens_per_request: u64,
    pub capability_fingerprint: String,
}

/// Closed runtime refusal vocabulary exposed by the policy readiness lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimePolicyRefusal {
    MissingPolicy,
    UnknownVersion,
    Expired,
    StaleGeneration,
    Revoked,
    MalformedRequest,
    ResourceMismatch,
    OperationDenied,
    AudienceMismatch,
    SignatureInvalid,
}

/// Non-secret policy state observed from one exact live runtime.
///
/// The runtime handle is deliberately not repeated here. The adapter method
/// is already addressed to one opaque handle and Layer 2 pairs the returned
/// snapshot with that handle while holding the session lifecycle fence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePolicyObservation {
    pub required: bool,
    pub supported: bool,
    pub installed: bool,
    pub revoked: bool,
    pub launch_id: Option<String>,
    pub generation: Option<u64>,
    pub fence: Option<u64>,
    pub expires_at_epoch_seconds: Option<u64>,
    pub last_refusal: Option<RuntimePolicyRefusal>,
}

/// Version of the runtime-side restricted gateway probe protocol.
pub const RUNTIME_GATEWAY_PROBE_VERSION: u32 = 2;

/// One non-secret target for a runtime-side restricted gateway read.
///
/// Layer 3 carries this provider-neutral shape to the exact runtime. The
/// runtime owns the concrete loopback route and never returns its response
/// body, credential, or headers to the control plane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeGatewayProbeRequest {
    pub version: u32,
    pub service_id: String,
    pub catalog_service_id: String,
    pub user_service_slug: String,
    pub resource_id: u64,
}

/// Closed result of traversing the command sandbox and restricted gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeGatewayProbeOutcome {
    Allowed,
    CommandSandboxInitializationFailed,
    GatewayNetworkInitializationFailed,
    PolicyDenied,
    ProviderDenied,
    TransientBrokerFailure,
}

/// A runtime acknowledgement that contains no provider response data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeGatewayProbeReceipt {
    pub version: u32,
    pub outcome: RuntimeGatewayProbeOutcome,
}

/// Exact provider identity required for a fenced destructive operation.
///
/// Handles are opaque and generations are only monotonic within one session,
/// so neither is globally unique. Pairing both with the provider-visible
/// session owner lets an adapter refuse a stale teardown atomically instead of
/// deleting whichever runtime owns the handle now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeIdentity {
    /// Opaque provider handle.
    pub handle: RuntimeHandle,
    /// Session ownership expected in provider-visible metadata.
    pub session: String,
    /// Durable generation expected in provider-visible metadata.
    ///
    /// `None` means the generation metadata must be absent. It is an exact
    /// legacy comparison, never permission to ignore recorded metadata. A
    /// caller may request it only after a monotonic protocol promotion proves
    /// that no legacy writer can create another absent-generation runtime.
    pub generation: Option<NonZeroU64>,
}

/// Provider-visible identity of one durable runtime reservation.
///
/// A provider create is not transactional with the session store. This key is
/// consequently written at create time and used after a crash to find every
/// sandbox that may have been produced before a handle was registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeReservationIdentity {
    /// Session for which the generation was reserved.
    pub session: String,
    /// Never-reused generation reserved before provider creation.
    pub generation: NonZeroU64,
    /// Exact reservation token stored durably before provider creation.
    pub token: String,
}

/// Something sent toward the runtime's terminal.
#[derive(Debug, Clone)]
pub enum TerminalInput {
    /// Raw bytes written to the terminal, as if typed.
    Bytes(Vec<u8>),
    /// The terminal was resized.
    Resize(TermSize),
    /// A named request the supervisor translates into one terminal control byte.
    Signal(String),
    /// Replace the credential the runtime presents when calling out.
    ///
    /// Sent when the owner's credential is refreshed. Delivering it over the
    /// same channel as terminal traffic is what lets the agent keep running:
    /// restarting it to pick up a new credential would discard the session's
    /// context, which is the thing a terminal exists to preserve.
    Credential {
        /// Opaque request correlation generated by the control plane.
        operation_id: String,
        /// Redacted bearer and its exact non-secret custody generation.
        credential: RuntimeCredential,
    },
    /// Install one opaque authenticated policy envelope or tombstone.
    RuntimePolicy {
        operation_id: String,
        envelope: String,
    },
    /// Install one exact sponsored model authority in the supervisor's local
    /// proxy. The authority is inert without the independently delivered
    /// session credential and contains no provider credential or grant handle.
    SponsoredModelAuthority {
        operation_id: String,
        authority: SponsoredModelRuntimeAuthority,
    },
    /// Restart the supervised program under a different front-end.
    ///
    /// The command is a kernel's own front-end name, and the supervisor only
    /// accepts one it was told at boot it may run.
    SwitchPlane { command: String },
}

/// What a runtime is using, as this layer reports it.
///
/// Levels and totals are carried through from the current supervisor frame;
/// rates are derived against the previous frame on the same link. Every rate
/// is absent on the first sample of a link, including the first one after a
/// runtime rotation.
///
/// Every field is optional. A level may be unreadable, a limit may not exist,
/// and a rate may have no sound predecessor. None of those absences is zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceSample {
    /// Processor use over `interval_ms`, in thousandths of one core.
    pub cpu_millis_used: Option<u32>,
    /// The processor allowance in thousandths of one core.
    pub cpu_millis_limit: Option<u32>,
    /// Memory in use at the instant of the sample, in bytes.
    pub memory_bytes_used: Option<u64>,
    /// The memory allowance in bytes.
    pub memory_bytes_limit: Option<u64>,
    /// The highest recorded memory use, in bytes.
    pub memory_bytes_peak: Option<u64>,
    /// Thousandths of the interval in which every task was memory-stalled.
    pub stall_permille: Option<u16>,
    /// Times memory usage reached its configured ceiling.
    pub ceiling_hits: Option<u64>,
    /// Allocations refused since the sandbox started.
    pub allocation_failures: Option<u64>,
    /// Processes killed for exceeding the memory allowance.
    pub kills: Option<u64>,
    /// Bytes received since the sandbox started.
    pub received_bytes: Option<u64>,
    /// Bytes sent since the sandbox started.
    pub sent_bytes: Option<u64>,
    /// Received bytes per second over `interval_ms`.
    pub received_bytes_per_second: Option<u64>,
    /// Sent bytes per second over `interval_ms`.
    pub sent_bytes_per_second: Option<u64>,
    /// The interval over which the three rates were derived, in milliseconds.
    ///
    /// Absent when there is no strictly later predecessor; present whenever
    /// elapsed time is sound, even if an individual counter is unavailable.
    pub interval_ms: Option<u64>,
}

/// What arrived from a runtime's terminal.
///
/// The ending shares the byte stream's queue so its order is structural: every
/// byte sent before `Exited` is consumed before the ending can be observed.
/// A channel that closes without this variant reported no process status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalOutput {
    /// Bytes produced by the runtime.
    Bytes(Vec<u8>),
    /// The supervised process ended after all of its terminal output was read.
    Exited {
        /// Its numeric exit status, or `None` when it was ended by a signal.
        code: Option<i32>,
    },
}

/// A bidirectional attachment to a runtime's terminal.
#[derive(Debug)]
pub struct TerminalChannel {
    /// Ordered output and ending reports produced by the runtime.
    pub output: mpsc::Receiver<TerminalOutput>,
    /// Input destined for the runtime.
    pub input: crate::TerminalInputSender,
}

/// One skill, as bytes ready to be written.
///
/// The paths are relative to the skill's own directory, exactly as the
/// registry names them — `SKILL.md`, `references/api-reference.md` — so the
/// kernel's root is prefixed at delivery and nothing here depends on which
/// kernel is being served.
#[derive(Debug, Clone)]
pub struct SkillPackage {
    /// The directory this skill occupies, which is the name the agent invokes.
    pub name: String,
    /// Each file, relative to that directory.
    pub files: Vec<(String, Vec<u8>)>,
}
