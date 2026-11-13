//! Negotiation is a provider acknowledgement, never a client header assertion.
use super::ProviderClient;
use cmaeg_core::channel::ProviderBinding;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

pub(crate) const CAPABILITY: &str = "nyxid_turn_trace_v1";

/// Malformed, future or unsolicited echoes keep legacy behavior. Retain only
/// the fixed token we requested; no arbitrary provider strings are persisted.
pub(super) fn read_echo<'de, D: Deserializer<'de>>(input: D) -> Result<Vec<String>, D::Error> {
    let value = Value::deserialize(input)?;
    Ok(
        if value
            .as_array()
            .is_some_and(|items| items.len() == 1 && items[0] == CAPABILITY)
        {
            vec![CAPABILITY.into()]
        } else {
            Vec::new()
        },
    )
}

impl ProviderClient {
    pub(crate) fn turn_trace_capability(&self) -> Option<&'static str> {
        self.turn_trace_enabled.then_some(CAPABILITY)
    }

    pub fn nyxid_trace_contract(&self, binding: &ProviderBinding) -> bool {
        self.turn_trace_enabled
            && binding.slug.as_str() == "nyxbot"
            && binding.capabilities.len() == 1
            && binding.capabilities[0] == CAPABILITY
    }
}
