//! Bounded current usage observations. These never grant admission or reclaim
//! retained capacity, and never expose a source message or resource identifier.
use super::*;
use crate::{
    bridge::{allocation::AllocationState, BridgeId},
    resource_access::{CallerContext, ResourceLineage},
    sandbox::{AgentStatus, SandboxStatus, WorkspaceStatus, WorktreeStatus},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, time::Duration};

pub const MAX_USAGE_ALLOCATIONS: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeUsageLimits {
    pub max_partitions: u32,
    pub max_sandbox_reservations: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeAllocationUsage {
    pub retained: u32,
    pub current: u32,
    pub ready: u32,
    pub retired: u32,
    /// Exactly the retained NewSandbox records counted by admission, including
    /// retired generations. This is not a count of running provider resources.
    pub sandbox_reservations: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeResourceCount {
    pub retained: u32,
    pub active: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum BridgeResourceUsage {
    Available {
        sandboxes: BridgeResourceCount,
        workspaces: BridgeResourceCount,
        worktrees: BridgeResourceCount,
        agents: BridgeResourceCount,
    },
    Redacted,
    InventoryLimited,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeCapacityReason {
    PartitionLimitReached,
    SandboxReservationLimitReached,
    RetiredSandboxReservationsRetained,
    SharedParentPolicyApplies,
    ResourceAccessRequired,
    InventoryLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeUsageView {
    pub bridge_id: BridgeId,
    pub bridge_revision: u64,
    pub observed_at_ms: i64,
    pub inventory_limit: u32,
    pub limits: BridgeUsageLimits,
    pub allocations: Option<BridgeAllocationUsage>,
    /// Distinct retained targets of this Bridge, including reused parents and
    /// existing Agents. This is not ownership of, or capacity in, those parents.
    pub target_resources: BridgeResourceUsage,
    pub capacity_reasons: Vec<BridgeCapacityReason>,
}

impl BridgePartitionService {
    pub async fn usage(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        bridge: &BridgeId,
    ) -> Result<BridgeUsageView, PartitionStatusError> {
        tokio::time::timeout(
            Duration::from_secs(15),
            self.usage_inner(context, credential, bridge),
        )
        .await
        .map_err(|_| PartitionStatusError::Unavailable)?
    }

    async fn usage_inner(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        bridge: &BridgeId,
    ) -> Result<BridgeUsageView, PartitionStatusError> {
        let mut manager = self.management(context, credential, bridge).await?;
        let config = &manager.record().current()?.config;
        let mut view = BridgeUsageView {
            bridge_id: bridge.clone(),
            bridge_revision: manager.record().revision,
            observed_at_ms: 0,
            inventory_limit: MAX_USAGE_ALLOCATIONS as u32,
            limits: BridgeUsageLimits {
                max_partitions: config.limits.max_partitions,
                max_sandbox_reservations: config.limits.max_sandboxes,
            },
            allocations: None,
            target_resources: BridgeResourceUsage::InventoryLimited,
            capacity_reasons: Vec::new(),
        };
        if config.placement.shared_capacity().is_some() {
            view.capacity_reasons
                .push(BridgeCapacityReason::SharedParentPolicyApplies);
        }
        let records = self.store.usage(&manager).await?;
        if let Some(records) = &records {
            if records.len() > MAX_USAGE_ALLOCATIONS {
                return Err(PartitionStatusError::Corrupt);
            }
            let mut counts = BridgeAllocationUsage::default();
            let mut targets = TargetCounts::default();
            let mut seen = std::collections::BTreeSet::new();
            let mut checked = Vec::new();
            let mut redacted = false;
            for evidence in records {
                validation::allocation(&manager, &evidence.allocation)?;
                if !seen.insert(&evidence.allocation.id) || evidence.observed_at_ms < 0 {
                    return Err(PartitionStatusError::Corrupt);
                }
                let record = &evidence.allocation;
                counts.retained += 1;
                counts.current += u32::from(evidence.current);
                counts.ready += u32::from(record.state == AllocationState::Ready);
                counts.retired += u32::from(record.state == AllocationState::Retired);
                counts.sandbox_reservations += u32::from(record.placement.creates_sandbox());
                if record.state == AllocationState::Retired
                    && record.placement.creates_sandbox()
                    && !view
                        .capacity_reasons
                        .contains(&BridgeCapacityReason::RetiredSandboxReservationsRetained)
                {
                    view.capacity_reasons
                        .push(BridgeCapacityReason::RetiredSandboxReservationsRetained);
                }
                let access = self
                    .authorize_targets(context, credential, &mut manager, evidence)
                    .await?;
                redacted |= access.redacted;
                for (_, _, resource) in &access.checked {
                    targets.observe(&resource.snapshot.lineage)?;
                }
                checked.push(access);
            }
            if counts.current >= view.limits.max_partitions {
                view.capacity_reasons
                    .push(BridgeCapacityReason::PartitionLimitReached);
            }
            if counts.sandbox_reservations >= view.limits.max_sandbox_reservations {
                view.capacity_reasons
                    .push(BridgeCapacityReason::SandboxReservationLimitReached);
            }
            view.allocations = Some(counts);
            view.target_resources = if redacted {
                view.capacity_reasons
                    .push(BridgeCapacityReason::ResourceAccessRequired);
                BridgeResourceUsage::Redacted
            } else {
                targets.view()
            };
            for access in &checked {
                self.revalidate_targets(context, credential, &manager, access)
                    .await?;
            }
            let current = self
                .store
                .usage(&manager)
                .await?
                .ok_or(PartitionStatusError::Conflict)?;
            if current.len() != records.len()
                || records.iter().zip(&current).any(|(old, new)| {
                    old.allocation != new.allocation
                        || old.current != new.current
                        || old.present != new.present
                })
            {
                return Err(PartitionStatusError::Conflict);
            }
        } else {
            view.capacity_reasons
                .push(BridgeCapacityReason::InventoryLimit);
        }
        self.revalidate_management(context, credential, &manager)
            .await?;
        view.observed_at_ms = i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| PartitionStatusError::Unavailable)?
                .as_millis(),
        )
        .map_err(|_| PartitionStatusError::Unavailable)?;
        Ok(view)
    }
}

#[derive(Default)]
struct TargetCounts {
    sandboxes: BTreeMap<String, (u64, bool)>,
    workspaces: BTreeMap<String, (u64, bool)>,
    worktrees: BTreeMap<String, (u64, bool)>,
    agents: BTreeMap<String, (u64, bool)>,
}
impl TargetCounts {
    fn observe(&mut self, lineage: &ResourceLineage) -> Result<(), PartitionStatusError> {
        let active = lineage.sandbox.status == SandboxStatus::Active;
        insert(
            &mut self.sandboxes,
            lineage.sandbox.id.as_str(),
            lineage.sandbox.revision,
            active,
        )?;
        if let Some(value) = &lineage.workspace {
            insert(
                &mut self.workspaces,
                value.id.as_str(),
                value.revision,
                active && value.status == WorkspaceStatus::Ready,
            )?;
        }
        if let Some(value) = &lineage.worktree {
            insert(
                &mut self.worktrees,
                value.id.as_str(),
                value.revision,
                active && value.status == WorktreeStatus::Ready,
            )?;
        }
        if let Some(value) = &lineage.agent {
            insert(
                &mut self.agents,
                value.id.as_str(),
                value.revision,
                active
                    && matches!(
                        value.status,
                        AgentStatus::Ready | AgentStatus::Working | AgentStatus::WaitingApproval
                    ),
            )?;
        }
        Ok(())
    }
    fn view(self) -> BridgeResourceUsage {
        fn count(values: BTreeMap<String, (u64, bool)>) -> BridgeResourceCount {
            BridgeResourceCount {
                retained: values.len() as u32,
                active: values.values().filter(|(_, active)| *active).count() as u32,
            }
        }
        BridgeResourceUsage::Available {
            sandboxes: count(self.sandboxes),
            workspaces: count(self.workspaces),
            worktrees: count(self.worktrees),
            agents: count(self.agents),
        }
    }
}
fn insert(
    values: &mut BTreeMap<String, (u64, bool)>,
    id: &str,
    revision: u64,
    active: bool,
) -> Result<(), PartitionStatusError> {
    let value = (revision, active);
    if values
        .insert(id.to_owned(), value)
        .is_some_and(|previous| previous != value)
    {
        return Err(PartitionStatusError::Conflict);
    }
    Ok(())
}
