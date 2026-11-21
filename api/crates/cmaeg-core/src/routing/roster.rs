//! The consumers a routing decision may select, and the target set it builds.
use std::iter;

use super::RouteError;
use crate::{
    activity::ConsumerTarget,
    channel::{ChannelDefinition, ConsumerView, RoutingPolicy},
    ids::{ConsumerId, ConsumerName},
};

/// The most bytes of a user token a refusal carries.
const MAX_TOKEN_PAYLOAD_BYTES: usize = 64;

/// One consumer: its stored name and the view its routing flags come from.
pub(super) struct Member<'a> {
    pub(super) name: &'a ConsumerName,
    pub(super) view: ConsumerView<'a>,
}

impl Member<'_> {
    pub(super) fn id(&self) -> &ConsumerId {
        self.view.id
    }

    /// The member as `Activity.routing` names a target.
    pub(super) fn target(&self) -> ConsumerTarget {
        ConsumerTarget {
            id: self.id().clone(),
            name: self.name.clone(),
        }
    }
}

/// A routed channel's policy and its consumers, `default` first: eight at most.
pub(super) struct Roster<'a> {
    pub(super) policy: &'a RoutingPolicy,
    members: Vec<Member<'a>>,
}

impl<'a> Roster<'a> {
    /// `None` for a channel without routing.
    pub(super) fn new(def: &'a ChannelDefinition) -> Option<Self> {
        let policy = def.routing.as_ref()?;
        let default = Member {
            name: &policy.default_consumer.name,
            view: def.consumer(None)?,
        };
        let others = policy.consumers.iter().filter_map(|consumer| {
            Some(Member {
                name: &consumer.name,
                view: def.consumer(Some(&consumer.id))?,
            })
        });
        Some(Self {
            policy,
            members: iter::once(default).chain(others).collect(),
        })
    }

    /// `None` gives `default`, as `ChannelDefinition::consumer` does; any
    /// other id, the consumer stored under it, pending or not.
    pub(super) fn get(&self, id: Option<&ConsumerId>) -> Option<&Member<'a>> {
        match id {
            None => self.members.first(),
            Some(id) => self.members.iter().find(|member| member.id() == id),
        }
    }

    /// The routable consumer a user token names: by name, case-insensitively,
    /// or by exact id. A pending consumer is never routed, so it is unknown.
    pub(super) fn named(&self, token: &str) -> Option<&Member<'a>> {
        self.members.iter().find(|member| {
            !member.view.pending
                && (member.name.as_str().eq_ignore_ascii_case(token)
                    || member.id().as_str() == token)
        })
    }

    pub(super) fn max_targets(&self) -> usize {
        usize::from(self.policy.max_targets)
    }
}

/// Targets in order and without duplicates, refused beyond `max_targets`.
pub(super) struct Targets {
    ids: Vec<ConsumerId>,
    max: usize,
}

impl Targets {
    pub(super) fn new(max: usize) -> Self {
        Self {
            ids: Vec::new(),
            max,
        }
    }

    /// Add `id` unless it is already present; a new target beyond the limit
    /// is refused, so the set never holds more than `max`.
    pub(super) fn push(&mut self, id: &ConsumerId) -> Result<(), RouteError> {
        if self.ids.contains(id) {
            return Ok(());
        }
        if self.ids.len() == self.max {
            return Err(RouteError::TooManyTargets);
        }
        self.ids.push(id.clone());
        Ok(())
    }

    pub(super) fn into_ids(self) -> Vec<ConsumerId> {
        self.ids
    }
}

/// A user token as a refusal carries it: at most 64 bytes, cut on a char
/// boundary.
pub(super) fn token_payload(token: &str) -> String {
    let mut end = token.len().min(MAX_TOKEN_PAYLOAD_BYTES);
    while !token.is_char_boundary(end) {
        end -= 1;
    }
    token[..end].to_owned()
}
