//! The durable event vocabulary: what a recorded event can say.
//!
//! Split from `event.rs`, which owns the log's records and the ephemeral lane
//! that rides beside it. The vocabulary is the half that grows — every new
//! thing the service learns to record adds a variant here, a discriminator to
//! [`KNOWN_TAGS`], and an arm to the exhaustive match its tests walk — and it
//! grows without the record types or the live lane having any opinion about it.

use std::num::NonZeroU64;

use serde::{Deserialize, Serialize};

use crate::session::{SessionStatus, StopReason};

/// The semantic event vocabulary. Superset of what any single interface projects.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum EventKind {
    /// The session was created.
    SessionCreated,
    /// A step on the way to a usable session.
    ///
    /// Starting a session is slow enough that "wait" is not an answer on its
    /// own — a person watching a silent screen cannot tell a slow start from a
    /// stuck one. These say which part is taking the time.
    Progress {
        stage: crate::session::ProvisioningStage,
    },
    /// Session status changed.
    StatusChanged {
        /// The new status.
        status: SessionStatus,
        /// Why, when the change was a stop or a pause.
        #[serde(skip_serializing_if = "Option::is_none")]
        stop_reason: Option<StopReason>,
        /// What went wrong, when something did.
        ///
        /// Present only for a failure, and written for the person who has to
        /// act on it. Without this a session that could not start is
        /// indistinguishable from one that ended normally, which is precisely
        /// the moment an explanation is worth most.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        /// The ability this session lacked, when that is what stopped it.
        ///
        /// Named, beside the prose rather than inside it. A refusal is
        /// arrived at typed — the core knows *which* ability was missing —
        /// and flattening it into a sentence throws away the only part a
        /// caller could act on. `API_Overview.md` rule 4 asks errors to name
        /// what is missing, and a sentence containing the word does not
        /// satisfy that: nothing downstream can read a sentence.
        ///
        /// Absent for every failure that was not a refusal, which is most of
        /// them. A provider that was merely unreachable lacks nothing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        capability: Option<String>,
    },
    /// A turn began.
    TurnStarted,
    /// A turn completed.
    TurnCompleted,
    /// The durable initial Trigger prompt changed delivery state.
    InitialPromptDeliveryChanged {
        state: crate::trigger::PromptDeliveryState,
    },
    /// A client took the exclusive terminal attachment.
    TerminalAttached,
    /// The exclusive terminal attachment was released.
    TerminalDetached,
    /// The terminal was resized.
    TerminalResized {
        /// New width in columns.
        columns: u16,
        /// New height in rows.
        rows: u16,
    },
    /// The runtime named the conversation this session is holding.
    ///
    /// Recorded rather than remembered, for the reason §5 gives for recording
    /// anything: a control plane that restarts holds no state that matters, and
    /// what it needs to take a live runtime back over is what that runtime
    /// calls the conversation already running in it. Without this the
    /// replacement opens a second conversation beside the first — the agent
    /// loses its context, and the owner, reading a log that has everything,
    /// cannot tell.
    ConversationOpened {
        /// The runtime's own name for it, carried through opaquely.
        conversation_id: String,
    },
    /// A message the agent finished and committed to.
    ///
    /// Durable because it is what the agent actually said; the token-level
    /// pieces it was assembled from are liveness and travel as ephemeral
    /// frames. Numbering every delta would punch permanent holes in the
    /// sequence a resuming client cursors on, and replaying them would show
    /// somebody their conversation being typed a second time.
    MessageCommitted {
        /// What the agent said, as it committed it.
        content: String,
    },
    /// The owner sent a message to the agent.
    ///
    /// Durable for the same reason the agent's committed message is: the
    /// conversation a client replays must show both sides, and the owner's
    /// turn is the one the surface labels with the time it was sent. Recorded
    /// here rather than reconstructed from the runtime's transcript, so the
    /// word this service keeps for it stays its own and no kernel's vocabulary
    /// has to be understood to know what the owner said.
    OwnerMessage {
        /// What the owner sent, verbatim.
        content: String,
    },
    /// Opaque caller metadata attached to the turn that just began.
    ///
    /// Kept beside the owner's message rather than inside it: conversation
    /// projections can show the message without exposing transport metadata,
    /// while an interface that owns response resources can reconstruct the
    /// exact response after a control-plane restart. Keys and values are
    /// bounded before this event is accepted by [`crate::SessionService`].
    TurnMetadata {
        /// Caller-supplied string pairs, in deterministic key order.
        metadata: std::collections::BTreeMap<String, String>,
    },
    /// The agent began using a tool.
    ToolCallStarted {
        /// Correlates with every later update to the same call.
        call_id: String,
        /// What it is doing, as the runtime would show a person.
        title: String,
        /// The kind of thing it is, in the runtime's own words.
        ///
        /// Opaque here. Interfaces render it and the core never branches on
        /// it: the set differs between runtimes and grows without warning, and
        /// a session must not fail because a kernel invented a tool category.
        kind: String,
        /// What it was asked to do, when this session records tool bodies.
        ///
        /// Absent both when the session was not granted the recording and when
        /// the runtime offered none, and the two are deliberately not told
        /// apart here: a client renders what it is given, never an estimate.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        arguments: Option<ToolBody>,
    },
    /// A tool call moved on.
    ToolCallUpdated {
        /// The call this is about.
        call_id: String,
        /// Where it has got to, in the runtime's own words.
        status: String,
        /// What it produced, when this session records tool bodies.
        ///
        /// Never `ToolCallContent`: a terminal identifier is a terminal fact
        /// and has no place in the conversation core.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<ToolBody>,
    },
    /// What this session has used, as its runtime reports it.
    ///
    /// Totals rather than deltas, because that is what runtimes report and
    /// because a lost delta would silently understate a bill forever, where a
    /// lost total is corrected by the next one.
    UsageReported {
        /// Tokens sent to the model.
        input_tokens: u64,
        /// Tokens the model produced.
        output_tokens: u64,
        /// How much of the model's context the conversation currently fills.
        context_used: u64,
        /// How large the context is that `context_used` is drawn from, where
        /// the runtime states one.
        ///
        /// `None` rather than zero, and the difference is the whole point: a
        /// figure without its ceiling answers nothing, and a ceiling of zero
        /// would draw a full bar on an empty conversation. Absent for every
        /// report that does not state one, including a turn's own accounting.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context_window: Option<u64>,
    },
    /// The conversation got smaller than it was.
    ///
    /// The agent summarised what it was holding to make room. Recorded rather
    /// than left to be noticed, so the owner sees a boundary instead of an
    /// agent that appears to have forgotten something.
    ///
    /// Written only where one runtime gave both readings. The protocol does
    /// not say whether the agent acted on its own or its owner asked, so this
    /// event deliberately claims neither.
    ContextCompacted {
        /// How full the context was at the previous reading, in tokens.
        was: u64,
        /// How full it is now.
        now: u64,
        /// The ceiling in force, where the runtime states one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<u64>,
    },
    /// The session's permission mode changed under its owner.
    ///
    /// Durable rather than only shown live, because a mode is how much the
    /// agent may do unattended, and a session whose mode changed is acting
    /// under a different amount of trust — the record must say so. The name is
    /// the kernel's own and is carried opaquely, exactly like a tool's kind.
    ModeChanged {
        /// The new mode, in the kernel's own word.
        mode: String,
    },
    /// The agent set a plan for the work it is doing.
    ///
    /// A revision replaces the previous one whole — the agent sends the
    /// complete list each time — so a client draws the latest rather than
    /// accumulating. Per-revision volume, not per token, which is why it is
    /// durable where a message delta is not.
    PlanSet {
        /// The steps, in order, each in the kernel's own words.
        entries: Vec<PlanStep>,
    },
    /// The kernel reported the settings a session may change.
    ///
    /// Durable so a client can render a selection rather than a free-text box,
    /// from the kernel's own names and kinds — never a set this service
    /// invented.
    ConfigOptionsSet {
        /// The settable options, each in the kernel's own words.
        options: Vec<ConfigOption>,
    },
    /// The kernel reported the actions the owner may invoke.
    ///
    /// Durable, for the same reason its neighbour above is: the kernel
    /// announces this once, when the conversation opens, and the protocol has
    /// no request to ask again. Live-only, it was sent before any client had
    /// subscribed and was therefore never received by anybody — every session,
    /// every time.
    ///
    /// A replayed list can in principle be stale. It is replaced whole on each
    /// announcement, so it goes stale only if a kernel changes what it offers
    /// mid-session without saying so, and a list one announcement old is worth
    /// more than the empty list that was the alternative.
    AvailableCommandsSet {
        /// The invocable actions, each in the kernel's own words.
        commands: Vec<AvailableCommand>,
    },
    /// Part of the conversation was lost rather than recorded.
    ///
    /// Written where the loss happened, in the same spirit as a rotation's
    /// timestamp: the owner is shown a boundary rather than left to puzzle
    /// over a conversation that skips. It means the runtime held more than it
    /// could while nothing was listening — a long control-plane restart — and
    /// dropped the oldest of it.
    ConversationGap {
        /// How many frames were dropped, as the runtime counted them.
        frames: u64,
    },
    /// The agent is waiting for its owner to sign in to their own account.
    ///
    /// Recorded rather than only shown. A sign-in is the one thing on this
    /// plane that a person must act on somewhere else, and a prompt lost to a
    /// reload leaves a session waiting on a code its owner can no longer see.
    /// Durable, so a reader who comes back finds it where they left it.
    ///
    /// **Carries no credential and cannot.** What it holds is the address of a
    /// page and a short-lived code that is worth nothing without the person's
    /// own account. The credential the sign-in produces is written by the
    /// agent inside its own sandbox and never travels — which is what makes
    /// the promise in `CredentialMode::Own` a fact rather than an intention.
    SignInRequired {
        /// The page the owner opens, on their own device.
        url: String,
        /// The one-time code entered there, where the kernel gave one.
        code: Option<String>,
        /// The kernel's own wording, where it gave any.
        message: Option<String>,
    },
    /// The runtime is blocked awaiting a decision from the client.
    ApprovalRequested {
        /// Correlates with the resolving decision.
        request_id: String,
        /// What the runtime is asking to do, as it would show a person.
        ///
        /// Absent on approvals recorded before this was carried, and on a
        /// runtime that offers no description of what it is asking.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action: Option<String>,
        /// Anything further the runtime offered about the request.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        /// Opaque adapter-owned information for delivering an answer after a
        /// control-plane replacement.
        ///
        /// The core never interprets this value. It is absent on history from
        /// builds predating durable approval routing and for adapters whose
        /// public request id is sufficient.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        locator: Option<layer3_adapter::PermissionLocator>,
    },
    /// The session was moved onto the other plane, because its owner asked.
    ///
    /// Durable because the surface changed under somebody, and the event log
    /// is where they read their session's history. It carries both planes and
    /// the moment the conversation was captured: anything the agent did after
    /// that moment did not travel, and a boundary somebody can see is one they
    /// can recognise.
    PlaneChanged {
        /// The plane the session was on.
        from: crate::options::SessionMode,
        /// The plane it is on now.
        to: crate::options::SessionMode,
        /// When its conversation was taken, RFC 3339.
        snapshot_at: String,
    },
    /// A pending approval was resolved.
    ApprovalResolved {
        /// The request that was resolved.
        request_id: String,
        /// Whether the action was permitted.
        allowed: bool,
    },
    /// A session's processor use has held near its reported limit long enough to say so.
    ///
    /// Durable because it happened once and the readings it was folded from
    /// are never stored. A missing rate or limit produces no episode.
    ///
    /// An open episode ends at the first status change that leaves
    /// `is_interactive()`, whether or not a close was written. A second open
    /// with no close between replaces the first; this is state, not a stack.
    CpuPressureOpened {
        /// Processor use over the reading interval, in thousandths of one core.
        used_millis: u32,
        /// The processor limit it was measured against, in the same unit.
        limit_millis: u32,
        /// The first as a fraction of the second, in permille.
        occupancy_permille: u16,
    },
    /// A session's processor use came away from its limit and stayed away.
    CpuPressureClosed {
        /// The highest processor use observed during the episode.
        peak_millis: u32,
        /// The processor limit those readings were measured against.
        limit_millis: u32,
        /// The peak as a fraction of the limit, in permille.
        peak_permille: u16,
        /// How long the episode ran, from a monotonic clock.
        duration_seconds: u32,
    },
    /// A session's memory has been near its limit long enough to say so.
    ///
    /// Durable because it happened once and the readings it was folded from
    /// are never stored. An episode opens only after occupancy has held at or
    /// above its entry threshold for several consecutive readings.
    ///
    /// An open episode ends at the first status change that leaves
    /// `is_interactive()`, whether or not a close was written. A second open
    /// with no close between replaces the first; this is state, not a stack.
    MemoryPressureOpened {
        /// Bytes in use at the reading that opened it.
        used_bytes: u64,
        /// The limit it was measured against.
        limit_bytes: u64,
        /// The first as a fraction of the second, in permille.
        occupancy_permille: u16,
    },
    /// A session's memory came away from its limit and stayed away.
    MemoryPressureClosed {
        /// The most bytes held at any point in the episode.
        peak_bytes: u64,
        /// The limit those bytes were measured against.
        limit_bytes: u64,
        /// The peak as a fraction of the limit, in permille.
        peak_permille: u16,
        /// How long the episode ran, from a monotonic clock.
        ///
        /// A duration rather than wall-clock subtraction: event timestamps are
        /// display moments and do not provide a monotonic interval.
        duration_seconds: u32,
    },
    /// A runtime handover began without changing the active incarnation.
    ///
    /// Written immediately before the paired `Rotating` status in the same
    /// durable event unit. The outgoing runtime's cumulative counter remains
    /// current until either a coordinated [`Self::RuntimeStarted`] switch or a
    /// compatibility [`Self::RuntimeIncarnationStarted`] publication. A failed
    /// attempt can return to the same runtime and therefore has no matching
    /// incarnation-start marker.
    RuntimeHandoverStarted,
    /// A compatibility store durably named a newly provisioned runtime.
    ///
    /// Written in the same atomic record/event commit that installs the
    /// runtime handle. Compatibility stores have no durable fencing generation,
    /// so every marker starts another incarnation. Failed provider attempts
    /// that never become durable write none; an idempotently retried commit
    /// writes the unit once.
    RuntimeIncarnationStarted,
    /// A runtime became the durable authority for this session.
    ///
    /// Written in the same atomic unit as the authority switch. A candidate
    /// that never takes authority therefore writes no marker, and retrying an
    /// already committed switch cannot write a second one. The generation is
    /// the durable fencing value, so readers can ignore a duplicate or older
    /// marker without treating it as another runtime incarnation.
    RuntimeStarted {
        /// Strictly positive durable runtime generation.
        generation: NonZeroU64,
    },
    /// The session's runtime is gone, and the service has established it.
    ///
    /// Written where reconciliation finds out, not where a replacement is
    /// started, and not only where this process happened to be serving the
    /// session. A control plane that has just restarted holds no memory of any
    /// session, which is precisely the case a lost runtime is most likely to
    /// be found in.
    ///
    /// It records that the runtime died. It does not record that anything was
    /// carried across: a conversation is captured by asking the runtime for
    /// it, and this one cannot be asked.
    RuntimeLost {
        /// The runtime that is gone, as the provider named it. Opaque here.
        runtime_id: String,
        /// Why, where that could be established at all.
        ///
        /// Absent when nothing said. A sandbox that vanished from the
        /// provider's list entirely leaves no verdict behind, and an invented
        /// one would be worse than the silence.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cause: Option<crate::session::RuntimeLossCause>,
    },
}

