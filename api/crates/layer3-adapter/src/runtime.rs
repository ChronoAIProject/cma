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
use std::num::NonZeroU32;

use tokio::sync::mpsc;

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

/// Something sent toward the runtime's terminal.
#[derive(Debug, Clone)]
pub enum TerminalInput {
    /// Raw bytes written to the terminal, as if typed.
    Bytes(Vec<u8>),
    /// The terminal was resized.
    Resize(TermSize),
    /// An out-of-band signal, named in the conventional short form.
    Signal(String),
    /// Replace the credential the runtime presents when calling out.
    ///
    /// Sent when the owner's credential is refreshed. Delivering it over the
    /// same channel as terminal traffic is what lets the agent keep running:
    /// restarting it to pick up a new credential would discard the session's
    /// context, which is the thing a terminal exists to preserve.
    Credential(String),
    /// Restart the supervised program under a different front-end.
    ///
    /// The command is a kernel's own front-end name, and the supervisor only
    /// accepts one it was told at boot it may run.
    SwitchPlane { command: String },
}

/// A bidirectional attachment to a runtime's terminal.
#[derive(Debug)]
pub struct TerminalChannel {
    /// Output produced by the runtime.
    pub output: mpsc::Receiver<Vec<u8>>,
    /// Input destined for the runtime.
    pub input: mpsc::Sender<TerminalInput>,
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
