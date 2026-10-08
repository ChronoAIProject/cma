//! Per-event delivery state, with one entry for each consumer an event reaches.
//!
//! Every command of an event reaches one worker, sharded by
//! `key(channel, event)`, so an event's consumers share one [`EventGroup`] and
//! their outcomes, tool replies and notices run in arrival order without a
//! lock. The ack and heartbeats belong to the event; answers, auto-final
//! suppression and failure notices belong to each consumer. An unrouted
//! event's group always holds exactly one consumer and then behaves exactly as
//! the single per-event state did before consumers existed (I6).
use super::event::EventKey;
use crate::channels::resolver::ResolvedChannel;
use cmaeg_core::{
    channel::{EffectiveReply, ReplyChoice},
    ids::ConsumerId,
    seal::EventRefPayload,
};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::time::Instant;

/// The most [`ConsumerDelivery`] entries one worker holds across its groups.
/// It bounds consumers rather than events, so a routed event costs its
/// consumers' share and an unrouted event exactly what it always did.
pub(super) const MAX_EVENTS: usize = 8192;
/// How long a terminal group is kept, so late commands still find it.
const TERMINAL_RETENTION: Duration = Duration::from_secs(30);

pub(super) struct EventGroup {
    pub trace: tracing::Span,
    pub channel: Arc<ResolvedChannel>,
    pub ack_sent: bool,
    pub ack_due: Option<Instant>,
    pub first_send_done: bool,
    pub last_progress_at: Instant,
    pub heartbeats: u8,
    /// When the group last became terminal. Set at creation too, so a group
    /// whose turns never start still expires.
    pub terminal_at: Option<Instant>,
    /// Lineage present: a hand-off. No ack, no heartbeats; no human is waiting.
    pub handoff: bool,
    pub consumers: Vec<ConsumerDelivery>,
}

pub(super) struct ConsumerDelivery {
    pub consumer: ConsumerId,
    pub event: EventRefPayload,
    /// `consumer.reply.auto_final ?? channel.reply.auto_final`, from the
    /// group's current channel snapshot.
    pub policy: EffectiveReply,
    /// Whether this consumer's turn is routed (`activity.routing` present).
    /// Fixed for the turn's life; false until its first outcome arrives.
    pub routed_turn: bool,
    pub tool_replied: bool,
    pub answer_attempted: bool,
    pub terminal: bool,
    pub last_output_at: Instant,
}

impl ConsumerDelivery {
    /// Still owed an answer: neither replied to nor finished.
    pub fn pending(&self) -> bool {
        !self.terminal && !self.tool_replied && !self.answer_attempted
    }
}

/// The consumer a reference authorizes; an absent one is `default`.
pub(super) fn consumer_of(event: &EventRefPayload) -> ConsumerId {
    event
        .consumer
        .clone()
        .unwrap_or_else(ConsumerId::default_consumer)
}

/// `consumer`'s reply policy under `channel`, or `None` when the snapshot no
/// longer defines that consumer.
fn policy_of(channel: &ResolvedChannel, consumer: &ConsumerId) -> Option<EffectiveReply> {
    channel
        .definition
        .consumer(Some(consumer))
        .map(|view| view.reply)
}

impl EventGroup {
    /// A group holding one [`ConsumerDelivery`], for the event's own consumer.
    pub fn new(channel: Arc<ResolvedChannel>, event: EventRefPayload) -> Self {
        let now = Instant::now();
        let mut group = Self {
            trace: tracing::Span::current(),
            handoff: event.lineage.is_some(),
            channel,
            ack_sent: false,
            ack_due: None,
            first_send_done: false,
            last_progress_at: now,
            heartbeats: 0,
            terminal_at: Some(now),
            consumers: Vec::with_capacity(1),
        };
        group.consumer_mut(&event);
        group
    }