/// What a tool call was asked to do, or what it produced.
///
/// A bounded, redacted copy of a runtime's own words. Bounded because a tool's
/// arguments or output can be enormous and the durable log is not a file store;
/// redacted because a tool's output can contain a value the session was handed,
/// and the core knows which values those are and replaces them before they are
/// written. The bound is stated rather than silently applied — §18.2 forbids
/// dropping the tail of a thing without saying so.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolBody {
    /// The body, as text.
    pub text: String,
    /// Whether the body was cut to fit the cap.
    #[serde(default)]
    pub truncated: bool,
}

/// One step of an agent's plan, in the kernel's own words.
///
/// Nothing is mapped onto a vocabulary this service invented: the kernel names
/// the step and its state, and a client renders both verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    /// What the step is, as the agent described it.
    pub title: String,
    /// Where it has got to, in the kernel's own word.
    pub status: String,
}

/// One setting the kernel says is settable, in its own words.
///
/// The kernel names the setting and the kind of value it takes; a client
/// renders a selection from these rather than a free-text box, exactly as it
/// does for the answers an approval offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigOption {
    /// The kernel's identifier for the setting.
    pub id: String,
    /// What it is called, as the kernel named it.
    pub name: String,
    /// What kind of value it takes, in the kernel's own word.
    pub kind: String,
}

