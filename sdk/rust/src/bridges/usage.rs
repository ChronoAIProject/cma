use super::{Bridges, validation};
use crate::{Error, Result};
use serde::{Deserialize, Deserializer};

string_enum! { /// Diagnostic capacity facts; none grants admission or cleanup.
    pub enum BridgeCapacityReason {
        PartitionLimitReached => "partition_limit_reached",
        SandboxReservationLimitReached => "sandbox_reservation_limit_reached",
        RetiredSandboxReservationsRetained => "retired_sandbox_reservations_retained",
        SharedParentPolicyApplies => "shared_parent_policy_applies",
        ResourceAccessRequired => "resource_access_required",
        InventoryLimit => "inventory_limit"
    }
}
/// The current Bridge's configured admission bounds.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeUsageLimits {
    /// Maximum retained partition heads.
    pub max_partitions: u32,
    /// Maximum retained allocations that create their own Sandbox.
    pub max_sandbox_reservations: u32,
}
/// Allocation records include retained predecessor generations.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeAllocationUsage {
    /// Retained generations.
    pub retained: u32,
    /// Current partition heads.
    pub current: u32,
    /// Allocations whose state is Ready, independent of runtime connectivity.
    pub ready: u32,
    /// Retired generations.
    pub retired: u32,
    /// Retained NewSandbox allocations, including retired generations.
    pub sandbox_reservations: u32,
}
/// Distinct resource model records referenced by this Bridge.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeResourceCount {
    /// Retained records; this is not proof of provider resources or ownership.
    pub retained: u32,
    /// Active durable lifecycle state with an Active parent Sandbox.
    pub active: u32,
}
/// Counts require current access to every existing target.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum BridgeResourceUsage {
    /// Complete distinct target counts, including reused parents.
    Available {
        /// Referenced Sandboxes.
        sandboxes: BridgeResourceCount,
        /// Referenced Workspaces.
        workspaces: BridgeResourceCount,
        /// Referenced Worktrees.
        worktrees: BridgeResourceCount,
        /// Referenced Agents.
        agents: BridgeResourceCount,
    },
    /// Current target access is insufficient; no partial counts are returned.
    Redacted {},
    /// The bounded inventory could not establish complete totals.
    InventoryLimited {},
}
/// Read-only usage under fresh Bridge management and target resource authority.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeUsage {
    /// Requested Bridge.
    pub bridge_id: String,
    /// Current management revision.
    pub bridge_revision: u64,
    /// Observation time in Unix milliseconds.
    pub observed_at_ms: u64,
    /// Maximum generations in one complete observation.
    pub inventory_limit: u32,
    /// Current configured bounds, not an admission promise.
    pub limits: BridgeUsageLimits,
    /// None when complete totals exceed the bounded read.
    #[serde(deserialize_with = "required_nullable")]
    pub allocations: Option<BridgeAllocationUsage>,
    /// Distinct current-authorized target counts or explicit unavailability.
    pub target_resources: BridgeResourceUsage,
    /// Fixed diagnostic reasons with no messages or secrets.
    pub capacity_reasons: Vec<BridgeCapacityReason>,
}
fn required_nullable<'de, D, T>(deserializer: D) -> std::result::Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

impl Bridges<'_> {
    /// Observe retained allocations and current target state. This GET never
    /// starts work, retries delivery, releases a reservation or changes quotas.
    pub async fn usage(&self, bridge_id: &str) -> Result<BridgeUsage> {
        validation::id(bridge_id)?;
        let value: BridgeUsage = self
            .client
            .get(&["bridges", bridge_id, "usage"], &[])
            .await?;
        value.check(bridge_id)?;
        Ok(value)
    }
}
impl BridgeUsage {
    fn check(&self, bridge_id: &str) -> Result<()> {
        use BridgeCapacityReason as Reason;
        let invalid = || Error::protocol("inconsistent Bridge usage observation");
        let limited = matches!(
            self.target_resources,
            BridgeResourceUsage::InventoryLimited { .. }
        );
        if self.bridge_id != bridge_id
            || self.bridge_revision == 0
            || self.bridge_revision > i64::MAX as u64
            || self.observed_at_ms > i64::MAX as u64
            || self.inventory_limit != 256
            || !(1..=100_000).contains(&self.limits.max_partitions)
            || !(1..=self.limits.max_partitions).contains(&self.limits.max_sandbox_reservations)
            || self
                .capacity_reasons
                .iter()
                .enumerate()
                .any(|(i, reason)| self.capacity_reasons[..i].contains(reason))
            || self.allocations.is_none() != limited
            || limited != self.capacity_reasons.contains(&Reason::InventoryLimit)
            || matches!(self.target_resources, BridgeResourceUsage::Redacted { .. })
                != self
                    .capacity_reasons
                    .contains(&Reason::ResourceAccessRequired)
        {
            return Err(invalid());
        }
        if self.allocations.is_none()
            && self.capacity_reasons.iter().any(|reason| {
                !matches!(
                    reason,
                    Reason::InventoryLimit | Reason::SharedParentPolicyApplies
                )
            })
        {
            return Err(invalid());
        }
        if let Some(counts) = &self.allocations {
            if self
                .capacity_reasons
                .contains(&Reason::RetiredSandboxReservationsRetained)
                && (counts.retired == 0 || counts.sandbox_reservations == 0)
            {
                return Err(invalid());
            }
            if counts.retained > self.inventory_limit
                || counts.current > counts.retained
                || counts
                    .ready
                    .checked_add(counts.retired)
                    .is_none_or(|n| n > counts.retained)
                || counts.sandbox_reservations > counts.retained
                || (counts.current >= self.limits.max_partitions)
                    != self
                        .capacity_reasons
                        .contains(&Reason::PartitionLimitReached)
                || (counts.sandbox_reservations >= self.limits.max_sandbox_reservations)
                    != self
                        .capacity_reasons
                        .contains(&Reason::SandboxReservationLimitReached)
            {
                return Err(invalid());
            }
            if let BridgeResourceUsage::Available {
                sandboxes,
                workspaces,
                worktrees,
                agents,
            } = &self.target_resources
            {
                for value in [sandboxes, workspaces, worktrees, agents] {
                    if value.active > value.retained || value.retained > counts.retained {
                        return Err(invalid());
                    }
                }
            }
        }
        Ok(())
    }
}
