//! Evaluate one aligned interval of a shadow report without external effects.

use crate::sandbox_metrics::{
    measured_evidence::{batch::MetricCoverage, coverage::ExactQuantity},
    Metric,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Interval {
    pub issuer: String,
    pub owner: String,
    pub sandbox: String,
    /// Runtime identity for CPU/RAM and persistent home identity for disk.
    pub resource: String,
    pub generation: String,
    pub metric: Metric,
    pub from_ms: u64,
    pub to_ms: u64,
    pub window: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Key {
    pub interval: Interval,
    /// Digest of the complete cohort manifest and selector.
    pub cohort_digest: String,
    pub revision: u64,
}

/// A remote cursor proof projected onto the target prefix after chain
/// verification. A cursor that has merely advanced past it is insufficient.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Prefix {
    pub contract: String,
    pub scope: String,
    pub destination: String,
    pub stream: String,
    pub cutoff_ms: u64,
    pub sequence: u64,
    pub digest: String,
}

#[derive(Debug, Serialize)]
pub struct Delivery {
    pub outbox: Prefix,
    pub receipt: Option<Prefix>,
    pub custody: Option<Prefix>,
    pub validated: Option<Prefix>,
    pub source_matches: bool,
}

/// The read port supplies the window proof after checking the contract and
/// rate version. Its digest binds reserve, settlement, release and remaining.
#[derive(Debug, Serialize)]
pub struct Funding {
    pub interval: Interval,
    pub contract: String,
    pub rate_version: String,
    pub digest: String,
    pub upper_bound: ExactQuantity,
    pub balances_match: bool,
}

#[derive(Debug, Serialize)]
pub struct Input {
    pub key: Key,
    pub predecessor_digest: Option<String>,
    pub cutoff_ms: u64,
    /// Quantities use core-seconds for CPU and byte-milliseconds for RAM/disk.
    pub allocation: ExactQuantity,
    pub measured: Option<ExactQuantity>,
    pub coverage: MetricCoverage,
    pub measured_revision: String,
    pub allocation_watermark: String,
    pub delivery: Option<Delivery>,
    pub funding: Option<Funding>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissingEvidence {
    Measurement,
    Delivery,
    Funding,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("shadow evidence identity, interval, quantity or snapshot conflicts")]
pub struct Conflict;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Matched,
    Blocked(MissingEvidence),
    Fail,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub key: Key,
    pub digest: String,
    pub decision: Decision,
}

pub trait EvidenceReader {
    /// Returns the effective evidence revision as of the cutoff after verifying
    /// source scope, interval alignment, units and correction lineage.
    fn read(&self, key: &Key) -> Result<Input, Conflict>;
}

pub trait SnapshotWriter {
    /// Same key and digest returns the original. Changed digest conflicts.
    fn append(&mut self, snapshot: Snapshot) -> Result<&Snapshot, Conflict>;
}

pub fn evaluate(input: &Input) -> Result<Snapshot, Conflict> {
    let interval = &input.key.interval;
    if interval.from_ms >= interval.to_ms
        || interval.to_ms > input.cutoff_ms
        || input.key.revision == 0
        || (input.key.revision == 1) != input.predecessor_digest.is_none()
        || input.key.cohort_digest.is_empty()
    {
        return Err(Conflict);
    }

    let decision = compare(input)?;
    let bytes = serde_json::to_vec(&("shadow-interval-v1", input)).map_err(|_| Conflict)?;
    Ok(Snapshot {
        key: input.key.clone(),
        digest: format!("{:x}", Sha256::digest(bytes)),
        decision,
    })
}

fn compare(input: &Input) -> Result<Decision, Conflict> {
    use Decision::{Blocked, Fail, Matched};
    let cap = input.allocation.checked_ratio().ok_or(Conflict)?;
    let measured = input
        .measured
        .as_ref()
        .map(|quantity| quantity.checked_ratio().ok_or(Conflict))
        .transpose()?;
    let c = &input.coverage;
    let covered = u128::from(c.measured_ms) + u128::from(c.estimated_ms);
    let duration = u128::from(input.key.interval.to_ms - input.key.interval.from_ms);
    if covered > u128::from(c.expected_ms)
        || u128::from(c.expected_ms) > duration
        || (covered > 0) != measured.is_some()
        || covered
            + u128::from(c.pending_ms)
            + u128::from(c.unmapped_ms)
            + u128::from(c.not_running_ms)
            + u128::from(c.not_allocated_ms)
            != duration
    {
        return Err(Conflict);
    }
    if measured.as_ref().is_some_and(|quantity| quantity > &cap) {
        return Ok(Fail);
    }
    let Some(funding) = input.funding.as_ref().filter(|proof| {
        [&proof.contract, &proof.rate_version, &proof.digest]
            .into_iter()
            .all(|value| !value.is_empty())
    }) else {
        return Ok(Blocked(MissingEvidence::Funding));
    };
    if funding.interval != input.key.interval {
        return Err(Conflict);
    }
    let upper = funding.upper_bound.checked_ratio().ok_or(Conflict)?;
    if !funding.balances_match || measured.is_some_and(|quantity| quantity > upper) {
        return Ok(Fail);
    }
    if covered != u128::from(c.expected_ms)
        || c.pending_ms.max(c.unmapped_ms) > 0
        || input.measured_revision.is_empty()
        || input.allocation_watermark.is_empty()
    {
        return Ok(Blocked(MissingEvidence::Measurement));
    }
    match &input.delivery {
        Some(delivery)
            if delivery.source_matches
                && (input.key.interval.to_ms..=input.cutoff_ms)
                    .contains(&delivery.outbox.cutoff_ms)
                && [&delivery.receipt, &delivery.custody, &delivery.validated]
                    .into_iter()
                    .all(|proof| proof.as_ref() == Some(&delivery.outbox)) =>
        {
            Ok(Matched)
        }
        _ => Ok(Blocked(MissingEvidence::Delivery)),
    }
}

#[cfg(test)]
mod tests;