/// One action the owner may invoke, as the kernel offers it.
///
/// Carried opaquely: the kernel names the action and what it does, and a
/// client renders both verbatim, never mapped onto a set this service invented.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvailableCommand {
    /// What the command is, as the kernel named it.
    pub name: String,
    /// What it does, as the kernel described it.
    pub description: String,
}

/// Every discriminator this build owns, exactly as serde spells it.
///
/// Must name every variant of [`EventKind`]. A variant missing from here would
/// be read back as somebody else's vocabulary and skipped rather than
/// projected, which is why the test module walks the enum against this list.
const KNOWN_TAGS: &[&str] = &[
    "session_created",
    "progress",
    "status_changed",
    "turn_started",
    "turn_completed",
    "initial_prompt_delivery_changed",
    "conversation_opened",
    "message_committed",
    "owner_message",
    "turn_metadata",
    "tool_call_started",
    "tool_call_updated",
    "usage_reported",
    "context_compacted",
    "mode_changed",
    "plan_set",
    "config_options_set",
    "available_commands_set",
    "conversation_gap",
    "terminal_attached",
    "terminal_detached",
    "terminal_resized",
    "approval_requested",
    "approval_resolved",
    "cpu_pressure_opened",
    "cpu_pressure_closed",
    "memory_pressure_opened",
    "memory_pressure_closed",
    "runtime_handover_started",
    "runtime_incarnation_started",
    "runtime_started",
    "runtime_lost",
    "plane_changed",
    "sign_in_required",
];

impl EventKind {
    /// Whether `tag` names a variant this build must validate strictly.
    ///
    /// Storage asks this before deserializing, so it can tell a discriminator
    /// from a newer writer apart from a payload that is broken under a
    /// discriminator we own. The first is history this build cannot read; the
    /// second is corruption, and reporting them the same way would either
    /// hide real damage or make a rolling deployment look like one.
    pub fn recognizes_tag(tag: &str) -> bool {
        KNOWN_TAGS.contains(&tag)
    }

    /// The discriminator this variant serializes under.
    ///
    /// Derived from the serialization rather than from a second match, so it
    /// cannot drift from what the store actually writes. For diagnostics: a
    /// failure to record says which event was lost, and a message that names
    /// the wrong one is worse than a message that names none.
    pub fn tag(&self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|value| {
                value
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "unrecognised".to_string())
    }
}

#[cfg(test)]
#[path = "event_kind_tests.rs"]
mod event_kind_tests;
