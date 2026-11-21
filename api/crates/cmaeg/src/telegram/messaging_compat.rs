//! Validation-only compatibility for transparent default-only routing enrollment.
use cmaeg_core::{
    channel::{ChannelDefinition, RoutingPolicy},
    ids::ConsumerId,
};

pub(crate) fn default_routing() -> RoutingPolicy {
    serde_json::from_value(serde_json::json!({
        "version":1,"default_consumer":{"name":"default"},"consumers":[],"default_targets":["default"]
    })).expect("native default routing is valid")
}

/// Existing routing-bearing authorities retain their exact hashes. Only an
/// original unrouted default consumer may also validate against absent routing,
/// and only while the complete current policy is this exact automatic policy.
pub(crate) fn accepts_unrouted_default(
    definition: &ChannelDefinition,
    consumer: Option<&ConsumerId>,
) -> bool {
    consumer.is_none() && definition.routing.as_ref() == Some(&default_routing())
}
