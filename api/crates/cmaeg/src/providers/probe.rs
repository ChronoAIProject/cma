//! Provider Agent Card conformance classification.

use std::time::Duration;

use serde_json::Value;

use cmaeg_core::channel::ConformanceLevel;

#[derive(Debug, Clone, PartialEq)]
pub struct AgentCardProbe {
    pub card: Value,
    pub level: ConformanceLevel,
    pub latency: Duration,
}

pub(crate) fn classify_conformance(card: &Value) -> ConformanceLevel {
    let capabilities = card.get("capabilities");
    let has = |name: &str| match capabilities {
        Some(Value::Object(values)) => values
            .get(name)
            .is_some_and(|value| value.as_bool().unwrap_or(true)),
        Some(Value::Array(values)) => values.iter().any(|value| value.as_str() == Some(name)),
        _ => false,
    };
    if has("rich_input") {
        ConformanceLevel::L5
    } else if has("cancel") {
        ConformanceLevel::L4
    } else if has("client_tools") {
        ConformanceLevel::L3
    } else if has("activity_items") {
        ConformanceLevel::L2
    } else {
        ConformanceLevel::L1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_capabilities_from_objects_or_arrays() {
        assert_eq!(
            classify_conformance(&serde_json::json!({"capabilities":{"client_tools":true}})),
            ConformanceLevel::L3
        );
        assert_eq!(
            classify_conformance(&serde_json::json!({"capabilities":["cancel"]})),
            ConformanceLevel::L4
        );
        assert_eq!(
            classify_conformance(&serde_json::json!({})),
            ConformanceLevel::L1
        );
    }
}
