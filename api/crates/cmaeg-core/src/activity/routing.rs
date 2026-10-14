//! The routing decision bound into a routed activity.
//!
//! The gateway resolves an event's consumers once, at ingress, and records
//! the decision in the activity itself. Every targeted consumer then receives
//! the same activity and digest, and the decision reaches providers unchanged:
//! CMA carries `activity` opaquely and refuses unknown top-level context
//! fields, so a field inside the activity is the only place it can travel.
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::ActivityKind;
use crate::{
    channel::{ReplyChoice, SourceId, MAX_TARGETS},
    ids::{ConsumerId, ConsumerName, EventId, RuleId},
    ValidationError,
};

/// The longest hand-off chain: a hand-off's `hop` is 1..=4.
pub const MAX_HOPS: u8 = 4;
/// The command whose arguments carry explicit consumer addressing.
const ADDRESSING_COMMAND: &str = "to";

/// Which consumers an event reaches and why. Present only on routed events;
/// an activity without it is exactly the activity a channel without routing
/// has always produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ActivityRouting {
    /// The source slot the event arrived on; `agent` for a hand-off.
    pub source_id: SourceId,
    pub resolution: RouteResolution,
    /// The rule that selected the targets, when a rule did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<RuleId>,
    /// 1..=8 consumers with unique ids. `targets[0]` is the primary consumer.
    pub targets: Vec<ConsumerTarget>,
    /// Byte offset into the raw message text (into `args` for a `to` command)
    /// where the unaddressed remainder begins: leading bot addresses, mention
    /// keys and consumer addressing come before it. Omitted when 0.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub addressing_bytes: u16,
    /// Present exactly on hand-off events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub causation: Option<Causation>,
}

/// How an event's targets were resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RouteResolution {
    /// The policy's `default_targets`.
    Default,
    /// A routing rule, named by `rule_id`.
    Rule,
    /// A leading `@name` mention.
    Mention,
    /// A `/to name` command.
    Command,
    /// A consumer named by a custom event field.
    EventField,
    /// The consumer that owns the clicked card.
    Interaction,
    /// A hand-off published by another consumer.
    AgentEvent,
}

/// One consumer an event reaches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ConsumerTarget {
    pub id: ConsumerId,
    pub name: ConsumerName,
}

/// What caused a hand-off: the event it continues and the chain it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Causation {
    /// The event whose turn published this one.
    pub parent_event_id: EventId,
    /// The chain's record-scoped root id: 32 lowercase hex digits.
    #[schema(pattern = "^[0-9a-f]{32}$")]
    pub root: String,
    /// The consumer that published this event.
    pub published_by: ConsumerId,
    /// The hand-off's position in its chain, 1..=4.
    #[schema(minimum = 1, maximum = 4)]
    pub hop: u8,
    /// Where the targets' automatic replies go.
    pub reply: ReplyChoice,
    /// The source slot the chain's first event arrived on.
    pub origin_source_id: SourceId,
}

impl ActivityRouting {
    /// Check the decision against the activity it is bound into.
    pub(super) fn validate(&self, kind: &ActivityKind) -> Result<(), ValidationError> {
        let mut ids = BTreeSet::new();
        if !(1..=usize::from(MAX_TARGETS)).contains(&self.targets.len())
            || !self.targets.iter().all(|target| ids.insert(&target.id))
        {
            return Err(ValidationError::new(
                "/routing/targets",
                "must name 1 to 8 consumers with unique ids",
            ));
        }
        self.validate_addressing(kind)?;
        let handoff = self.resolution == RouteResolution::AgentEvent;
        if handoff != self.causation.is_some() || handoff != self.source_id.is_agent() {
            return Err(ValidationError::new(
                "/routing/causation",
                "is present exactly when the resolution is agent_event and the source is agent",
            ));
        }
        if let Some(causation) = &self.causation {
            causation.validate()?;
        }
        Ok(())
    }

    fn validate_addressing(&self, kind: &ActivityKind) -> Result<(), ValidationError> {
        let addressed = match kind {
            ActivityKind::Message { text, .. } => Some(text.as_str()),
            ActivityKind::Command { name, args } if name == ADDRESSING_COMMAND => {
                Some(args.as_str())
            }
            _ => None,
        };
        let offset = usize::from(self.addressing_bytes);
        let valid = match addressed {
            Some(text) => text.is_char_boundary(offset),
            None => offset == 0,
        };
        if valid {
            Ok(())
        } else {
            Err(ValidationError::new(
                "/routing/addressing_bytes",
                "must be a character boundary within the message text or `to` arguments, and 0 otherwise",
            ))
        }
    }
}

impl Causation {
    fn validate(&self) -> Result<(), ValidationError> {
        if !(1..=MAX_HOPS).contains(&self.hop) {
            return Err(ValidationError::new(
                "/routing/causation/hop",
                "must be 1 to 4",
            ));
        }
        if !is_root_id(&self.root) {
            return Err(ValidationError::new(
                "/routing/causation/root",
                "must be 32 lowercase hex digits",
            ));
        }
        Ok(())
    }
}

/// Whether `value` has the shape of a record-scoped root id.
pub(crate) fn is_root_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_zero(value: &u16) -> bool {
    *value == 0
}
