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
mod runtime;
mod sandbox_reclaimer;
mod task_retirement;
mod terminal_input;

pub use runtime::{
    ResourceSample, RuntimeCapacity, RuntimeEnvironment, RuntimeGeneration, RuntimeHandle,
    RuntimeIdentity, RuntimeListing, RuntimeLossReason, RuntimeMode, RuntimeOptions,
    RuntimePermission, RuntimePolicyReceipt, RuntimeReservationIdentity, RuntimeSpec, SkillPackage,
    TermSize, TerminalChannel, TerminalInput, TerminalOutput, WorkspaceCheckpointPackage,
    WorkspaceCheckpointRequest, WorkspaceEntryKind, WorkspaceEntryPackage, WorkspacePackage,
};
pub use terminal_input::TerminalInputSender;

use tokio::sync::mpsc;

pub use conversation::{
    AvailableCommand, AvailableCommandsUpdate, ContentBlock, ContentChunk, ConversationChannel,
    ConversationId, ConversationInput, ConversationInputSender, ConversationSendOutcome,
    ConversationUpdate, MessageId, PermissionDecision, PermissionLocator, PermissionOption,
    PermissionOptionId, PermissionOptionKind, PermissionRequest, PromptResponse,
    RequestPermissionRequest, SessionUpdate, ToolCall, ToolCallId, ToolCallStatus, ToolCallUpdate,
    ToolCallUpdateFields, ToolKind, TurnStopReason, TurnUsage, UsageUpdate,
};

pub mod claude_code;
pub mod codex;
pub mod dsh;
pub mod hosted;
pub mod layer4;
pub mod sandbox;
pub mod supervisor;
pub mod terminal;

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
    /// A turn in flight can be abandoned without ending the session.
    ///
    /// Separate from steering, which continues a turn rather than abandoning
    /// one. A kernel may have neither, either, or both: one wire offers a
    /// cancel and no mid-turn input, another the reverse. Claimed and absent,
    /// this is an operation a session accepts and silently does nothing with —
    /// the worst shape a refusal can take, because the person who asked
    /// watches the agent carry on.
    pub interrupt: bool,
    /// Reports token usage as it accrues.
    pub usage: bool,
    /// Reports what its sandbox is using: memory, processor time and traffic.
    ///
    /// This is a property of the supervisor image rather than of the agent
    /// inside it, and applies equally to terminal and conversation sessions.
    pub resources: bool,
}

/// Errors an adapter may surface.
///
/// **The variant is a fact about what was heard**, and callers depend on it:
/// a session's runtime is replaced on one of these and left alone on the
/// others, so a variant that is only approximately right costs somebody their
/// conversation in one direction or their session in the other.
#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    /// Nothing answered.
    ///
    /// A provider that could not be reached, a provider saying this sandbox is
    /// not there, a socket that would not open, a deadline that passed in
    /// silence, a link that closed under a waiting call. **This and only this
    /// is evidence that a runtime is gone**, and everything that is evidence of
    /// that has to arrive here.
    #[error("runtime unavailable: {0}")]
    Unavailable(String),
    /// Something answered, and refused.
    ///
    /// A reply came back. Whatever it says, the thing that sent it exists and
    /// is dispatching, so this is never on its own grounds for replacing a
    /// sandbox.
    #[error("runtime rejected request: {0}")]
    Rejected(String),
    /// Something answered, and refused for want of room.
    ///
    /// A reply came back, so this is never evidence a runtime is gone -- the
    /// same reasoning as `Rejected`, from which it is split for one reason:
    /// retrying it is pointless on the timescale a retry loop works at.
    /// Capacity is freed by somebody stopping something, not by waiting three
    /// seconds and asking again.
    #[error("no room for a runtime this size: {0}")]
    AtCapacity(String),
    /// The provider cannot answer this question at all.
    ///
    /// Distinct from a failure: nothing went wrong, the capability is simply
    /// absent. Callers that would otherwise read a negative answer into silence
    /// need to be able to tell the two apart.
    #[error("not supported by this runtime: {0}")]
    Unsupported(String),
}

