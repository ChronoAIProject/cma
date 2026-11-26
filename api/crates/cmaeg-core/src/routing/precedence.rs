//! Steps 1 to 5 of the fixed precedence, and the constraints every selection
//! meets before it is bound into an activity.
use super::{
    addressing, event_field,
    roster::{Roster, Targets},
    rules, RouteError, RouteInput, Trace, TraceStep,
};
use crate::{
    activity::{
        ActivityKind, ActivityRouting, ActorKind, Causation, ConsumerTarget, RouteResolution,
    },
    channel::{ChannelDefinition, SourceId},
    ids::{ConsumerId, RuleId},
};

/// The targets a step selects, with every constraint applied.
pub(super) struct Selection {
    pub(super) resolution: RouteResolution,
    pub(super) rule_id: Option<RuleId>,
    /// 1..=max_targets; `targets[0]` is the primary consumer.
    pub(super) targets: Vec<ConsumerTarget>,
    pub(super) addressing_bytes: u16,
}

impl Selection {
    fn of(resolution: RouteResolution, targets: Vec<ConsumerTarget>) -> Self {
        Self {
            resolution,
            rule_id: None,
            targets,
            addressing_bytes: 0,
        }
    }

    fn bind(self, source_id: SourceId, causation: Option<Causation>) -> ActivityRouting {
        ActivityRouting {
            source_id,
            resolution: self.resolution,
            rule_id: self.rule_id,
            targets: self.targets,
            addressing_bytes: self.addressing_bytes,
            causation,
        }
    }
}

/// The full precedence on a routed channel with routing enabled.
pub(super) fn route(
    def: &ChannelDefinition,
    roster: &Roster<'_>,
    input: &RouteInput<'_>,
    trace: &mut Trace,
) -> Result<ActivityRouting, RouteError> {
    let source_id = input.activity.source.source_id();
    // 1. A click reaches only its card's consumer; nothing below may pre-empt it.
    if let Some(card) = input.card_consumer {
        trace.push(TraceStep::Precedence("interaction"));
        let target = interaction(roster, card)?;
        let selection = Selection::of(RouteResolution::Interaction, vec![target]);
        return Ok(selection.bind(source_id, None));
    }
    // 2. A hand-off reaches the consumers its publisher named.
    if let Some((tokens, causation)) = &input.published {
        trace.push(TraceStep::Precedence("agent_event"));
        let ids = event_field::published(roster, tokens)?;
        let targets = explicit(roster, input, &ids)?;
        let selection = Selection::of(RouteResolution::AgentEvent, targets);
        return Ok(selection.bind(SourceId::agent(), Some(causation.clone())));
    }
    let selection = select(def, roster, input, trace).map_err(|refusal| refusal.error)?;
    Ok(selection.bind(source_id, None))
}

/// The card's consumer, `default` for `None`. A consumer removed or set
/// pending since the card was sent can no longer answer it.
pub(super) fn interaction(
    roster: &Roster<'_>,
    card: Option<&ConsumerId>,
) -> Result<ConsumerTarget, RouteError> {
    roster
        .get(card)
        .filter(|member| !member.view.pending)
        .map(|member| member.target())
        .ok_or(RouteError::InteractionDenied)
}

/// A refusal and the resolution being attempted when it was raised, which
/// the kill switch records in its shadow step.
pub(super) struct Refusal {
    pub(super) resolution: RouteResolution,
    pub(super) error: RouteError,
}

