use super::super::{BridgeAgentTarget, validation as bridge};
use super::*;
use crate::resource_access::ResourceActor;

fn prefixed(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|hash| {
        hash.len() == 64
            && hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
pub(super) fn allocation_id(value: &str) -> Result<()> {
    prefixed(value, "bral_")
        .then_some(())
        .ok_or_else(|| Error::validation("invalid allocation identity"))
}
pub(super) fn cursor(value: &str) -> bool {
    !value.is_empty() && value.len() <= 1024 && !value.chars().any(char::is_control)
}
fn target(value: &BridgeAgentTarget) -> bool {
    [
        &value.sandbox_id,
        &value.workspace_id,
        &value.worktree_id,
        &value.agent_id,
    ]
    .iter()
    .all(|id| bridge::valid_id(id))
}
fn revision(value: u64) -> bool {
    value > 0 && value <= i64::MAX as u64
}
fn hold_rank(value: BridgeChildCleanupHold) -> usize {
    use BridgeChildCleanupHold::*;
    match value {
        AgentBusy => 0,
        WorkspaceBusy => 1,
        PendingCommand => 2,
        UncertainCommand => 3,
        PendingReply => 4,
        UncertainReply => 5,
        OtherWorkspaceChildren => 6,
        InventoryLimit => 7,
        AuthorityUnavailable => 8,
        ParentUnavailable => 9,
        RuntimeChanged => 10,
        EvidenceUnavailable => 11,
        EvidenceInconsistent => 12,
    }
}
impl BridgeChildCleanupStatus {
    pub(super) fn check(
        &self,
        bridge_id: &str,
        allocation_id: &str,
        request: Option<&BridgeChildCleanupRequest>,
    ) -> Result<()> {
        if self.bridge_id != bridge_id
            || self.allocation_id != allocation_id
            || !prefixed(&self.allocation_id, "bral_")
            || !(1..=256).contains(&self.generation)
            || !revision(self.allocation_revision)
            || !target(&self.target)
            || !self.workspace_retained
            || !self.history_retained
            || self.agent_capacity_released
                != (self.progress == BridgeChildCleanupProgress::Complete)
            || (self.progress == BridgeChildCleanupProgress::NotRequested) != self.receipt.is_none()
            || (self.receipt.is_none() && (!self.holds.is_empty() || self.observed_at_ms.is_some()))
            || (self.receipt.is_some() && self.observed_at_ms.is_none())
            || self.observed_at_ms.is_some_and(|at| at > i64::MAX as u64)
            || (self.progress == BridgeChildCleanupProgress::Complete && !self.holds.is_empty())
            || self
                .holds
                .windows(2)
                .any(|pair| hold_rank(pair[0]) >= hold_rank(pair[1]))
        {
            return Err(Error::protocol(
                "child cleanup identity, target or progress is inconsistent",
            ));
        }
        if let Some(receipt) = &self.receipt {
            if !prefixed(&receipt.id, "bclj_")
                || receipt.bridge_id != self.bridge_id
                || receipt.allocation_id != self.allocation_id
                || receipt.generation != self.generation
                || receipt.target != self.target
                || receipt.accepted_at_ms > i64::MAX as u64
                || !revision(receipt.request.expected_bridge_revision)
                || receipt.request.expected_allocation_revision != self.allocation_revision
                || !matches!(&receipt.requested_by, ResourceActor::Human { subject } | ResourceActor::Api { subject, delegation_id: None } if bridge::valid_id(subject))
                || request.is_some_and(|request| request != &receipt.request)
            {
                return Err(Error::protocol(
                    "child cleanup receipt does not match its exact request and target",
                ));
            }
        } else if request.is_some() {
            return Err(Error::protocol(
                "child cleanup POST returned no acceptance receipt",
            ));
        }
        Ok(())
    }
}
impl BridgeCleanupAllocationPage {
    pub(super) fn check(&self, bridge_id: &str, options: &PageOptions) -> Result<()> {
        let limit = usize::from(options.limit.unwrap_or(50));
        if self.items.len() > limit
            || self.items.iter().enumerate().any(|(index, item)| {
                item.bridge_id != bridge_id
                    || !prefixed(&item.id, "bral_")
                    || !(1..=256).contains(&item.generation)
                    || !revision(item.revision)
                    || !target(&item.target)
                    || (index > 0 && self.items[index - 1].id >= item.id)
            })
            || self
                .next_cursor
                .as_deref()
                .is_some_and(|next| !cursor(next) || Some(next) == options.cursor.as_deref())
        {
            return Err(Error::protocol(
                "allocation page changed Bridge, ordering, limit or continuation",
            ));
        }
        Ok(())
    }
}
