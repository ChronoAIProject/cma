//! What a session's agent is allowed and asked to do.
//!
//! Chosen when a session is created and fixed for its life, because these
//! decide how the agent behaves from its first turn: which model answers, how
//! hard it thinks, what it may do without asking, and whose credential it
//! spends.
//!
//! Deliberately generic. Every runtime expresses these differently — one calls
//! a permission level by one name, another by a second, a third has no notion
//! of it at all — so the core states the intent and each adapter renders it in
//! its own vocabulary. Naming a specific runtime's settings here would put that
//! runtime's vocabulary in the one place that is supposed to outlive it.

use serde::{Deserialize, Serialize};

/// How much deliberation to ask of the model.
///
/// A spectrum rather than a boolean because the trade is real: more thinking
/// costs time and money on work that did not need it, and less produces
/// confident answers to hard questions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    /// Barely any. For mechanical work where latency dominates.
    Minimal,
    Low,
    Medium,
    /// The default, and what an unconfigured session gets.
    ///
    /// Deliberately not the middle setting. This runs coding work, where a
    /// wrong answer costs far more than the extra seconds spent avoiding it.
    #[default]
    High,
    /// As much as the model offers.
    XHigh,
}

impl ReasoningEffort {
    /// The wire name, which is also what runtimes tend to expect.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
        }
    }
}

/// What the agent may do without stopping to ask.
///
/// The names describe the amount of trust extended, not the mechanism, because
/// the mechanism differs per runtime: one asks for approval per command, one
/// confines writes to a workspace, one does both. What a person is choosing is
/// how much rope to give it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    /// Ask before anything that touches the world outside the workspace.
    ///
    /// The default, and deliberately so: a session that can act freely should
    /// be something a person chose, never something they got by not choosing.
    #[default]
    Guarded,
    /// Act within the workspace, asking only when something fails or when the
    /// action reaches beyond it.
    Autonomous,
    /// Ask for nothing, and confine nothing.
    ///
    /// The agent can run any command and reach anything the sandbox can. That
    /// is survivable only because the sandbox is the boundary — this removes
    /// the boundaries *inside* it, not the sandbox itself.
    Unrestricted,
}

impl PermissionMode {
    /// Whether this mode removes the guard rails entirely.
    ///
    /// Exposed so an interface can mark it as the exceptional choice it is,
    /// rather than each surface deciding for itself which modes are alarming.
    pub fn is_unrestricted(self) -> bool {
        matches!(self, Self::Unrestricted)
    }
}

/// Whose credential the agent spends on model calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CredentialMode {
    /// The platform's, on the owner's behalf.
    ///
    /// The default. The owner's own identity authorises the call, and their
    /// entitlements apply to it, without a second credential existing anywhere.
    #[default]
    Delegated,
    /// The owner's own account with the model vendor.
    ///
    /// For someone who would rather spend their own subscription. The agent
    /// authenticates itself inside the sandbox, so no credential of theirs
    /// reaches this service.
    Own,
}

/// Which plane a session is driven on.
///
/// Not a preference: it decides what the session *is*. A terminal session is a
/// screen someone types at, and a conversation session is a structured
/// exchange of messages, tool calls and approvals. The two need different
/// abilities from the runtime behind them, and a session that asked for one
/// and got the other would be broken in a way no error message could rescue,
/// so this is fixed at creation and checked against the runtime before a
/// session exists at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    /// A human-facing terminal, driven by keystrokes and read as a screen.
    ///
    /// The default, and deliberately so: it is what every session created
    /// before this existed was, and reading those records back must not
    /// silently turn them into something else.
    #[default]
    Terminal,
    /// A structured conversation: prompts in, messages, tool calls and
    /// approvals out.
    Conversation,
}

/// The full set of choices a session is created with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AgentOptions {
    /// Which plane the session is driven on.
    #[serde(default)]
    pub mode: SessionMode,
    /// Model to select, or `None` to accept the deployment's default.
    ///
    /// Opaque here. The core neither validates nor interprets it: the set of
    /// usable models belongs to whatever serves them, and a list baked in here
    /// would be wrong the first time that set changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default)]
    pub reasoning_effort: ReasoningEffort,
    #[serde(default)]
    pub permission: PermissionMode,
    #[serde(default)]
    pub credential: CredentialMode,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_asks_before_acting() {
        // The property worth protecting: a session that can act without asking
        // must be something someone chose. Nobody gets it by omission.
        let o = AgentOptions::default();
        assert_eq!(o.permission, PermissionMode::Guarded);
        assert!(!o.permission.is_unrestricted());
    }

    #[test]
    fn the_default_spends_the_platform_credential() {
        assert_eq!(
            AgentOptions::default().credential,
            CredentialMode::Delegated
        );
    }

    #[test]
    fn the_default_model_is_the_deployments_own() {
        // None rather than a name: a model list baked into the core would be
        // wrong the first time the upstream changed its offering.
        assert!(AgentOptions::default().model.is_none());
    }

    #[test]
    fn only_the_unrestricted_mode_reports_itself_as_such() {
        assert!(PermissionMode::Unrestricted.is_unrestricted());
        assert!(!PermissionMode::Guarded.is_unrestricted());
        assert!(!PermissionMode::Autonomous.is_unrestricted());
    }

    #[test]
    fn effort_levels_carry_stable_wire_names() {
        // Stored and sent, so a rename would silently change behaviour for
        // sessions already recorded.
        assert_eq!(ReasoningEffort::Minimal.as_str(), "minimal");
        assert_eq!(ReasoningEffort::Medium.as_str(), "medium");
        assert_eq!(ReasoningEffort::XHigh.as_str(), "xhigh");
        // The default is stated rather than assumed, since it is what most
        // sessions will actually run with.
        assert_eq!(ReasoningEffort::default(), ReasoningEffort::High);
    }

    #[test]
    fn options_round_trip_through_their_wire_form() {
        let o = AgentOptions {
            mode: SessionMode::Conversation,
            model: Some("some-model".into()),
            reasoning_effort: ReasoningEffort::High,
            permission: PermissionMode::Unrestricted,
            credential: CredentialMode::Own,
        };
        let json = serde_json::to_string(&o).expect("serialises");
        let back: AgentOptions = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(back, o);
    }

    #[test]
    fn an_absent_choice_deserialises_to_the_safe_default() {
        // Sessions recorded before these existed must not come back as
        // unrestricted.
        let o: AgentOptions = serde_json::from_str("{}").expect("deserialises");
        assert_eq!(o, AgentOptions::default());
        assert_eq!(o.permission, PermissionMode::Guarded);
    }
}
