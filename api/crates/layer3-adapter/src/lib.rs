//! Layer 3 — the Agent Client Protocol, as kernel adapters speak it.
//!
//! The kernel conversation contract *is* ACP. The core acts as an ACP client;
//! each runtime presents as an ACP agent through its adapter. That is a
//! statement about this file rather than an aspiration for it: the update
//! stream, the permission request and the stop reasons below are the
//! protocol's own types, taken from its published schema so they cannot drift
//! from the specification.
//!
//! ACP does not describe everything a hosted runtime needs, so this layer adds
//! what it leaves out — bringing a runtime up, tearing one down, enumerating
//! them, capturing a conversation for resume, refreshing a credential, and the
//! terminal, which is a plane ACP has no notion of. Those are extensions
//! around the contract, not amendments to it.
//!
//! This file is the abstraction. It must contain no kernel-specific type,
//! constant, or conditional. Adding a kernel means adding a module, never
//! editing this trait to accommodate it.
//!
//! The layer above refers to all of this in the trait's own vocabulary —
//! conversation, update, permission request — and never by the protocol's
//! name. The core must outlive any particular protocol, which is why the name
//! stops here.

mod conversation;

pub use conversation::{
    ContentBlock, ContentChunk, ConversationChannel, ConversationId, ConversationInput,
    ConversationUpdate, MessageId, PermissionDecision, PermissionOption, PermissionOptionId,
    PermissionOptionKind, PermissionRequest, PromptResponse, RequestPermissionRequest,
    SessionUpdate, ToolCall, ToolCallId, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
    ToolKind, TurnStopReason, TurnUsage, UsageUpdate,
};

pub mod claude_code;
pub mod codex;
pub mod sandbox;
pub mod supervisor;
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

