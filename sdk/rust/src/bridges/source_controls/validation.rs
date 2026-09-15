use super::super::validation as bridge;
use super::*;
use crate::resource_access::ResourceActor;

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn prefixed(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(digest)
}
pub(super) fn id(value: &str) -> Result<()> {
    prefixed(value, "brinv_")
        .then_some(())
        .ok_or_else(|| Error::validation("invalid invoke grant ID"))
}
fn stamp(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}
impl BridgeSourceControlRequest {
    pub(super) fn check(&self) -> std::result::Result<(), &'static str> {
        if !(1..=i64::MAX as u64).contains(&self.expected_bridge_revision)
            || !(1..=2).contains(&self.expected_grant_revision)
            || self.expected_policy_revision > 255
            || (self.action == BridgeSourceControlAction::AllowSenderCancelAndSteer
                && self.expected_policy_revision == 255)
        {
            return Err("invalid source-control consent revision or reserved revocation slot");
        }
        Ok(())
    }
}
impl BridgeSourceControlPolicy {
    pub(super) fn check(&self, grant_id: &str) -> Result<()> {
        let target = &self.target;
        if self.grant_id != grant_id
            || !prefixed(&self.grant_id, "brinv_")
            || !prefixed(&self.id, "brscp_")
            || !bridge::owner(&self.owner)
            || !bridge::valid_id(&self.pin.bridge_id)
            || !(1..=256).contains(&self.pin.config_revision)
            || !(1..=256).contains(&self.revision)
            || (self.enabled && self.revision == 256)
            || !stamp(&self.updated_at)
            || !matches!(&self.updated_by, ResourceActor::Human { subject } | ResourceActor::Api { subject, delegation_id: None } if bridge::valid_id(subject))
            || ![
                &target.ids.sandbox_id,
                &target.ids.workspace_id,
                &target.ids.worktree_id,
                &target.ids.agent_id,
            ]
            .iter()
            .all(|id| bridge::valid_id(id))
            || ![
                &target.sandbox_created_at,
                &target.workspace_created_at,
                &target.agent_created_at,
            ]
            .iter()
            .all(|time| stamp(time))
            || !digest(&target.agent_creation_fingerprint)
            || !digest(&target.worktree_identity_digest)
        {
            return Err(Error::protocol(
                "source-control policy has inconsistent identity, lineage or consent",
            ));
        }
        Ok(())
    }
}
impl BridgeSourceControlReceipt {
    pub(super) fn check(&self, grant_id: &str, body: &BridgeSourceControlRequest) -> Result<()> {
        self.result.check(grant_id)?;
        self.request.check().map_err(Error::protocol)?;
        if !prefixed(&self.id, "brscc_")
            || &self.request != body
            || self.result.revision != self.request.expected_policy_revision + 1
            || self.result.enabled
                != (self.request.action == BridgeSourceControlAction::AllowSenderCancelAndSteer)
            || (self.result.enabled && self.request.expected_grant_revision != 1)
        {
            return Err(Error::protocol(
                "source-control receipt changed the original request or consent",
            ));
        }
        Ok(())
    }
}
