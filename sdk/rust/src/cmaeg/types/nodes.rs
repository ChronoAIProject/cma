//! Public channel-node diagnostics and policy-only routing requests.
use super::*;
use serde::{Deserialize, Serialize};

/// The result of a configuration check; no external delivery is performed.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceCheckResponse {
    /// Saved producer slot.
    pub source_index: u64,
    /// Configured platform.
    pub platform: String,
    /// Always configuration_only for this operation.
    pub scope: String,
    /// Always false for this operation.
    pub external_delivery_tested: bool,
    /// Individual configuration checks.
    pub checks: Vec<SourceCheck>,
}
/// One source configuration diagnostic, without credential values.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceCheck {
    /// Stable diagnostic identifier.
    pub id: String,
    /// Human-readable label.
    pub label: String,
    /// Check result.
    pub status: SourceCheckStatus,
    /// Explanation and limits of the check.
    pub detail: String,
}
/// Source diagnostic result.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceCheckStatus {
    /// Configuration reference is present.
    Passed,
    /// Missing reference or check not performed.
    Unavailable,
}
/// Mutable policy for an already provisioned consumer.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ConsumerPolicyRequest {
    /// Existing consumer identifier.
    pub id: ConsumerId,
    /// Addressable consumer name.
    pub name: ConsumerName,
    /// Reply label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Whether explicit mentions address this consumer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mentionable: Option<bool>,
    /// Non-human actor policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub non_human: Option<bool>,
    /// Reply policy override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<ConsumerReply>,
    /// Hand-off publishing policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish: Option<PublishPolicy>,
}
/// Routing policy without server-owned consumer bindings or credentials.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutingPolicyRequest {
    /// Policy schema version.
    pub version: u64,
    /// Default consumer presentation and policy.
    pub default_consumer: DefaultConsumer,
    /// Exact existing additional consumer ids in their original order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumers: Option<Vec<ConsumerPolicyRequest>>,
    /// First-match routing rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<RoutingRule>>,
    /// Fallback destinations.
    pub default_targets: Vec<ConsumerId>,
    /// Explicit addressing policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addressing: Option<Addressing>,
    /// Upper bound on fanout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_targets: Option<u64>,
    /// Reply labeling policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_label: Option<ReplyLabel>,
    /// Default reply destination.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_reply: Option<DefaultReply>,
    /// Hand-off event limits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_events: Option<AgentEventPolicy>,
}
/// Version-fenced routing replacement.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutingUpdateRequest {
    /// Expected saved channel version.
    pub expected_version: u64,
    /// Desired policy for the existing consumers.
    pub routing: RoutingPolicyRequest,
}