    /// The index of `event`'s consumer, joined with `event` if it is new.
    pub fn consumer_mut(&mut self, event: &EventRefPayload) -> usize {
        let consumer = consumer_of(event);
        if let Some(index) = self.position(&consumer) {
            return index;
        }
        // A consumer the snapshot no longer defines (revoked mid-turn) keeps
        // the channel's own policy and the origin; it is never routed elsewhere.
        let policy = policy_of(&self.channel, &consumer).unwrap_or(EffectiveReply {
            auto_final: self.channel.definition.reply.auto_final,
            destination: ReplyChoice::Origin,
        });
        self.consumers.push(ConsumerDelivery {
            consumer,
            event: event.clone(),
            policy,
            routed_turn: false,
            tool_replied: false,
            answer_attempted: false,
            terminal: false,
            last_output_at: Instant::now(),
        });
        self.consumers.len() - 1
    }

    fn position(&self, consumer: &ConsumerId) -> Option<usize> {
        self.consumers
            .iter()
            .position(|delivery| &delivery.consumer == consumer)
    }

    /// Replace the channel snapshot, as a tool reply carrying a newer one
    /// does, and re-read each consumer's policy from it.
    pub fn set_channel(&mut self, channel: Arc<ResolvedChannel>) {
        for delivery in &mut self.consumers {
            if let Some(policy) = policy_of(&channel, &delivery.consumer) {
                delivery.policy = policy;
            }
        }
        self.channel = channel;
    }

    /// A group is terminal only when every consumer it holds is.
    pub fn terminal(&self) -> bool {
        self.consumers.iter().all(|delivery| delivery.terminal)
    }

    /// Mark consumer `at` terminal. Once every consumer is, the group stops
    /// its timers and starts its retention.
    pub fn finish(&mut self, at: usize) {
        self.consumers[at].terminal = true;
        if self.terminal() {
            self.terminal_at = Some(Instant::now());
            self.ack_due = None;
        }
    }
}

/// A worker's groups and the running total of their consumer entries.
#[derive(Default)]
pub(super) struct Groups {
    groups: HashMap<EventKey, EventGroup>,
    entries: usize,
}

impl Groups {
    /// Whether a command for `consumer` of event `id` may proceed: when its
    /// group already holds that consumer or, after expiring retained groups
    /// once the total reaches the cap, while the total is below it.
    pub fn reserve(&mut self, id: &EventKey, consumer: &ConsumerId) -> bool {
        if self.entries >= MAX_EVENTS {
            self.expire();
        }
        self.holds(id, consumer) || self.entries < MAX_EVENTS
    }

    fn holds(&self, id: &EventKey, consumer: &ConsumerId) -> bool {
        self.groups
            .get(id)
            .is_some_and(|group| group.position(consumer).is_some())
    }

    /// The group of event `id`, created with `channel` if absent, and the
    /// index of `event`'s consumer in it, joined if absent. Call only for a
    /// command [`Groups::reserve`] admitted.
    pub fn join(
        &mut self,
        id: EventKey,
        channel: &Arc<ResolvedChannel>,
        event: &EventRefPayload,
    ) -> (&mut EventGroup, usize) {
        let entries = &mut self.entries;
        let group = self.groups.entry(id).or_insert_with(|| {
            *entries += 1;
            EventGroup::new(channel.clone(), event.clone())
        });
        let before = group.consumers.len();
        let at = group.consumer_mut(event);
        *entries += group.consumers.len() - before;
        (group, at)
    }

    /// Drop every group terminal for longer than the retention, releasing
    /// all of its entries.
    pub fn expire(&mut self) {
        let entries = &mut self.entries;
        self.groups.retain(|_, group| {
            let keep = group
                .terminal_at
                .is_none_or(|at| at.elapsed() < TERMINAL_RETENTION);
            if !keep {
                *entries -= group.consumers.len();
            }
            keep
        });
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut EventGroup> {
        self.groups.values_mut()
    }

    /// Consumer entries live across every group.
    #[cfg(test)]
    pub fn entries(&self) -> usize {
        self.entries
    }
}

#[cfg(test)]
#[path = "group_tests.rs"]
mod tests;
