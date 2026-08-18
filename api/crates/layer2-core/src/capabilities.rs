//! What a session was granted, and what it was refused.
//!
//! Two facts have to be told apart everywhere in this service: "this session
//! can never do that" and "that just failed". Only the second is worth
//! retrying, only the first is worth telling a person at creation, and a
//! caller who cannot distinguish them ends up doing neither well.
//!
//! So a session carries a granted set, decided when it is created. It is the
//! intersection of what its runtime can do with what its mode asks for, which
//! means a request outside it is refused by name before anything is
//! provisioned — rather than accepted, provisioned, and found wanting by
//! whoever tries to use it.
//!
//! Recorded rather than recomputed. A runtime's abilities can change when its
//! implementation does, and a live session finding itself with a different set
//! than it started with is the kind of surprise this whole design exists to
//! remove.
//!
//! The invariant is therefore **"this changes only by an explicit, recorded
//! act"**, not "this never changes". One act qualifies: an owner moving their
//! session between planes, which is a rotation — the set is re-derived from
//! the same rule, against the same runtime, and the change is written to the
//! session's log where its owner reads it. That is the opposite of a quiet
//! recomputation, and it is what the sentence above is protecting against.

use layer3_adapter::KernelCapabilities;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::options::SessionMode;

/// The abilities a session holds, fixed at creation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SessionCapabilities {
    /// Structured conversation: prompts, messages, tool calls, approvals.
    #[serde(default)]
    pub conversation: bool,
    /// A human-facing terminal, its screen and its geometry.
    #[serde(default)]
    pub terminal: bool,
    /// Can be carried onto a replacement runtime with its history.
    #[serde(default)]
    pub snapshot: bool,
    /// Accepts input while a turn is in flight.
    #[serde(default)]
    pub steer: bool,
    /// A turn in flight can be abandoned without ending the session.
    #[serde(default)]
    pub interrupt: bool,
    /// Reports token usage as it accrues.
    #[serde(default)]
    pub usage: bool,
}

impl SessionCapabilities {
    /// Decide what a session gets, or name the first thing it cannot have.
    ///
    /// The mode states a requirement — a terminal session is nothing without a
    /// terminal, a conversation session nothing without a conversation — and
    /// everything else is granted only where the runtime actually offers it.
    ///
    /// The two planes are exclusive by construction. A conversation session on
    /// a runtime that also renders a terminal does not get both: every
    /// interface dispatches on this set to decide what an operation means, and
    /// a session claiming both would make `send_input` ambiguous — keystrokes
    /// or a prompt, with no way to tell which the caller meant.
    pub fn granted(mode: SessionMode, kernel: KernelCapabilities) -> Result<Self, CoreError> {
        match mode {
            SessionMode::Terminal => {
                if !kernel.terminal {
                    return Err(CoreError::CapabilityUnsupported("terminal".into()));
                }
                Ok(Self {
                    conversation: false,
                    terminal: true,
                    snapshot: kernel.snapshot,
                    // Steering, interrupting and usage belong to the
                    // conversation plane: they are expressed in turns and
                    // tokens, and a screen has neither. Granting them here
                    // would advertise operations with nothing behind them —
                    // a terminal stops what it is doing with a control
                    // character, which is input rather than an operation.
                    steer: false,
                    interrupt: false,
                    usage: false,
                })
            }
            SessionMode::Conversation => {
                if !kernel.conversation {
                    return Err(CoreError::CapabilityUnsupported("conversation".into()));
                }
                Ok(Self {
                    conversation: true,
                    terminal: false,
                    snapshot: kernel.snapshot,
                    steer: kernel.steer,
                    interrupt: kernel.interrupt,
                    usage: kernel.usage,
                })
            }
        }
    }

    /// Whether this set was never written down.
    ///
    /// `granted` never produces an empty set: it always grants the plane it
    /// was asked for, or it fails. So a session holding nothing at all is not
    /// a session that was granted nothing — it is a record written before this
    /// existed, and the difference decides whether it is settled or believed.
    ///
    /// The same distinction `granted_services` draws with `Option`, drawn here
    /// without one because the type already carries an impossible value.
    pub fn unrecorded(&self) -> bool {
        *self == Self::default()
    }

    /// Refuse unless this session holds `capability`, naming it if it does not.
    ///
    /// The single place a missing ability turns into an error, so every
    /// interface reports the same thing in the same words and none of them
    /// invents a 404 for it.
    pub fn require(&self, capability: Capability) -> Result<(), CoreError> {
        if self.holds(capability) {
            return Ok(());
        }
        Err(CoreError::CapabilityUnsupported(capability.name().into()))
    }

    /// Whether this session holds a named ability.
    pub fn holds(&self, capability: Capability) -> bool {
        match capability {
            Capability::Conversation => self.conversation,
            Capability::Terminal => self.terminal,
            Capability::Snapshot => self.snapshot,
            Capability::Steer => self.steer,
            Capability::Interrupt => self.interrupt,
            Capability::Usage => self.usage,
        }
    }
}

/// Which planes a runtime with these abilities can be asked to serve.
///
/// Derived by asking `granted` rather than by restating the rule, so what a
/// deployment advertises and what it will accept cannot drift apart. A client
/// offering a plane the core would refuse has moved the refusal from before
/// the create button to after it, which is the whole thing this exists to
/// prevent.
///
/// Ordered as the modes are declared, so the answer is stable between calls
/// and a client can render it without sorting.
pub fn servable_modes(kernel: KernelCapabilities) -> Vec<SessionMode> {
    [SessionMode::Terminal, SessionMode::Conversation]
        .into_iter()
        .filter(|mode| SessionCapabilities::granted(*mode, kernel).is_ok())
        .collect()
}

/// One ability, named, so a refusal can say which one is missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    /// Structured conversation.
    Conversation,
    /// A human-facing terminal.
    Terminal,
    /// Carrying a session onto a replacement runtime.
    Snapshot,
    /// Input while a turn is in flight.
    Steer,
    /// Abandoning a turn in flight.
    Interrupt,
    /// Token usage as it accrues.
    Usage,
}

impl Capability {
    /// The wire name, which is what a refusal quotes.
    ///
    /// Part of the contract: clients branch on these, so a rename changes
    /// behaviour for everything already written against them.
    pub fn name(self) -> &'static str {
        match self {
            Self::Conversation => "conversation",
            Self::Terminal => "terminal",
            Self::Snapshot => "snapshot",
            Self::Steer => "steer",
            Self::Interrupt => "interrupt",
            Self::Usage => "usage",
        }
    }
}

#[cfg(test)]
#[path = "capabilities_tests.rs"]
mod capabilities_tests;
