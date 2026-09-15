use super::{
    super::{BridgeAgentTarget, validation},
    *,
};
use crate::{Error, Result, resource_access::ResourceActor};

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(super) fn valid_id(value: &str) -> bool {
    value.strip_prefix("brinv_").is_some_and(digest)
}
pub(super) fn id(value: &str) -> Result<()> {
    valid_id(value)
        .then_some(())
        .ok_or_else(|| Error::validation("invalid bridge invoke grant identifier"))
}
fn actor(value: &ResourceActor) -> bool {
    match value {
        ResourceActor::Human { subject }
        | ResourceActor::Api {
            subject,
            delegation_id: None,
        } => validation::valid_id(subject),
        _ => false,
    }
}
fn stamp(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}
pub(super) fn target(value: &BridgeAgentTarget) -> bool {
    [
        &value.sandbox_id,
        &value.workspace_id,
        &value.worktree_id,
        &value.agent_id,
    ]
    .iter()
    .all(|id| validation::valid_id(id))
}
impl BridgeInvokeGrant {
    pub(super) fn check(&self) -> Result<()> {
        let state = match &self.state {
            BridgeInvokeGrantState::Active => self.revision == 1,
            BridgeInvokeGrantState::Revoked { at, actor: manager } => {
                self.revision == 2 && stamp(at) && actor(manager)
            }
        };
        if !valid_id(&self.id)
            || !validation::owner(&self.owner)
            || !state
            || !validation::valid_id(&self.pin.bridge_id)
            || !(1..=256).contains(&self.pin.config_revision)
            || !actor(&self.issued_by)
            || !stamp(&self.issued_at)
            || !target(&self.target.ids)
            || ![
                &self.target.sandbox_created_at,
                &self.target.workspace_created_at,
                &self.target.agent_created_at,
            ]
            .iter()
            .all(|v| stamp(v))
            || !digest(&self.target.agent_creation_fingerprint)
            || !digest(&self.target.worktree_identity_digest)
        {
            return Err(Error::protocol(
                "bridge invoke grant has invalid identity, lineage, attribution or state",
            ));
        }
        Ok(())
    }
}
impl BridgeInvokeReceipt {
    pub(super) fn check(&self) -> Result<()> {
        self.result.check()?;
        let attributed = match &self.result.state {
            BridgeInvokeGrantState::Active => self.actor == self.result.issued_by,
            BridgeInvokeGrantState::Revoked { actor, .. } => &self.actor == actor,
        };
        if !digest(&self.id)
            || !digest(&self.request_digest)
            || !actor(&self.actor)
            || !attributed
            || self.bridge_id != self.result.pin.bridge_id
        {
            return Err(Error::protocol(
                "bridge invoke receipt does not match its result or actual actor",
            ));
        }
        Ok(())
    }
}
impl BridgeInvokeReadiness {
    pub(super) fn check(&self, bridge_id: &str) -> Result<()> {
        if self.bridge_id != bridge_id
            || self.revision == 0
            || self.revision > i64::MAX as u64
            || !(1..=256).contains(&self.config_revision)
            || self.config_revision > self.revision
            || (self.status == BridgeInvokeConsentStatus::ConsentPresent)
                != self.candidate.is_some()
        {
            return Err(Error::protocol(
                "bridge invoke readiness is inconsistent with its requested bridge",
            ));
        }
        if let Some(grant) = &self.candidate {
            grant.check()?;
            if grant.pin.bridge_id != self.bridge_id
                || grant.pin.config_revision != self.config_revision
                || grant.state != BridgeInvokeGrantState::Active
            {
                return Err(Error::protocol(
                    "readiness candidate is not active consent for the current configuration",
                ));
            }
        }
        Ok(())
    }
}