/// Steps 3 to 5: explicit addressing, else the first matching active rule,
/// else the default targets. The kill switch shadows exactly this.
pub(super) fn select(
    def: &ChannelDefinition,
    roster: &Roster<'_>,
    input: &RouteInput<'_>,
    trace: &mut Trace,
) -> Result<Selection, Refusal> {
    // Clicks are resolved only from their card, which the caller supplies.
    if matches!(input.activity.kind, ActivityKind::Interaction(_)) {
        return Err(at(RouteResolution::Interaction)(
            RouteError::InteractionDenied,
        ));
    }
    if let Some(form) = addressing::recognize(roster, input.activity, input.chat_source) {
        let refused = at(form.resolution());
        if let Some(addressed) = form.resolve(roster).map_err(&refused)? {
            let resolution = addressed.resolution;
            let targets = addressed_targets(roster, input, &addressed.targets, resolution, trace)
                .map_err(&refused)?;
            return Ok(Selection {
                resolution,
                rule_id: None,
                targets,
                addressing_bytes: addressed.addressing_bytes,
            });
        }
    }
    let resolution = RouteResolution::EventField;
    if let Some(ids) = event_field::targets(roster, input).map_err(at(resolution))? {
        let targets =
            addressed_targets(roster, input, &ids, resolution, trace).map_err(at(resolution))?;
        return Ok(Selection::of(resolution, targets));
    }
    if let Some(rule) = rules::first_match(def, input.activity, trace) {
        trace.push(TraceStep::Precedence("rules"));
        let resolution = RouteResolution::Rule;
        let targets = implicit(roster, input, &rule.targets, trace).map_err(at(resolution))?;
        return Ok(Selection {
            rule_id: Some(rule.id.clone()),
            ..Selection::of(resolution, targets)
        });
    }
    trace.push(TraceStep::Precedence("default"));
    let resolution = RouteResolution::Default;
    let targets =
        implicit(roster, input, &roster.policy.default_targets, trace).map_err(at(resolution))?;
    Ok(Selection::of(resolution, targets))
}

fn at(resolution: RouteResolution) -> impl Fn(RouteError) -> Refusal {
    move |error| Refusal { resolution, error }
}

/// Record explicit addressing in the trace and check its targets.
fn addressed_targets(
    roster: &Roster<'_>,
    input: &RouteInput<'_>,
    ids: &[ConsumerId],
    resolution: RouteResolution,
    trace: &mut Trace,
) -> Result<Vec<ConsumerTarget>, RouteError> {
    trace.push(TraceStep::Precedence("addressing"));
    let form = match resolution {
        RouteResolution::Mention => "mention",
        RouteResolution::Command => "command",
        _ => "event_field",
    };
    trace.push(TraceStep::Addressing {
        form,
        tokens: u8::try_from(ids.len()).unwrap_or(u8::MAX),
    });
    explicit(roster, input, ids)
}

/// Targets someone named. A non-human actor's event is refused when it
/// names a consumer that does not answer non-human actors: the producer
/// asked for that consumer, so dropping it silently would mislead them.
fn explicit(
    roster: &Roster<'_>,
    input: &RouteInput<'_>,
    ids: &[ConsumerId],
) -> Result<Vec<ConsumerTarget>, RouteError> {
    if ids.len() > roster.max_targets() {
        return Err(RouteError::TooManyTargets);
    }
    let human = input.activity.actor.kind == ActorKind::Human;
    ids.iter()
        .map(|id| {
            // The parsers return only routable consumers.
            let member = roster
                .get(Some(id))
                .filter(|member| !member.view.pending)
                .ok_or(RouteError::ConsumerUnavailable)?;
            if !human && !member.view.non_human_effective(input.non_human_providers) {
                return Err(RouteError::TargetIgnoresActorKind(member.name.to_string()));
            }
            Ok(member.target())
        })
        .collect()
}

/// A rule's or the default's stored targets. Pending consumers, and for a
/// non-human actor consumers that would refuse it, are skipped; the event
/// still reaches the rest.
fn implicit(
    roster: &Roster<'_>,
    input: &RouteInput<'_>,
    stored: &[ConsumerId],
    trace: &mut Trace,
) -> Result<Vec<ConsumerTarget>, RouteError> {
    // Bounded at write time; checked again so a stored list can never widen
    // an event beyond max_targets.
    let mut unique = Targets::new(roster.max_targets());
    for id in stored {
        unique.push(id)?;
    }
    let human = input.activity.actor.kind == ActorKind::Human;
    let mut targets = Vec::new();
    let mut ignored = false;
    for id in unique.into_ids() {
        // A target no stored consumer backs is no more routable than a pending one.
        let Some(member) = roster.get(Some(&id)).filter(|member| !member.view.pending) else {
            trace.push(skipped(id, "pending"));
            continue;
        };
        if !human && !member.view.non_human_effective(input.non_human_providers) {
            ignored = true;
            trace.push(skipped(id, "ignores_actor_kind"));
            continue;
        }
        targets.push(member.target());
    }
    match (targets.is_empty(), ignored) {
        (false, _) => Ok(targets),
        (true, true) => Err(RouteError::NoTargetAcceptsActorKind),
        (true, false) => Err(RouteError::ConsumerUnavailable),
    }
}

fn skipped(consumer: ConsumerId, reason: &'static str) -> TraceStep {
    TraceStep::Skipped { consumer, reason }
}
