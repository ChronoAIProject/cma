//! Accounting for authenticated, pre-bound approval operations in one turn.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{identifier, TurnTimeoutError};
mod events;

pub const DEFAULT_APPROVAL_BUDGET_MS: i64 = 300_000;
const MAX_OPERATIONS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalLimits {
    pub episode_ms: i64,
    pub total_ms: i64,
}

impl Default for ApprovalLimits {
    fn default() -> Self {
        Self {
            episode_ms: DEFAULT_APPROVAL_BUDGET_MS,
            total_ms: DEFAULT_APPROVAL_BUDGET_MS,
        }
    }
}

/// Authenticated clock capability, frozen before eligible operations dispatch.
/// The interval maps a provider timestamp into possible service timestamps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalClockBounds {
    pub offset_min_ms: i64,
    pub offset_max_ms: i64,
    pub valid_until_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalOperationBinding {
    pub operation_id: String,
    pub model_attempt_id: String,
    pub provider: String,
    pub authority_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingApprovalReceipt {
    pub binding: ApprovalOperationBinding,
    pub review_id: String,
    pub revision: u64,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalResolution {
    Approved,
    Denied,
    Expired,
    Disconnected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedApprovalReceipt {
    pub binding: ApprovalOperationBinding,
    pub revision: u64,
    pub resolved_at_ms: i64,
    pub resolution: ApprovalResolution,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Operation {
    binding: ApprovalOperationBinding,
    registered_at_ms: i64,
    revision: u64,
    receipt_sha256: Option<String>,
    review_id: Option<String>,
    started_at_ms: Option<i64>,
    expires_at_ms: Option<i64>,
    ended_at_ms: Option<i64>,
    resolution: Option<ApprovalResolution>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalBudget {
    limits: ApprovalLimits,
    clock: ApprovalClockBounds,
    admitted_at_ms: i64,
    observed_at_ms: i64,
    operations: Vec<Operation>,
}

impl ApprovalBudget {
    pub fn admit(
        limits: ApprovalLimits,
        clock: ApprovalClockBounds,
        now_ms: i64,
    ) -> Result<Self, TurnTimeoutError> {
        let value = Self {
            limits,
            clock,
            admitted_at_ms: now_ms,
            observed_at_ms: now_ms,
            operations: Vec::new(),
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), TurnTimeoutError> {
        if self.admitted_at_ms < 0
            || self.observed_at_ms < self.admitted_at_ms
            || self.limits.episode_ms <= 0
            || self.limits.total_ms <= 0
            || self.limits.episode_ms > DEFAULT_APPROVAL_BUDGET_MS
            || self.limits.total_ms > DEFAULT_APPROVAL_BUDGET_MS
            || self.clock.valid_until_ms < self.admitted_at_ms
            || self
                .clock
                .offset_max_ms
                .checked_sub(self.clock.offset_min_ms)
                .is_none_or(|width| !(0..=4_000).contains(&width))
            || self.operations.len() > MAX_OPERATIONS
        {
            return Err(TurnTimeoutError::Invalid);
        }
        for (index, operation) in self.operations.iter().enumerate() {
            validate_binding(&operation.binding)?;
            if operation.registered_at_ms < self.admitted_at_ms
                || operation.registered_at_ms > self.observed_at_ms
                || operation.revision > (1_u64 << 53) - 1
                || self.operations[..index]
                    .iter()
                    .any(|old| old.binding.operation_id == operation.binding.operation_id)
                || (operation.started_at_ms.is_some() != operation.expires_at_ms.is_some())
                || (operation.started_at_ms.is_some() != operation.review_id.is_some())
                || (operation.started_at_ms.is_some() != operation.receipt_sha256.is_some())
                || (operation.resolution.is_some() != operation.ended_at_ms.is_some())
                || (operation.started_at_ms.is_none() && operation.resolution.is_some())
                || operation.ended_at_ms.is_some_and(|end| end < 0)
                || (operation.started_at_ms.is_none() && operation.revision != 0)
                || (operation.started_at_ms.is_some() && operation.revision == 0)
                || operation.started_at_ms.is_some_and(|start| {
                    start < operation.registered_at_ms || start > self.observed_at_ms
                })
                || operation
                    .expires_at_ms
                    .zip(operation.started_at_ms)
                    .is_some_and(|(end, start)| end < start)
                || operation
                    .ended_at_ms
                    .zip(operation.started_at_ms)
                    .is_some_and(|(end, start)| end < start || end > self.observed_at_ms)
                || operation
                    .review_id
                    .as_ref()
                    .is_some_and(|id| !identifier(id, 256))
                || operation
                    .receipt_sha256
                    .as_ref()
                    .is_some_and(|value| !super::hex(value, 64))
                || (operation.resolution == Some(ApprovalResolution::Approved)
                    && operation
                        .ended_at_ms
                        .zip(operation.expires_at_ms)
                        .is_some_and(|(end, expiry)| end > expiry))
            {
                return Err(TurnTimeoutError::Invalid);
            }
        }
        Ok(())
    }

    /// Reserve bounded custody before forwarding. A full budget cannot evict an
    /// old operation to make room for another remote effect.
    pub fn register(
        &self,
        binding: ApprovalOperationBinding,
        now_ms: i64,
    ) -> Result<Self, TurnTimeoutError> {
        self.validate()?;
        validate_binding(&binding)?;
        if let Some(old) = self
            .operations
            .iter()
            .find(|old| old.binding.operation_id == binding.operation_id)
        {
            return if old.binding == binding {
                Ok(self.with_observed_time(now_ms))
            } else {
                Err(TurnTimeoutError::Conflict)
            };
        }
        let now_ms = now_ms.max(self.observed_at_ms);
        if now_ms > self.clock.valid_until_ms
            || self.operations.len() == MAX_OPERATIONS
            || self
                .deadline_at_ms()
                .is_some_and(|deadline| deadline <= now_ms)
        {
            return Err(TurnTimeoutError::Conflict);
        }
        let mut next = self.clone();
        next.observed_at_ms = now_ms;
        next.operations.push(Operation {
            binding,
            registered_at_ms: now_ms,
            revision: 0,
            receipt_sha256: None,
            review_id: None,
            started_at_ms: None,
            expires_at_ms: None,
            ended_at_ms: None,
            resolution: None,
        });
        next.validate()?;
        Ok(next)
    }

    pub fn pending(
        &self,
        receipt: &PendingApprovalReceipt,
        now_ms: i64,
    ) -> Result<Self, TurnTimeoutError> {
        self.validate()?;
        let index = self.operation(&receipt.binding)?;
        let old = &self.operations[index];
        let fingerprint = fingerprint(receipt)?;
        if receipt.revision == old.revision && old.receipt_sha256.as_ref() == Some(&fingerprint) {
            return Ok(self.with_observed_time(now_ms));
        }
        if receipt.revision == 0
            || receipt.revision <= old.revision
            || old.resolution.is_some()
            || !identifier(&receipt.review_id, 256)
            || receipt.expires_at_ms < receipt.created_at_ms
        {
            return Err(TurnTimeoutError::Conflict);
        }
        let now_ms = now_ms.max(self.observed_at_ms);
        self.require_clock(now_ms)?;
        let start = receipt
            .created_at_ms
            .checked_add(self.clock.offset_min_ms)
            .ok_or(TurnTimeoutError::Invalid)?;
        let latest_start = receipt
            .created_at_ms
            .checked_add(self.clock.offset_max_ms)
            .ok_or(TurnTimeoutError::Invalid)?;
        let expiry = receipt
            .expires_at_ms
            .checked_add(self.clock.offset_min_ms)
            .ok_or(TurnTimeoutError::Invalid)?;
        if start > now_ms || latest_start < old.registered_at_ms || expiry < old.registered_at_ms {
            return Err(TurnTimeoutError::Conflict);
        }
        let mut next = self.clone();
        next.observed_at_ms = now_ms;
        let operation = &mut next.operations[index];
        operation.revision = receipt.revision;
        operation.receipt_sha256 = Some(fingerprint);
        operation.review_id = Some(receipt.review_id.clone());
        // A replacement review can shorten the episode, never restart it.
        operation.started_at_ms = Some(
            old.started_at_ms
                .map_or(start, |previous| previous.min(start))
                .max(old.registered_at_ms),
        );
        operation.expires_at_ms = Some(
            old.expires_at_ms
                .map_or(expiry, |previous| previous.min(expiry)),
        );
        next.validate()?;
        Ok(next)
    }

    pub fn resolve(
        &self,
        receipt: &ResolvedApprovalReceipt,
        now_ms: i64,
    ) -> Result<Self, TurnTimeoutError> {
        self.validate()?;
        let index = self.operation(&receipt.binding)?;
        let old = &self.operations[index];
        let fingerprint = fingerprint(receipt)?;
        if receipt.revision == old.revision && old.receipt_sha256.as_ref() == Some(&fingerprint) {
            return Ok(self.with_observed_time(now_ms));
        }
        if receipt.revision <= old.revision || old.resolution.is_some() {
            return Err(TurnTimeoutError::Conflict);
        }
        let start = old.started_at_ms.ok_or(TurnTimeoutError::Conflict)?;
        let now_ms = now_ms.max(self.observed_at_ms);
        self.require_clock(now_ms)?;
        let earliest_end = receipt
            .resolved_at_ms
            .checked_add(self.clock.offset_min_ms)
            .ok_or(TurnTimeoutError::Invalid)?;
        let end = receipt
            .resolved_at_ms
            .checked_add(self.clock.offset_max_ms)
            .ok_or(TurnTimeoutError::Invalid)?
            .min(now_ms);
        if earliest_end > now_ms || end < start {
            return Err(TurnTimeoutError::Conflict);
        }
        if receipt.resolution == ApprovalResolution::Approved && end > old.expires_at_ms.unwrap() {
            return Err(TurnTimeoutError::Conflict);
        }
        let mut next = self.clone();
        next.observed_at_ms = now_ms;
        let operation = &mut next.operations[index];
        operation.revision = receipt.revision;
        operation.receipt_sha256 = Some(fingerprint);
        operation.ended_at_ms = Some(end);
        operation.resolution = Some(receipt.resolution);
        next.validate()?;
        Ok(next)
    }

    pub fn is_waiting(&self) -> bool {
        self.operations
            .iter()
            .any(|operation| operation.started_at_ms.is_some() && operation.resolution.is_none())
    }

    pub(super) fn has_operations(&self) -> bool {
        !self.operations.is_empty()
    }

    pub fn waiting_since_ms(&self) -> Option<i64> {
        self.operations
            .iter()
            .filter(|operation| operation.resolution.is_none())
            .filter_map(|operation| operation.started_at_ms)
            .min()
    }

    pub fn deadline_at_ms(&self) -> Option<i64> {
        let episodes = self
            .operations
            .iter()
            .filter_map(|operation| {
                let start = operation.started_at_ms?;
                let deadline = start
                    .saturating_add(self.limits.episode_ms)
                    .min(operation.expires_at_ms.unwrap());
                (operation.ended_at_ms.is_none_or(|end| end > deadline)).then_some(deadline)
            })
            .min();
        episodes
            .into_iter()
            .chain(self.budget_exhaustion_at())
            .min()
    }

    pub fn consumed_ms(&self, now_ms: i64) -> i64 {
        let now_ms = now_ms.max(self.observed_at_ms);
        self.intervals()
            .into_iter()
            .map(|(start, end)| end.min(now_ms).saturating_sub(start).max(0))
            .fold(0, i64::saturating_add)
    }

    fn budget_exhaustion_at(&self) -> Option<i64> {
        let mut remaining = self.limits.total_ms;
        for (start, end) in self.intervals() {
            let duration = end.saturating_sub(start);
            if duration >= remaining {
                return Some(start.saturating_add(remaining));
            }
            remaining -= duration;
        }
        None
    }

    fn intervals(&self) -> Vec<(i64, i64)> {
        let mut intervals: Vec<_> = self
            .operations
            .iter()
            .filter_map(|operation| {
                operation
                    .started_at_ms
                    .map(|start| (start, operation.ended_at_ms.unwrap_or(i64::MAX)))
            })
            .collect();
        intervals.sort_unstable();
        let mut union: Vec<(i64, i64)> = Vec::with_capacity(intervals.len());
        for (start, end) in intervals {
            if let Some(previous) = union.last_mut().filter(|previous| start <= previous.1) {
                previous.1 = previous.1.max(end);
            } else {
                union.push((start, end));
            }
        }
        union
    }

    fn operation(&self, binding: &ApprovalOperationBinding) -> Result<usize, TurnTimeoutError> {
        self.operations
            .iter()
            .position(|operation| &operation.binding == binding)
            .ok_or(TurnTimeoutError::Conflict)
    }

    fn require_clock(&self, now_ms: i64) -> Result<(), TurnTimeoutError> {
        if now_ms > self.clock.valid_until_ms {
            return Err(TurnTimeoutError::Conflict);
        }
        Ok(())
    }

    fn with_observed_time(&self, now_ms: i64) -> Self {
        let mut next = self.clone();
        next.observed_at_ms = now_ms.max(self.observed_at_ms);
        next
    }
}

fn validate_binding(binding: &ApprovalOperationBinding) -> Result<(), TurnTimeoutError> {
    if !identifier(&binding.operation_id, 256)
        || !identifier(&binding.model_attempt_id, 256)
        || !identifier(&binding.provider, 64)
        || !super::hex(&binding.authority_digest, 64)
    {
        return Err(TurnTimeoutError::Invalid);
    }
    Ok(())
}

fn fingerprint(value: &impl Serialize) -> Result<String, TurnTimeoutError> {
    serde_json::to_vec(value)
        .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
        .map_err(|_| TurnTimeoutError::Invalid)
}

#[cfg(test)]
mod tests;
