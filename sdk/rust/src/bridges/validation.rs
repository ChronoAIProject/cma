use super::*;
use crate::{
    Error, Result,
    resource_access::{ResourceActor, ResourceOwner},
};

pub(super) fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.:/".contains(&b))
}
pub(super) fn id(value: &str) -> Result<()> {
    valid_id(value)
        .then_some(())
        .ok_or_else(|| Error::validation("invalid bounded bridge identifier"))
}
pub(super) fn revision(value: u64) -> Result<()> {
    (value > 0 && value <= i64::MAX as u64)
        .then_some(())
        .ok_or_else(|| {
            Error::validation("bridge revision must be 1 through the signed 64-bit bound")
        })
}
pub(super) fn owner(value: &ResourceOwner) -> bool {
    match value {
        ResourceOwner::Person { subject } | ResourceOwner::Organization { subject } => {
            valid_id(subject)
        }
    }
}
fn name(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 200 && !value.chars().any(char::is_control)
}
fn list(values: &[String], limit: usize, opaque: bool) -> bool {
    !values.is_empty()
        && values.len() <= limit
        && values
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            == values.len()
        && values.iter().all(|v| {
            if opaque {
                !v.trim().is_empty() && v.len() <= 256 && !v.chars().any(char::is_control)
            } else {
                valid_id(v)
            }
        })
}
fn gate(value: &BridgeIdentityAdmission) -> bool {
    match value {
        BridgeIdentityAdmission::Allowlist { ids } => list(ids, 512, true),
        _ => true,
    }
}
impl BridgeConfiguration {
    pub(super) fn check(&self, fresh: bool) -> std::result::Result<(), &'static str> {
        if !name(&self.display_name)
            || !valid_id(&self.binding.bot_id)
            || !valid_id(&self.binding.key_id)
            || !list(&self.binding.route_ids, 64, false)
        {
            return Err("invalid bridge name or selected binding");
        }
        match &self.placement {
            BridgePlacement::WorkspacePerPartition {
                sandbox_id,
                workspace_profile,
                agent_profile,
                capacity,
                ..
            } => {
                if !valid_id(sandbox_id)
                    || [workspace_profile, agent_profile]
                        .iter()
                        .any(|p| p.revision == 0 || !valid_id(&p.id))
                    || !capacity.valid()
                {
                    return Err("invalid shared parent, profiles or reserved capacity");
                }
            }
            BridgePlacement::WorktreeAgentPerPartition {
                sandbox_id,
                workspace_id,
                agent_profile,
                capacity,
                ..
            } => {
                if !valid_id(sandbox_id)
                    || !valid_id(workspace_id)
                    || agent_profile.revision == 0
                    || !valid_id(&agent_profile.id)
                    || !capacity.valid()
                {
                    return Err("invalid shared parent, profiles or reserved capacity");
                }
            }
            BridgePlacement::ExistingAgent { target } => {
                if [
                    &target.sandbox_id,
                    &target.workspace_id,
                    &target.worktree_id,
                    &target.agent_id,
                ]
                .iter()
                .any(|s| !valid_id(s))
                {
                    return Err("invalid retained bridge target");
                }
            }
            BridgePlacement::NewSandbox {
                sandbox_profile,
                workspace_profile,
                agent_profile,
            } => {
                if [sandbox_profile, workspace_profile, agent_profile]
                    .iter()
                    .any(|p| p.revision == 0 || !valid_id(&p.id))
                {
                    return Err("invalid bridge profile revision pin");
                }
            }
        }
        if fresh
            && matches!(self.placement, BridgePlacement::ExistingAgent { .. })
                != (self.partition == BridgePartitionPolicy::FixedSharedAgent)
        {
            return Err("fixed Agent placement requires explicit shared context");
        }
        let admitted = match &self.admission {
            BridgeAdmission::All => true,
            BridgeAdmission::Allowlist { sender_ids } => list(sender_ids, 512, false),
            BridgeAdmission::Scoped { policy } => gate(&policy.senders) && gate(&policy.chats),
        };
        if !admitted {
            return Err("bridge admission requires bounded unique identities");
        }
        let l = &self.limits;
        if !(1..=100_000).contains(&l.max_partitions)
            || !(1..=l.max_partitions).contains(&l.max_sandboxes)
            || !(1..=100_000).contains(&l.max_queued_messages)
            || !(1..=100_000).contains(&l.max_messages_per_minute)
            || !(1..=1_048_576).contains(&l.max_payload_bytes)
            || !(1..=2_592_000).contains(&l.inbox_retention_seconds)
            || !(1..=l.inbox_retention_seconds).contains(&l.reply_secret_retention_seconds)
        {
            return Err("bridge limits exceed their bounds or are inconsistent");
        }
        if serde_json::to_vec(self)
            .map_err(|_| "invalid bridge configuration")?
            .len()
            > 16_384
        {
            return Err("bridge configuration exceeds its bounded size");
        }
        Ok(())
    }
}
fn valid_revisions(revision: u64, config_revision: u64) -> bool {
    revision > 0
        && revision <= i64::MAX as u64
        && config_revision > 0
        && config_revision <= 256
        && config_revision <= revision
}
impl Bridge {
    pub(super) fn check(&self) -> Result<()> {
        if !valid_id(&self.id)
            || !owner(&self.owner)
            || !name(&self.display_name)
            || !valid_id(&self.bot_id)
            || !valid_revisions(self.revision, self.config_revision)
            || self.created_at.is_empty()
            || self.updated_at.is_empty()
        {
            return Err(Error::protocol(
                "bridge projection has invalid identity or revision bounds",
            ));
        }
        Ok(())
    }
}
impl BridgeReceipt {
    pub(super) fn check(&self) -> Result<()> {
        let actor = match &self.actor {
            ResourceActor::Human { subject }
            | ResourceActor::Api {
                subject,
                delegation_id: None,
            } => valid_id(subject),
            _ => false,
        };
        if !valid_id(&self.bridge_id)
            || !owner(&self.owner)
            || !actor
            || !valid_revisions(self.revision, self.config_revision)
            || self.applied_at.is_empty()
            || !matches!(
                self.command.as_str(),
                "create" | "update" | "enable" | "disable" | "retire" | "emergency"
            )
        {
            return Err(Error::protocol(
                "bridge receipt has invalid identity, attribution or revisions",
            ));
        }
        Ok(())
    }
    pub(super) fn matches(&self, id: &str, expected: u64, command: &str) -> Result<()> {
        self.check()?;
        if self.bridge_id != id
            || expected.checked_add(1) != Some(self.revision)
            || self.command != command
        {
            return Err(Error::protocol(
                "bridge receipt does not match its requested command",
            ));
        }
        Ok(())
    }
}
impl BridgeCredentialStatus {
    pub(super) fn check(&self) -> Result<()> {
        if self.revision > i64::MAX as u64
            || self.present != (self.revision > 0)
            || (self.revoked && !self.present)
        {
            return Err(Error::protocol("bridge key-custody status is inconsistent"));
        }
        Ok(())
    }
}
impl BridgeValidation {
    pub(super) fn check(&self, id: &str, expected: u64) -> Result<()> {
        if self.bridge_id != id
            || self.revision != expected
            || !self.valid
            || !valid_revisions(self.revision, self.config_revision)
        {
            return Err(Error::protocol(
                "bridge validation does not match the requested revision",
            ));
        }
        Ok(())
    }
}

impl BridgeSharedCapacity {
    fn valid(&self) -> bool {
        (1..=15).contains(&self.keep_other_workspaces)
            && (1..=15).contains(&self.keep_other_agents_per_workspace)
    }
}
