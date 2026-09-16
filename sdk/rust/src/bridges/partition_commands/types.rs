use crate::resource_access::ResourceActor;
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Explicit change to one current partition binding.
pub enum BridgePartitionCommandAction {
    /// Reserve the next binding generation, retaining the original history.
    Reset,
    /// Block future input until an explicit reset.
    Detach,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// State captured atomically when the command was accepted.
pub enum BridgePartitionCommandState {
    /// Successor is durably reserved and awaits provisioning.
    Reserved,
    /// Successor is bound to the existing Agent.
    Ready,
    /// Original binding is retired; retained work may still drain.
    Retired,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Exact revisions to retain with the command key after response loss.
pub struct BridgePartitionCommandRequest {
    /// Bridge definition revision used for the original acceptance.
    pub expected_bridge_revision: u64,
    /// Current allocation revision the command must replace.
    pub expected_revision: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Identity and state of one affected generation.
pub struct BridgePartitionCommandResult {
    /// Stable durable identity.
    pub id: String,
    /// Partition generation, independently bounded from one through 256.
    pub generation: u64,
    /// Allocation revision captured at acceptance.
    pub revision: u64,
    /// Allocation state captured at acceptance.
    pub state: BridgePartitionCommandState,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Permanent acceptance receipt; later progress never rewrites this result.
pub struct BridgePartitionCommandReceipt {
    /// Stable durable identity.
    pub id: String,
    /// Original Bridge identity.
    pub bridge_id: String,
    /// Server digest of the exact path, action and revisions.
    pub request_sha256: String,
    /// Current owner or administrator who accepted the command.
    #[serde(deserialize_with = "manager")]
    pub actor: ResourceActor,
    /// Accepted reset or detach action.
    pub action: BridgePartitionCommandAction,
    /// Bridge definition revision used for the original acceptance.
    pub expected_bridge_revision: u64,
    /// Exact original allocation identity from the command path.
    pub allocation_id: String,
    /// Partition generation, independently bounded from one through 256.
    pub generation: u64,
    /// Original allocation revision before acceptance.
    pub previous_revision: u64,
    /// Retired original generation, with its advanced revision.
    pub result: BridgePartitionCommandResult,
    /// New generation for reset, or null for detach.
    #[serde(deserialize_with = "required_nullable")]
    pub successor: Option<BridgePartitionCommandResult>,
    /// Original command acceptance time in Unix milliseconds.
    pub created_at_ms: u64,
}

fn required_nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> std::result::Result<Option<T>, D::Error> {
    Option::deserialize(deserializer)
}

fn manager<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<ResourceActor, D::Error> {
    #[derive(Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
    enum Manager {
        Human {
            subject: String,
        },
        Api {
            subject: String,
            #[serde(deserialize_with = "required_nullable")]
            delegation_id: Option<String>,
        },
    }
    Ok(match Manager::deserialize(deserializer)? {
        Manager::Human { subject } => ResourceActor::Human { subject },
        Manager::Api {
            subject,
            delegation_id,
        } => ResourceActor::Api {
            subject,
            delegation_id,
        },
    })
}
