use mongodb::bson::DateTime;
use serde::{Deserialize, Serialize};

use super::super::{validation, AuthorityError, ReleaseTarget, Result, WriterIdentity};
use crate::warm_pools::PoolTarget;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NativePollLane {
    Background,
    StartupAgent,
    StartupMachine,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CommandRecord {
    pub key: String,
    pub writer: WriterIdentity,
    pub lane: NativePollLane,
    pub sequence: i64,
    pub previous_receipt_sha256: Option<String>,
    pub selected_target: Option<ReleaseTarget>,
}

/// Prepared by the store. No arbitrary producer, query, update or transaction.
/// The caller saves this before execute; cancellation only permits reconcile.
#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub(crate) struct NativePollCommand(pub(super) CommandRecord);

impl NativePollCommand {
    pub(crate) fn key(&self) -> &str {
        &self.0.key
    }
    pub(crate) fn lane(&self) -> NativePollLane {
        self.0.lane
    }
    pub(crate) fn sequence(&self) -> i64 {
        self.0.sequence
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "target",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(crate) enum NativePollOutcome {
    Empty,
    Claimed(Box<PoolTarget>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReceiptRecord {
    #[serde(rename = "_id")]
    pub key: String,
    pub kind: String,
    pub schema_version: i64,
    pub command: CommandRecord,
    pub outcome: NativePollOutcome,
    pub admission_sequence: i64,
    pub control_revision: i64,
    pub receipt_sha256: String,
}

/// Terminality of one Mongo transaction only. Never a provider/finality proof.
#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub(crate) struct NativePollReceipt(pub(super) ReceiptRecord);

impl NativePollReceipt {
    pub(crate) fn key(&self) -> &str {
        &self.0.key
    }
    pub(crate) fn lane(&self) -> NativePollLane {
        self.0.command.lane
    }
    pub(crate) fn sequence(&self) -> i64 {
        self.0.command.sequence
    }
    pub(crate) fn sha256(&self) -> &str {
        &self.0.receipt_sha256
    }
    pub(crate) fn outcome(&self) -> &NativePollOutcome {
        &self.0.outcome
    }
}

/// Deliberately not deserializable. A historical positive is never Fresh.
#[derive(Debug, Clone)]
pub(crate) enum NativePollExecution {
    Fresh(NativePollReceipt),
    Replay(NativePollReceipt),
    Uncertain,
}

pub(super) fn cursor_key(writer: &WriterIdentity, lane: NativePollLane) -> Result<String> {
    validation::hash(&("native-target-poll-cursor-v1", writer, lane))
}

pub(super) fn command_key(
    writer: &WriterIdentity,
    lane: NativePollLane,
    seq: i64,
) -> Result<String> {
    validation::hash(&("native-target-poll-command-v1", writer, lane, seq))
}

pub(super) fn lease(command: &CommandRecord) -> String {
    format!("native-poll-{}", command.key)
}

impl CommandRecord {
    pub fn validate(&self) -> Result<()> {
        self.writer.validate()?;
        if self.sequence < 1
            || self.key != command_key(&self.writer, self.lane, self.sequence)?
            || (self.sequence == 1) != self.previous_receipt_sha256.is_none()
            || (self.lane == NativePollLane::Background) != self.selected_target.is_none()
        {
            return Err(AuthorityError::Invalid);
        }
        if let Some(hash) = &self.previous_receipt_sha256 {
            validation::digest(hash)?;
        }
        if let Some(target) = &self.selected_target {
            target.validate()?;
        }
        Ok(())
    }
}

impl ReceiptRecord {
    pub fn digest(&self) -> Result<String> {
        validation::hash(&(
            "native-target-poll-receipt-v1",
            &self.command,
            &self.outcome,
            self.admission_sequence,
            self.control_revision,
        ))
    }

    pub fn validate(&self) -> Result<()> {
        self.command.validate()?;
        if self.key != self.command.key
            || self.kind != "native_target_poll_receipt"
            || self.schema_version != 1
            || self.admission_sequence < 1
            || self.control_revision < 1
            || self.receipt_sha256 != self.digest()?
        {
            return Err(AuthorityError::Invalid);
        }
        if let NativePollOutcome::Claimed(target) = &self.outcome {
            validate_target(&self.command, target)?;
        }
        Ok(())
    }
}

pub(super) fn validate_target(command: &CommandRecord, target: &PoolTarget) -> Result<()> {
    validation::digest(
        target
            .id
            .strip_prefix("wp_")
            .ok_or(AuthorityError::Invalid)?,
    )?;
    validation::digest(&target.plan.config_sha256)?;
    validation::identity(&target.actor)?;
    validation::identity(&target.plan.namespace)?;
    if let Some(binding) = &target.binding {
        for value in [
            &binding.name,
            &binding.uid,
            &binding.template_name,
            &binding.template_uid,
        ] {
            validation::identity(value)?;
        }
    }
    if target.plan.deployment != command.writer.deployment
        || target.document_schema_version != if target.machine { 4 } else { 3 }
        || target.revision < 1
        || target.applied_revision < 0
        || target.applied_revision > target.revision
        || target.retired
        || (target.retiring && target.target_replicas != 0)
        || target.target_replicas > 1000
        || target.requested.cpu_millis != target.plan.spec.cpu_millis
        || target.requested.memory_mib != target.plan.spec.memory_mib
        || target.requested.target_replicas > 1000
        || target.plan.spec.validate().is_err()
        || target.lease.as_deref() != Some(lease(command).as_str())
        || target.lease_until <= DateTime::from_millis(0)
        || target.binding.as_ref().is_some_and(|binding| {
            binding.spec != target.plan.spec
                || binding.pod_policy != target.plan.pod_policy
                || binding.config_sha256 != target.plan.config_sha256
        })
    {
        return Err(AuthorityError::Invalid);
    }
    if let Some(selected) = &command.selected_target {
        let binding = target.binding.as_ref().ok_or(AuthorityError::Invalid)?;
        if target.retiring
            || target.machine != (command.lane == NativePollLane::StartupMachine)
            || selected.target_id != target.id
            || selected.target_revision != target.revision
            || selected.plan_sha256 != target.plan.config_sha256
            || selected.image != target.plan.spec.image.uri
            || selected.pool_name != binding.name
            || selected.pool_uid != binding.uid
            || selected.template_name != binding.template_name
            || selected.template_uid != binding.template_uid
        {
            return Err(AuthorityError::Invalid);
        }
    }
    Ok(())
}
