//! Conservative retained-cache charges, including spare allocation capacity.
//!
//! Serialization misses collection buffers and their element headers. Count
//! those recursively; the enclosing object/buffer already pays inline headers.
//! These are budget charges, not allocator-specific measurements of process RSS.
use std::mem::size_of;

use cmaeg_core::{
    channel::{Addressing, AgentEventPolicy, SourceId},
    ids::ConsumerId,
};

use super::{
    detail::{ChannelDetail, ReplyView, SecretsView, SourceView},
    routing_view::{DefaultReplyView, RoutingView, RuleView},
    summary::{ChannelSummary, ConsumerChip},
};

/// Cache nodes, the key's inline representation and Arc bookkeeping. The
/// normalized record key's allocation is charged separately at its full bound.
const ENTRY_OVERHEAD_BYTES: u64 = 512;
const ALLOCATION_GRANULE: u64 = 64;
const ALLOCATION_OVERHEAD: u64 = 64;

pub(super) fn resident_charge(summary: &ChannelSummary) -> u64 {
    ENTRY_OVERHEAD_BYTES
        .saturating_add(allocation(cmaeg_core::ids::MAX_ID_BYTES))
        .saturating_add(allocation(size_of::<ChannelSummary>()))
        .saturating_add(summary.heap())
}

fn allocation(capacity: usize) -> u64 {
    if capacity == 0 {
        return 0;
    }
    u64::try_from(capacity)
        .unwrap_or(u64::MAX)
        .div_ceil(ALLOCATION_GRANULE)
        .saturating_mul(ALLOCATION_GRANULE)
        .saturating_add(ALLOCATION_OVERHEAD)
}

trait Heap {
    fn heap(&self) -> u64;
}

impl Heap for String {
    fn heap(&self) -> u64 {
        allocation(self.capacity())
    }
}
impl Heap for &'static str {
    fn heap(&self) -> u64 {
        0
    }
}
impl<T: Heap> Heap for Option<T> {
    fn heap(&self) -> u64 {
        self.as_ref().map_or(0, Heap::heap)
    }
}
impl<T: Heap> Heap for Vec<T> {
    fn heap(&self) -> u64 {
        self.iter().fold(
            allocation(self.capacity().saturating_mul(size_of::<T>())),
            |bytes, value| bytes.saturating_add(value.heap()),
        )
    }
}

// These private string wrappers copy validated input: consumers have at most
// 16 bytes and source slots at most 14. Their constructors/clones cannot retain
// arbitrary caller capacity. Pay a full 64-byte payload plus allocation costs;
// enclosing structs or vectors already pay their String headers.
impl Heap for ConsumerId {
    fn heap(&self) -> u64 {
        allocation(64)
    }
}
impl Heap for SourceId {
    fn heap(&self) -> u64 {
        allocation(64)
    }
}

// Exhaustive destructuring makes a new field require an accounting decision.
// Fields after the semicolon contain no owned allocations; their inline size
// is already included in their enclosing object or vector buffer.
macro_rules! heap_fields {
    ($kind:ty { $($owned:ident),* ; $($inline:ident),* $(,)? }) => {
        impl Heap for $kind {
            fn heap(&self) -> u64 {
                let Self { $($owned,)* $($inline: _,)* } = self;
                0u64 $(.saturating_add($owned.heap()))*
            }
        }
    };
}

heap_fields!(ChannelSummary {
    owner, name, created_at, sources, consumers, warnings, detail;
    owner_kind, routed, default_receives_all, status, record_bytes
});
heap_fields!(ConsumerChip {
    id, name, label, slug, key_hash, target, binding_id, publish;
    pending, level, auto_final, destination, mentionable, non_human, non_human_set
});
heap_fields!(ChannelDetail {
    reply, sources, secrets, routing;
    partition, custom_key, custom_key_inert, connection_managed
});
heap_fields!(ReplyView { sinks; auto_final, ack_after_ms });
heap_fields!(SourceView {
    id, bot_username, bot_id, admission, auth, key_ids, settings;
    kind, platform, route_count
});
heap_fields!(SecretsView {
    agent_key_hash, hmac_key_ids;
    agent_key, channel_key, telegram_bot_token, telegram_webhook_secret
});
heap_fields!(RoutingView {
    default_targets, rules, default_reply, addressing, agent_events;
    max_targets, reply_label
});
heap_fields!(RuleView { id, sources, matches, targets; active });
heap_fields!(DefaultReplyView { source, description; });

heap_fields!(Addressing { ; mentions, command, event_field, unknown_target_notice });
heap_fields!(AgentEventPolicy { ; max_hops, per_consumer_per_minute, per_channel_per_minute });