impl AdapterError {
    /// The same failure, said with more context, and still the same kind.
    ///
    /// Rewording used to change the kind. Three call sites wrapped every
    /// failure of a follow-up call in `Rejected`, so a deadline and a closed
    /// link — both of them silence — reached the core as though the runtime
    /// had answered. Anything reading the variant to decide whether a sandbox
    /// is still there was reading a fact the wrapper had invented.
    #[must_use]
    pub fn context(self, what: &str) -> Self {
        match self {
            Self::Unavailable(m) => Self::Unavailable(format!("{what}: {m}")),
            Self::Rejected(m) => Self::Rejected(format!("{what}: {m}")),
            Self::AtCapacity(m) => Self::AtCapacity(format!("{what}: {m}")),
            Self::Unsupported(m) => Self::Unsupported(format!("{what}: {m}")),
        }
    }

    /// Whether this is evidence that the runtime is gone.
    ///
    /// Only silence is. A refusal proves the opposite — something was there to
    /// refuse — and a stated absence is a property of the image rather than of
    /// this instance, so another sandbox from it would say the same thing.
    #[must_use]
    pub fn is_runtime_loss(&self) -> bool {
        matches!(self, Self::Unavailable(_))
    }
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
    /// Takes the same options the runtime was provisioned with, because a
    /// conversation is opened with the same intent: how much the agent may do
    /// unattended is settled per conversation on some kernels rather than per
    /// runtime, and an adapter that had to remember it would forget across a
    /// control-plane restart — exactly when a session is being taken back over.
    ///
    /// Callers check the `conversation` capability first, and a session
    /// without it never reaches here. The default refusal is the backstop for
    /// the case that check cannot cover: an adapter whose capabilities say one
    /// thing and whose implementation has not caught up.
    ///
    /// The caller supplies the sink rather than receiving one back, because
    /// opening a conversation is not instantaneous and things worth recording
    /// happen inside it. An own-account session stops mid-open to ask its
    /// owner to sign in, and a receiver handed over afterwards could not carry
    /// that request until the sign-in it was asking for had already happened.
    /// A caller that is draining before it calls hears everything.
    async fn open_conversation(
        &self,
        _handle: &RuntimeHandle,
        _options: &RuntimeOptions,
        _updates: mpsc::Sender<ConversationUpdate>,
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

    /// Install one authenticated runtime policy and wait for the exact runtime
    /// generation to acknowledge it.
    ///
    /// The envelope is opaque at this boundary. Its signer, semantics and
    /// evaluator belong on opposite sides of Layer 3; this layer only carries
    /// it and reports the monotonic receipt the runtime returned.
    async fn install_runtime_policy(
        &self,
        _handle: &RuntimeHandle,
        _envelope: &str,
    ) -> Result<RuntimePolicyReceipt, AdapterError> {
        Err(AdapterError::Unsupported(
            "this runtime does not accept authenticated policy data".to_string(),
        ))
    }

    /// Tear the runtime down using provider authority captured when this call
    /// begins.
    ///
    /// This handle-only operation exists for compatibility stores. Coordinated
    /// lifecycle paths must use [`KernelAdapter::terminate_if_identity`]. An
    /// implementor must capture an immutable provider object identity before
    /// waiting on local cleanup; where the provider exposes no such identity,
    /// the deployment must guarantee that handles are not reused while one
    /// lifecycle operation is in flight.
    async fn terminate(&self, handle: &RuntimeHandle) -> Result<(), AdapterError>;

    /// Tear down only the runtime carrying this exact session and generation.
    ///
    /// A read followed by [`KernelAdapter::terminate`] is not equivalent: the
    /// provider could reuse the opaque handle between those calls, and runtime
    /// generations are only unique within a session. Implementors expose this
    /// only when their provider can enforce both metadata comparisons in the
    /// same mutation. A legacy identity requires generation metadata to be
    /// absent; it is not a wildcard. The default fails closed, leaving durable
    /// cleanup debt available for a later adapter or operator to retry.
    async fn terminate_if_identity(&self, _identity: &RuntimeIdentity) -> Result<(), AdapterError> {
        Err(AdapterError::Unsupported(
            "this provider cannot conditionally delete an exact runtime identity".into(),
        ))
    }

    /// Find every provider runtime created for one exact durable reservation.
    ///
    /// More than one result is valid: an unknown provider-create result may be
    /// retried and a provider without create idempotency can accept both calls.
    /// Recovery must register at most one and retain exact cleanup authority for
    /// every duplicate. A whole-fleet census is not equivalent because paging
    /// without a stable snapshot can omit the one row recovery is looking for.
    async fn discover_runtime_candidates(
        &self,
        _reservation: &RuntimeReservationIdentity,
    ) -> Result<Vec<RuntimeHandle>, AdapterError> {
        Err(AdapterError::Unsupported(
            "this provider cannot discover an exact runtime reservation".into(),
        ))
    }

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

    /// Quiesce every supported workspace writer, seal one complete checkpoint,
    /// resume the same writers, and return the exact retained artifact.
    ///
    /// Defaults to a stated absence so runtimes predating the versioned Layer 4
    /// capability cannot accidentally enter a Trigger contribution flow.
    async fn checkpoint_workspace(
        &self,
        _handle: &RuntimeHandle,
        _request: &WorkspaceCheckpointRequest,
    ) -> Result<WorkspaceCheckpointPackage, AdapterError> {
        Err(AdapterError::Unsupported(
            "this runtime cannot coherently checkpoint its workspace".to_string(),
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

    /// Watch what this runtime is using, for as long as the caller listens.
    ///
    /// The caller supplies the sink so it can already be draining while the
    /// watch opens and cannot miss the first reading. Dropping the receiving
    /// half ends the watch.
    ///
    /// Defaults to a stated absence. A runtime that cannot report resource use
    /// is still a working runtime, and callers must not retry that as failure.
    async fn watch_resources(
        &self,
        _handle: &RuntimeHandle,
        _readings: mpsc::Sender<ResourceSample>,
    ) -> Result<(), AdapterError> {
        Err(AdapterError::Unsupported(
            "this runtime does not report what it is using".to_string(),
        ))
    }

    /// Move a live session between the terminal and conversation planes
    /// without replacing its sandbox.
    ///
    /// Distinct from a rotation: the program inside the existing pod is
    /// restarted under the other front-end against the same filesystem and the
    /// same conversation, so nothing travels and nothing is lost. Defaults to
    /// unsupported — a kernel that cannot switch in place says so, and the
    /// caller falls back to the rotation that replaces the pod.
    async fn switch_plane(
        &self,
        _handle: &RuntimeHandle,
        _mode: RuntimeMode,
    ) -> Result<(), AdapterError> {
        Err(AdapterError::Unsupported(
            "this kernel cannot switch plane in place".to_string(),
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
        // terminal, holds no conversation, cannot be captured, cannot
        // enumerate what it runs, or cannot report its resources is still a
        // kernel — and a caller reading any of those as a failure would retry
        // forever, or tear down a live session whose only fault is being one.
        let bare = BareKernel;
        let handle = RuntimeHandle("bare-1".into());
        for error in [
            bare.attach_terminal(&handle)
                .await
                .expect_err("no terminal"),
            bare.open_conversation(&handle, &RuntimeOptions::default(), mpsc::channel(1).0)
                .await
                .expect_err("no conversation"),
            bare.snapshot(&handle).await.expect_err("no snapshot"),
            bare.list_runtimes().await.expect_err("cannot enumerate"),
            bare.watch_resources(&handle, mpsc::channel(1).0)
                .await
                .expect_err("no resource readings"),
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
            locator: None,
            decision: PermissionDecision::Abort,
        };
        match input {
            ConversationInput::PermissionAnswer {
                id,
                locator,
                decision,
            } => {
                assert_eq!(id, "req-1");
                assert!(locator.is_none());
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
