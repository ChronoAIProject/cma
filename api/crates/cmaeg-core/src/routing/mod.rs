//! Which consumers an admitted event reaches.
//!
//! Routing is decided once per event, at ingress, and bound into
//! `Activity.routing`. Everything here is pure: no I/O, clock or logging,
//! and every loop is bounded by the stored policy limits (32 rules of six
//! lists of 64 entries, 8 targets, 16 trace steps), so the same input always
//! yields the same decision.
//!
//! Routing never widens admission. The caller runs it only after the
//! source's admission policy has accepted the event; addressing, rules and
//! defaults choose among the channel's consumers, never whether a sender or
//! chat may reach the channel at all.
use std::collections::BTreeSet;

use crate::{
    activity::{Activity, ActivityRouting, Causation, RouteResolution},
    channel::{ChannelDefinition, Source},
    ids::ConsumerId,
};

pub mod addressing;
pub mod event_field;
mod precedence;
mod roster;
pub mod rules;
mod trace;

use roster::Roster;

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
    /// The user's token, bounded as for an unknown target, never a hidden name.
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

/// Route an admitted event on `def`, in this fixed order:
///
/// | Order | Trigger | Result |
/// |---|---|---|
/// | 0a | `def.routing` absent | `Legacy`; `Activity.routing` stays absent |
/// | 0b | `enabled == false` | the kill switch's per-event shadow decision |
/// | 1 | `card_consumer` is `Some(c)` | `interaction`: `[c or default]` |
/// | 2 | `published` is `Some` | `agent_event`: the publisher's targets |
/// | 3 | chat addressing or `cmaegtargets` | `mention`, `command` or `event_field` |
/// | 4 | the first matching active rule | `rule`: its targets |
/// | 5 | otherwise | `default`: `default_targets` |
///
/// Steps 4 and 5 skip pending consumers and, for a non-human actor,
/// consumers that do not answer one; explicitly named targets are refused
/// instead. Targets keep their order without duplicates, and `targets[0]`
/// is the primary consumer.
pub fn resolve(
    def: &ChannelDefinition,
    input: &RouteInput<'_>,
    enabled: bool,
) -> Result<Resolution, RouteError> {
    let mut trace = Trace::default();
    let routing = decide(def, input, enabled, &mut trace)?;
    Ok(match routing {
        Some(routing) => Resolution::Routed(Box::new(RouteDecision { routing, trace })),
        None => Resolution::Legacy,
    })
}

/// The trace `resolve` records for the same input, whatever the outcome.
/// A routed decision carries its trace; a refusal or a kill-switch legacy
/// outcome does not, so a caller that reports one asks for it here. A
/// channel without routing records nothing.
pub fn explain(def: &ChannelDefinition, input: &RouteInput<'_>, enabled: bool) -> Trace {
    let mut trace = Trace::default();
    // The outcome is resolve's to report; only the steps are wanted here.
    let _outcome = decide(def, input, enabled, &mut trace);
    trace
}

/// The routing to bind, or `None` for the legacy path.
fn decide(
    def: &ChannelDefinition,
    input: &RouteInput<'_>,
    enabled: bool,
    trace: &mut Trace,
) -> Result<Option<ActivityRouting>, RouteError> {
    let Some(roster) = Roster::new(def) else {
        return Ok(None);
    };
    if !enabled {
        return kill_switch(def, &roster, input, trace);
    }
    precedence::route(def, &roster, input, trace).map(Some)
}

/// Step 0b: multi-consumer routing is switched off, but this channel stores
/// a routing policy. An event may take today's single-consumer path only
/// when its policy would have given it to `default`; everything else is
/// refused. Refusing an event is recoverable, and leaking it to another
/// agent is not, so the switch fails closed per event (I13).
///
/// - A click on `default`'s card is legacy; any other card is denied.
/// - A hand-off cannot occur, since publishing is off with routing; it is
///   refused all the same.
/// - Any other event computes the shadow decision, steps 3 to 5 with every
///   constraint. A refusal closes. Explicit addressing (`mention`, `command`,
///   `event_field`) is legacy only when it names exactly `default`. A `rule`
///   or `default` resolution is legacy when `default` is among its targets;
///   the others are not reached, which narrows who sees the event and never
///   widens it.
///
/// A per-channel predicate such as `default_receives_all` is not used. It
/// cannot see explicit addressing, and extended to cover it, it would refuse
/// nearly every multi-consumer channel, because `mentionable` defaults to
/// true; deciding per event refuses only the events that would leak.
fn kill_switch(
    def: &ChannelDefinition,
    roster: &Roster<'_>,
    input: &RouteInput<'_>,
    trace: &mut Trace,
) -> Result<Option<ActivityRouting>, RouteError> {
    trace.push(TraceStep::Precedence("kill_switch"));
    if let Some(card) = input.card_consumer {
        let legacy = card.is_none_or(ConsumerId::is_default);
        record_shadow(trace, RouteResolution::Interaction, legacy);
        return if legacy {
            Ok(None)
        } else {
            Err(RouteError::InteractionDenied)
        };
    }
    if input.published.is_some() {
        record_shadow(trace, RouteResolution::AgentEvent, false);
        return Err(RouteError::KillSwitchClosed);
    }
    let (resolution, legacy) = match precedence::select(def, roster, input, trace) {
        Err(refusal) => (refusal.resolution, false),
        Ok(selection) => {
            let targets = selection.targets.as_slice();
            let legacy = match selection.resolution {
                RouteResolution::Rule | RouteResolution::Default => {
                    targets.iter().any(|target| target.id.is_default())
                }
                _ => matches!(targets, [only] if only.id.is_default()),
            };
            (selection.resolution, legacy)
        }
    };
    record_shadow(trace, resolution, legacy);
    if legacy {
        Ok(None)
    } else {
        Err(RouteError::KillSwitchClosed)
    }
}

fn record_shadow(trace: &mut Trace, resolution: RouteResolution, legacy: bool) {
    let outcome = if legacy { "legacy" } else { "closed" };
    trace.push(TraceStep::Shadow {
        resolution,
        outcome,
    });
}
