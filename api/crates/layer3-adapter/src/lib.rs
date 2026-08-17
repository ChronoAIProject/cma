//! Layer 3 — ACP-shaped kernel adapters.
//!
//! This file is the abstraction. It must contain no kernel-specific type,
//! constant, or conditional. Adding a kernel means adding a module, never
//! editing this trait to accommodate it.

pub mod claude_code;
pub mod codex;
pub mod sandbox;
pub mod terminal;

use std::collections::BTreeMap;

use tokio::sync::mpsc;

/// Size of the runtime's terminal, in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermSize {
    /// Columns.
    pub cols: u16,
    /// Rows.
    pub rows: u16,
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
    /// What the runtime should already carry when the agent starts.
    pub environment: RuntimeEnvironment,
}

/// What a runtime should be told about how to behave.
///
/// Declared here rather than taken from the core, because this layer must not
/// depend on the core — the dependency runs the other way. The two are
/// deliberately separate vocabularies that the caller maps between: the core
/// says what a person asked for, and this says what a runtime is given.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeOptions {
    /// Model to select, or `None` for the deployment's default.
    pub model: Option<String>,
    /// How much deliberation to ask for, already in its wire form.
    pub reasoning_effort: Option<String>,
    /// How much the agent may do unattended.
    pub permission: RuntimePermission,
    /// Whether the agent authenticates with the owner's own vendor account
    /// instead of being handed a delegated credential.
    pub own_credential: bool,
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
}

/// A bidirectional attachment to a runtime's terminal.
#[derive(Debug)]
pub struct TerminalChannel {
    /// Output produced by the runtime.
    pub output: mpsc::Receiver<Vec<u8>>,
    /// Input destined for the runtime.
    pub input: mpsc::Sender<TerminalInput>,
}

/// Errors an adapter may surface.
#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    /// The runtime could not be provisioned or reached.
    #[error("runtime unavailable: {0}")]
    Unavailable(String),
    /// The runtime rejected the request.
    #[error("runtime rejected request: {0}")]
    Rejected(String),
    /// The provider cannot answer this question at all.
    ///
    /// Distinct from a failure: nothing went wrong, the capability is simply
    /// absent. Callers that would otherwise read a negative answer into silence
    /// need to be able to tell the two apart.
    #[error("not supported by this runtime: {0}")]
    Unsupported(String),
}

/// A kernel runtime, abstracted. Layer 2 depends on this trait and never on an
/// implementation of it.
#[async_trait::async_trait]
pub trait KernelAdapter: Send + Sync {
    /// Human-readable kernel name, for diagnostics only.
    fn kernel_name(&self) -> &'static str;

    /// Bring up a runtime for a session.
    async fn provision(&self, spec: &RuntimeSpec) -> Result<RuntimeHandle, AdapterError>;

    /// Open a bidirectional attachment to the runtime's terminal.
    async fn attach_terminal(
        &self,
        handle: &RuntimeHandle,
    ) -> Result<TerminalChannel, AdapterError>;

    /// Tear the runtime down.
    async fn terminate(&self, handle: &RuntimeHandle) -> Result<(), AdapterError>;

    /// Every runtime this adapter can see, with the session each was created
    /// for.
    ///
    /// Reconciliation needs the provider's own view rather than ours, because
    /// the runtimes worth finding are exactly the ones our records have lost
    /// track of. The session identifier is whatever was recorded at
    /// provisioning time and may be absent, in which case nothing links the
    /// runtime to a session and it is an orphan by definition.
    ///
    /// Capture the agent's conversation so it can be resumed elsewhere.
    ///
    /// The thing that makes a runtime replaceable. What it returns is the
    /// agent's own state in whatever form the agent resumes from, paired with
    /// `RuntimeSpec::restore_from`; nothing outside the adapter interprets it.
    ///
    /// `None` means the agent has nothing to resume from yet, which is not a
    /// failure — a session that has not been used has no conversation.
    ///
    /// Defaults to unsupported. An adapter that cannot snapshot can still run
    /// sessions; it simply cannot carry one across a runtime replacement, and
    /// a caller needs to know that rather than silently losing the history.
    async fn snapshot(&self, _handle: &RuntimeHandle) -> Result<Option<Vec<u8>>, AdapterError> {
        Err(AdapterError::Unsupported(
            "this runtime cannot capture its agent's conversation".to_string(),
        ))
    }

    /// Defaults to saying it cannot answer, rather than to an empty list. The
    /// difference matters more than it looks: an empty list is a statement that
    /// nothing is running, and a caller acting on that would tear down or
    /// replace every live session behind an adapter whose only fault is not
    /// implementing this.
    async fn list_runtimes(&self) -> Result<Vec<RuntimeListing>, AdapterError> {
        Err(AdapterError::Unsupported(
            "this runtime cannot enumerate what it is running".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_compare_by_value() {
        assert_eq!(RuntimeHandle("a".into()), RuntimeHandle("a".into()));
        assert_ne!(RuntimeHandle("a".into()), RuntimeHandle("b".into()));
    }

    #[test]
    fn adapter_errors_carry_their_context() {
        let e = AdapterError::Unavailable("connection refused".into());
        assert!(e.to_string().contains("connection refused"));
    }
}
