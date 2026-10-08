//! Custom event targets named by the `cmaegtargets` CloudEvent attribute.
//!
//! The name is 12 lowercase alphanumerics, a valid CloudEvents extension
//! name, so binary mode carries it as `ce-cmaegtargets` and structured mode
//! as a top-level attribute. On a channel without routing the attribute stays
//! in the event and means nothing, as it always has.
use serde_json::Value;

use super::{
    roster::{token_payload, Roster, Targets},
    RouteError, RouteInput,
};
use crate::{activity::ActivityKind, channel::ChannelDefinition, ids::ConsumerId};

/// The extension attribute that names a custom event's targets.
pub const TARGETS_ATTRIBUTE: &str = "cmaegtargets";
/// The longest `cmaegtargets` value, in bytes.
pub const MAX_TARGETS_ATTRIBUTE_BYTES: usize = 256;

/// The consumers a custom event names in `cmaegtargets`, in order and
/// without duplicates, or `None` when it names none or the channel has no
/// routing.
///
/// The value is 1..=`max_targets` comma-separated names or ids, each
/// trimmed. HMAC signs only `webhook-id.timestamp.body`, so in binary mode
/// every `ce-*` attribute arrives in unsigned headers and a replayed request
/// could swap the targets: such an event is refused when it names targets.
/// `mentionable` restricts chat users, not authenticated producers.
pub fn read(
    def: &ChannelDefinition,
    input: &RouteInput<'_>,
) -> Result<Option<Vec<ConsumerId>>, RouteError> {
    match Roster::new(def) {
        Some(roster) => targets(&roster, input),
        None => Ok(None),
    }
}

/// `read` against a routed channel's roster.
pub(super) fn targets(
    roster: &Roster<'_>,
    input: &RouteInput<'_>,
) -> Result<Option<Vec<ConsumerId>>, RouteError> {
    let ActivityKind::Event { ce, .. } = &input.activity.kind else {
        return Ok(None);
    };
    let Some(value) = ce.extensions.get(TARGETS_ATTRIBUTE) else {
        return Ok(None);
    };
    if !roster.policy.addressing.event_field {
        return Err(RouteError::TargetsNotPermitted);
    }
    let Value::String(value) = value else {
        return Err(RouteError::InvalidTargets);
    };
    if value.len() > MAX_TARGETS_ATTRIBUTE_BYTES {
        return Err(RouteError::InvalidTargets);
    }
    let tokens: Vec<&str> = value.split(',').map(str::trim).collect();
    check_count(roster, &tokens)?;
    if !input.attributes_signed {
        return Err(RouteError::TargetsUnsigned);
    }
    resolve(roster, &tokens).map(Some)
}

/// A hand-off's target tokens, resolved as `cmaegtargets` is but without
/// the signed-attribute rule: the publisher authenticated as a consumer.
pub(super) fn published(
    roster: &Roster<'_>,
    tokens: &[String],
) -> Result<Vec<ConsumerId>, RouteError> {
    let tokens: Vec<&str> = tokens.iter().map(|token| token.trim()).collect();
    check_count(roster, &tokens)?;
    resolve(roster, &tokens)
}

/// Every token must be non-empty, and there may be 1..=`max_targets` of
/// them; checked before any is resolved, so resolution stays bounded.
fn check_count(roster: &Roster<'_>, tokens: &[&str]) -> Result<(), RouteError> {
    if tokens.is_empty() || tokens.iter().any(|token| token.is_empty()) {
        return Err(RouteError::InvalidTargets);
    }
    if tokens.len() > roster.max_targets() {
        return Err(RouteError::TooManyTargets);
    }
    Ok(())
}

/// Each token must name a routable consumer, by name or id.
fn resolve(roster: &Roster<'_>, tokens: &[&str]) -> Result<Vec<ConsumerId>, RouteError> {
    let mut targets = Targets::new(roster.max_targets());
    for token in tokens {
        let member = roster
            .named(token)
            .ok_or_else(|| RouteError::TargetUnknown(token_payload(token)))?;
        targets.push(member.id())?;
    }
    Ok(targets.into_ids())
}
