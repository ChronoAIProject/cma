//! Rule matching: the first active rule whose match lists all accept an event.
use super::{Trace, TraceStep};
use crate::{
    activity::{Activity, ActivityKind},
    channel::{ActivityKindName, ChannelDefinition, RoutingRule, SourceId},
};

/// Whether `rule` matches the event that arrived on `source_id`, and if not,
/// the first match list that refused it.
///
/// AND across non-empty lists, OR within a list. `chats` and `senders`
/// compare exactly; `event_types` and `event_sources` apply only to the
/// `event` kind, so a rule that lists them never matches anything else.
pub fn matches(
    rule: &RoutingRule,
    activity: &Activity,
    source_id: &SourceId,
) -> (bool, Option<&'static str>) {
    let when = &rule.when;
    let event = match &activity.kind {
        ActivityKind::Event { ce, .. } => Some(ce),
        _ => None,
    };
    let kind = kind_name(&activity.kind);
    let lists = [
        ("sources", listed(&when.sources, |id| id == source_id)),
        ("kinds", listed(&when.kinds, |listed| *listed == kind)),
        (
            "chats",
            listed(&when.chats, |chat| *chat == activity.conversation.id),
        ),
        (
            "senders",
            listed(&when.senders, |sender| *sender == activity.actor.id),
        ),
        (
            "event_types",
            listed(&when.event_types, |pattern| {
                event.is_some_and(|ce| pattern.matches(&ce.r#type))
            }),
        ),
        (
            "event_sources",
            listed(&when.event_sources, |pattern| {
                event.is_some_and(|ce| pattern.matches(&ce.source))
            }),
        ),
    ];
    match lists.into_iter().find(|(_, accepted)| !accepted) {
        Some((list, _)) => (false, Some(list)),
        None => (true, None),
    }
}

/// The first matching active rule of `def`'s policy, in list order, or
/// `None` without one. Every rule considered is recorded in `trace`; an
/// inactive rule, one naming only detached sources, is recorded and skipped.
pub fn first_match<'a>(
    def: &'a ChannelDefinition,
    activity: &Activity,
    trace: &mut Trace,
) -> Option<&'a RoutingRule> {
    let routing = def.routing.as_ref()?;
    let source_id = activity.source.source_id();
    for rule in &routing.rules {
        let inactive = !rule.is_active(def);
        let (matched, failed_on) = if inactive {
            (false, None)
        } else {
            matches(rule, activity, &source_id)
        };
        trace.push(TraceStep::RuleTried {
            rule_id: rule.id.clone(),
            matched,
            inactive,
            failed_on,
        });
        if matched {
            return Some(rule);
        }
    }
    None
}

/// An empty list admits every event; otherwise one entry must accept it.
fn listed<T>(list: &[T], accepts: impl Fn(&T) -> bool) -> bool {
    list.is_empty() || list.iter().any(accepts)
}

fn kind_name(kind: &ActivityKind) -> ActivityKindName {
    match kind {
        ActivityKind::Message { .. } => ActivityKindName::Message,
        ActivityKind::Command { .. } => ActivityKindName::Command,
        ActivityKind::Interaction(_) => ActivityKindName::Interaction,
        ActivityKind::Event { .. } => ActivityKindName::Event,
    }
}
