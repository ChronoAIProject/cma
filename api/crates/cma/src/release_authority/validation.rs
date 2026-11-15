use serde::Serialize;
use sha2::{Digest, Sha256};

use super::*;

pub(super) fn digest(value: &str) -> Result<()> {
    if value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        Ok(())
    } else {
        Err(AuthorityError::Invalid)
    }
}

pub(super) fn identity(value: &str) -> Result<()> {
    if !value.is_empty() && value.len() <= 512 && value.bytes().all(|c| c.is_ascii_graphic()) {
        Ok(())
    } else {
        Err(AuthorityError::Invalid)
    }
}

pub(super) fn hash(value: &impl Serialize) -> Result<String> {
    let bytes = serde_json::to_vec(value).map_err(|_| AuthorityError::Invalid)?;
    if bytes.len() > 64 * 1024 {
        return Err(AuthorityError::Invalid);
    }
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub(super) fn key(deployment: &str, id: &str) -> Result<String> {
    identity(deployment)?;
    identity(id)?;
    hash(&("cma.release-authority.v1", deployment, id))
}

pub(super) fn increment(value: i64) -> Result<i64> {
    value.checked_add(1).ok_or(AuthorityError::Conflict)
}

impl ReleaseTarget {
    pub(crate) fn validate(&self) -> Result<()> {
        identity(&self.image)?;
        let (repository, image) = self
            .image
            .rsplit_once("@sha256:")
            .ok_or(AuthorityError::Invalid)?;
        if repository.is_empty() || repository.contains('@') || self.target_revision < 1 {
            return Err(AuthorityError::Invalid);
        }
        digest(image)?;
        digest(&self.plan_sha256)?;
        digest(&self.qualification_sha256)?;
        digest(
            self.target_id
                .strip_prefix("wp_")
                .ok_or(AuthorityError::Invalid)?,
        )?;
        for value in [
            &self.pool_name,
            &self.pool_uid,
            &self.template_name,
            &self.template_uid,
        ] {
            identity(value)?;
        }
        Ok(())
    }
}

impl ReleasePair {
    pub(crate) fn validate(&self) -> Result<()> {
        self.agent.validate()?;
        if let Some(machine) = &self.machine {
            machine.validate()?;
            if machine.image == self.agent.image
                || machine.target_id == self.agent.target_id
                || machine.pool_uid == self.agent.pool_uid
                || machine.template_uid == self.agent.template_uid
            {
                return Err(AuthorityError::Invalid);
            }
        }
        Ok(())
    }
}

impl WriterIdentity {
    pub(crate) fn validate(&self) -> Result<()> {
        identity(&self.deployment)?;
        identity(&self.instance_id)?;
        digest(&self.config_sha256)?;
        if self.epoch < 1 || self.fence < 1 {
            return Err(AuthorityError::Invalid);
        }
        Ok(())
    }
}

impl OperatorIdentity {
    pub(crate) fn validate(&self) -> Result<()> {
        identity(&self.subject)?;
        digest(&self.authorization_sha256)
    }
}

impl ReleaseHead {
    pub(crate) fn validate(&self) -> Result<()> {
        identity(&self.deployment)?;
        self.pair.validate()?;
        if self.schema_version != 1
            || self.epoch < 1
            || self.control_revision < 1
            || self.admission_sequence < 0
            || self.active_effects < 0
            || self.active_effects > self.admission_sequence
            || self.writer_fence < 1
            || self.protected_pairs.is_empty()
            || self.protected_pairs.len() > 2
            || !self.protected_pairs.contains(&self.pair)
            || (self.phase == Phase::Active && self.writer.is_none())
            || (self.phase == Phase::Active && self.operation_id.is_some())
            || (self.phase != Phase::Active && self.operation_id.is_none())
            || (matches!(self.phase, Phase::CommittedVerifying | Phase::FailedHold)
                && (self.writer.is_some() || self.active_effects != 0))
            || (matches!(self.phase, Phase::Frozen | Phase::Prepared) && self.active_effects != 0)
        {
            return Err(AuthorityError::Invalid);
        }
        for pair in &self.protected_pairs {
            pair.validate()?;
        }
        if let Some(id) = &self.operation_id {
            identity(id)?;
        }
        if let Some(writer) = &self.writer {
            writer.validate()?;
            if writer.deployment != self.deployment
                || writer.epoch != self.epoch
                || writer.fence != self.writer_fence
            {
                return Err(AuthorityError::Invalid);
            }
        }
        Ok(())
    }
}

impl OperationRequest {
    pub(super) fn identity(&self) -> (&str, &str) {
        match self {
            Self::Bootstrap(r) => (&r.deployment, &r.operation_id),
            Self::Transition(r) => (&r.deployment, &r.operation_id),
        }
    }

    pub(super) fn validate(&self) -> Result<()> {
        let (deployment, id) = self.identity();
        identity(deployment)?;
        identity(id)?;
        match self {
            Self::Bootstrap(r) => {
                r.operator.validate()?;
                r.pair.validate()?;
                r.fallback_pair.validate()?;
                r.writer.validate()?;
                if r.writer.deployment != r.deployment
                    || r.writer.epoch != 1
                    || r.writer.fence != 1
                    || r.pair == r.fallback_pair
                    || r.pair.machine.is_some() != r.fallback_pair.machine.is_some()
                {
                    return Err(AuthorityError::Invalid);
                }
            }
            Self::Transition(r) => {
                r.operator.validate()?;
                r.before.validate()?;
                r.after.validate()?;
                r.next_writer.validate()?;
                if r.expected_epoch < 1
                    || r.expected_control_revision < 1
                    || r.next_writer.deployment != r.deployment
                    || r.next_writer.epoch != increment(r.expected_epoch)?
                    || r.before == r.after
                    || r.before.machine.is_some() != r.after.machine.is_some()
                {
                    return Err(AuthorityError::Invalid);
                }
            }
        }
        Ok(())
    }
}

impl ReleaseOperation {
    pub(super) fn validate(&self) -> Result<()> {
        self.request.validate()?;
        if let Some(predecessor) = &self.predecessor_operation_id {
            identity(predecessor)?;
            if predecessor == &self.operation_id
                || matches!(self.request, OperationRequest::Bootstrap(_))
            {
                return Err(AuthorityError::Invalid);
            }
        }
        let (deployment, id) = self.request.identity();
        if self.schema_version != 1
            || self.deployment != deployment
            || self.operation_id != id
            || self.key != key(deployment, id)?
            || self.request_sha256 != hash(&self.request)?
            || self.receipts.is_empty()
            || self.receipts.len() > 32
        {
            return Err(AuthorityError::Invalid);
        }
        let mut previous = 0;
        for receipt in self
            .receipts
            .iter()
            .chain(self.terminal_recovery_receipt.iter())
        {
            digest(&receipt.request_sha256)?;
            digest(&receipt.evidence_sha256)?;
            if receipt.epoch < 1 || receipt.control_revision <= previous {
                return Err(AuthorityError::Invalid);
            }
            previous = receipt.control_revision;
        }
        if let Some(receipt) = &self.terminal_recovery_receipt {
            if self.receipts.len() != 32
                || !matches!(receipt.phase, Phase::Active | Phase::FailedHold)
            {
                return Err(AuthorityError::Invalid);
            }
        }
        Ok(())
    }
}

impl EffectIntent {
    pub(crate) fn validate(&self) -> Result<()> {
        identity(&self.effect_id)?;
        identity(&self.kind)?;
        digest(&self.resource_identity_sha256)?;
        digest(&self.request_sha256)
    }
}

impl ObservedOutcome {
    pub(crate) fn validate(&self) -> Result<()> {
        digest(&self.receipt_sha256)?;
        if let Some(incarnation) = &self.receiver_incarnation_sha256 {
            digest(incarnation)?;
        }
        Ok(())
    }
}

impl EffectPermit {
    pub(super) fn validate(&self) -> Result<()> {
        self.writer.validate()?;
        self.intent.validate()?;
        if self.schema_version != 1
            || self.key != key(&self.writer.deployment, &self.intent.effect_id)?
            || self.admitted_sequence < 1
            || self.revision < 1
            || self.observations.len() > 32
        {
            return Err(AuthorityError::Invalid);
        }
        for outcome in &self.observations {
            outcome.validate()?;
        }
        if let Some(evidence) = &self.terminal_evidence_sha256 {
            digest(evidence)?;
        }
        Ok(())
    }
}
