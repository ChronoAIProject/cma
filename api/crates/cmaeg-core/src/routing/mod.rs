//! Which consumers an admitted event reaches.
//!
//! Routing is decided once per event, at ingress, and bound into
//! `Activity.routing`. Everything here is pure: no I/O, clock, randomness or
//! logging, and every loop is bounded by the stored policy limits (32 rules
//! of six lists of 64 entries, 8 targets, 16 trace steps), so the same input
//! always yields the same decision.
use std::collections::BTreeSet;

use crate::{
    activity::{Activity, ActivityRouting, Causation},
    channel::Source,
    ids::ConsumerId,
};

pub mod addressing;
pub mod event_field;
mod roster;
pub mod rules;
mod trace;

pub use trace::{Trace, TraceStep, MAX_TRACE_STEPS};

/// Everything the routing decision reads beyond the channel definition.
#[derive(Debug, Clone)]
pub struct RouteInput<'a> {
    pub activity: &'a Activity,
    /// def.source_of(&activity.source): the source the activity arrived on.
    pub chat_source: Option<&'a Source>,
    /// Some(..) only for interaction clicks: the clicked card's consumer (None = default).
    pub card_consumer: Option<Option<&'a ConsumerId>>,
    /// Hand-offs only: the publisher's target tokens and the causation to record.
    pub published: Option<(&'a [String], Causation)>,
    /// CMAEG_NON_HUMAN_PROVIDERS, read by the caller.
    pub non_human_providers: &'a BTreeSet<String>,
    /// False for a binary-mode event on an HMAC source: its ce-* attributes are unsigned.
    pub attributes_signed: bool,
}

/// Why an event cannot be routed. It carries no secret: its strings are user
/// tokens or consumer names, which the caller bounds and escapes before any
/// notice. `Display` gives only the stable reason, never the token.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RouteError {
    /// The offending token, truncated to 64 bytes on a char boundary.
    #[error("target_unknown")]
    TargetUnknown(String),
    /// The consumer's name.
    #[error("target_not_mentionable")]
    TargetNotMentionable(String),
    #[error("too_many_targets")]
    TooManyTargets,
    #[error("targets_not_permitted")]
    TargetsNotPermitted,
    #[error("invalid_targets")]
    InvalidTargets,
    #[error("targets_unsigned")]
    TargetsUnsigned,
    /// The consumer's name.
    #[error("target_ignores_actor_kind")]
    TargetIgnoresActorKind(String),
    #[error("no_target_accepts_actor_kind")]
    NoTargetAcceptsActorKind,
    #[error("consumer_unavailable")]
    ConsumerUnavailable,
    #[error("kill_switch_closed")]
    KillSwitchClosed,
    #[error("interaction_denied")]
    InteractionDenied,
}

/// The outcome of routing an admitted event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Today's single-consumer path: no `Activity.routing`, delivered to
    /// `default` exactly as on a channel without routing.
    Legacy,
    /// Boxed: a decision is several hundred bytes and a legacy outcome none.
    Routed(Box<RouteDecision>),
}

/// A routed event's decision and how it was reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteDecision {
    pub routing: ActivityRouting,
    pub trace: Trace,
}