/// What a kernel can do, stated once rather than discovered by trying.
///
/// A capability found out by attempting it is a failure nobody can attribute:
/// the caller sees an error and cannot tell "this kernel never could" from
/// "this went wrong just now", and neither can the person they escalate to.
/// So each kernel declares its abilities up front, the core checks a session's
/// requested shape against them before placing it, and anything missing is
/// refused by name at creation — never degraded quietly into a session that
/// looks fine and cannot do what it was asked for.
///
/// Static on purpose. These describe the kernel, not the moment: a kernel that
/// can stream a conversation can always stream one, and a conversation that
/// fails right now is a failure, which is a different thing and reported as
/// one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KernelCapabilities {
    /// Emits structured conversation events — messages, tool calls, turns —
    /// rather than only bytes for a screen.
    pub conversation: bool,
    /// Renders a human-facing terminal.
    pub terminal: bool,
    /// Can capture its conversation so a session resumes it elsewhere.
    ///
    /// What makes a session survivable when its runtime is replaced. Read
    /// statically as well as attempted, so rotation can decline to move a
    /// session it already knows cannot be moved.
    pub snapshot: bool,
    /// Accepts input while a turn is in flight, without cancelling it first.
    pub steer: bool,
    /// Reports token usage as it accrues.
    pub usage: bool,
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

    /// What this kernel can do, whatever it happens to be doing now.
    ///
    /// Required rather than defaulted: a kernel that forgot to answer would
    /// otherwise advertise nothing and every session on it would be refused,
    /// or advertise everything and every refusal would arrive too late.
    /// Making it a decision the author must state is the point.
    fn capabilities(&self) -> KernelCapabilities;

    /// Bring up a runtime for a session.
    async fn provision(&self, spec: &RuntimeSpec) -> Result<RuntimeHandle, AdapterError>;

    /// Open a bidirectional attachment to the runtime's terminal.
    ///
    /// Defaults to unsupported, for the same reason `snapshot` and
    /// `list_runtimes` do: a kernel that renders no terminal is a kernel, not
    /// a broken one, and requiring every implementation to write a refusal by
    /// hand is how a stub ends up returning an empty screen instead.
    async fn attach_terminal(
        &self,
        _handle: &RuntimeHandle,
    ) -> Result<TerminalChannel, AdapterError> {
        Err(AdapterError::Unsupported(
            "this kernel does not render a terminal".to_string(),
        ))
    }

    /// Open a bidirectional attachment to the runtime's conversation.
    ///
    /// The other plane. Where the terminal carries bytes for a screen, this
    /// carries what the agent is actually doing — messages, tool calls, the
    /// decisions it is blocked on — in a form the core can record and every
    /// interface can project without parsing pixels.
    ///
    /// Callers check the `conversation` capability first, and a session
    /// without it never reaches here. The default refusal is the backstop for
    /// the case that check cannot cover: an adapter whose capabilities say one
    /// thing and whose implementation has not caught up.
    async fn open_conversation(
        &self,
        _handle: &RuntimeHandle,
    ) -> Result<ConversationChannel, AdapterError> {
        Err(AdapterError::Unsupported(
            "this kernel does not hold a conversation".to_string(),
        ))
    }

    /// Replace the credential the runtime presents when calling out.
    ///
    /// Required of every kernel, because every kernel calls out on somebody's
    /// behalf and every credential eventually expires. A runtime that could
    /// not be told about a renewal would have to be restarted to pick one up,
    /// and restarting it discards the conversation — which is the one thing a
    /// session exists to keep.
    ///
    /// Its consumer was always the supervisor's loopback proxy. It rode the
    /// terminal channel only because the terminal was the sole channel there
    /// was; a kernel with no terminal could not have been told at all.
    async fn refresh_credential(
        &self,
        handle: &RuntimeHandle,
        credential: &str,
    ) -> Result<(), AdapterError>;

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

    /// An adapter that implements only what the trait requires.
    ///
    /// Exists to exercise the defaults: everything a kernel need not do must
    /// answer "cannot" rather than fail, and a caller has to be able to tell
    /// those apart without reading an error string.
    struct BareKernel;

    #[async_trait::async_trait]
    impl KernelAdapter for BareKernel {
        fn kernel_name(&self) -> &'static str {
            "bare"
        }

        fn capabilities(&self) -> KernelCapabilities {
            KernelCapabilities::default()
        }

        async fn provision(&self, _spec: &RuntimeSpec) -> Result<RuntimeHandle, AdapterError> {
            Ok(RuntimeHandle("bare-1".into()))
        }

        async fn refresh_credential(
            &self,
            _handle: &RuntimeHandle,
            _credential: &str,
        ) -> Result<(), AdapterError> {
            Ok(())
        }

        async fn terminate(&self, _handle: &RuntimeHandle) -> Result<(), AdapterError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn every_optional_plane_defaults_to_saying_it_cannot() {
        // Absence of an ability is not a failure. A kernel that renders no
        // terminal, holds no conversation, cannot be captured, or cannot
        // enumerate what it runs is still a kernel — and a caller reading any
        // of those as a failure would retry forever, or tear down a live
        // session whose only fault is being one of them.
        let bare = BareKernel;
        let handle = RuntimeHandle("bare-1".into());
        for error in [
            bare.attach_terminal(&handle)
                .await
                .expect_err("no terminal"),
            bare.open_conversation(&handle)
                .await
                .expect_err("no conversation"),
            bare.snapshot(&handle).await.expect_err("no snapshot"),
            bare.list_runtimes().await.expect_err("cannot enumerate"),
        ] {
            assert!(
                matches!(error, AdapterError::Unsupported(_)),
                "expected an absent ability, got {error:?}"
            );
        }
    }

    #[test]
    fn the_conversation_channel_carries_the_protocol_own_vocabulary() {
        // The types on this plane are the protocol's, not a copy of them, so a
        // frame that round-trips through storage or a wire keeps its shape.
        let update = SessionUpdate::AgentMessageChunk(
            agent_client_protocol_schema::v1::ContentChunk::new(ContentBlock::from("hello")),
        );
        let json = serde_json::to_value(&update).expect("serialises");
        assert_eq!(
            json["sessionUpdate"], "agent_message_chunk",
            "the wire name is the protocol's: {json}"
        );
        let back: SessionUpdate = serde_json::from_value(json).expect("deserialises");
        assert!(matches!(back, SessionUpdate::AgentMessageChunk(_)));
    }

    #[test]
    fn a_turn_stop_reason_keeps_the_protocol_spelling() {
        // Five of them, and they are the protocol's own. Renaming one here
        // would change what a kernel is understood to have said.
        for (reason, wire) in [
            (TurnStopReason::EndTurn, "end_turn"),
            (TurnStopReason::MaxTokens, "max_tokens"),
            (TurnStopReason::MaxTurnRequests, "max_turn_requests"),
            (TurnStopReason::Refusal, "refusal"),
            (TurnStopReason::Cancelled, "cancelled"),
        ] {
            assert_eq!(
                serde_json::to_value(reason).expect("serialises"),
                serde_json::Value::String(wire.to_string())
            );
        }
    }

    #[test]
    fn a_permission_answer_names_the_request_it_answers() {
        // The correlation id is minted here because the protocol carries it at
        // the JSON-RPC layer, and the core needs something it can record now
        // and accept back minutes later, possibly after a restart.
        let input = ConversationInput::PermissionAnswer {
            id: "req-1".into(),
            decision: PermissionDecision::Abort,
        };
        match input {
            ConversationInput::PermissionAnswer { id, decision } => {
                assert_eq!(id, "req-1");
                assert_eq!(decision, PermissionDecision::Abort);
            }
            other => panic!("expected a permission answer, got {other:?}"),
        }
    }

    #[test]
    fn adapter_errors_carry_their_context() {
        let e = AdapterError::Unavailable("connection refused".into());
        assert!(e.to_string().contains("connection refused"));
    }
}
