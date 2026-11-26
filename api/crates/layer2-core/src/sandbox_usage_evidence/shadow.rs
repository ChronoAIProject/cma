//! Evaluate one aligned interval of a shadow report without external effects.

use crate::sandbox_metrics::{
    measured_evidence::coverage::{ExactQuantity, MeasurementStatus},
    Metric,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Identifier(String);
impl Identifier {
    pub fn new(value: &str) -> Result<Self, Conflict> {
        if value.trim().is_empty() {
            return Err(Conflict);
        }
        Ok(Self(value.to_owned()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Interval {
    pub issuer: Identifier,
    pub owner: Identifier,
    pub sandbox: Identifier,
    /// Runtime or retained home identity.
    pub resource: Identifier,
    pub generation: Identifier,
    pub metric: Metric,
    pub from_ms: u64,
    pub to_ms: u64,
    pub window: Identifier,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Key {
    pub interval: Interval,
    pub cohort_digest: Identifier,
    pub revision: u64,
}

/// Remote cursor proofs attest this exact prefix after verification of the chain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Prefix {
    pub contract: Identifier,
    pub scope: Identifier,
    pub destination: Identifier,
    pub stream: Identifier,
    pub cutoff_ms: u64,
    pub sequence: u64,
    pub digest: Identifier,
}

#[derive(Debug, Serialize)]
pub struct Delivery {
    pub outbox: Prefix,
    pub receipt: Option<Prefix>,
    pub custody: Option<Prefix>,
    pub validated: Option<Prefix>,
    pub source_matches: bool,
}

/// A verified window proof whose digest binds reserve, settlement, release and remaining.
#[derive(Debug, Serialize)]
pub struct Funding {
    pub interval: Interval,
    pub contract: Identifier,
    pub rate_version: Identifier,
    pub digest: Identifier,
    pub upper_bound: ExactQuantity,
    pub balances_match: bool,
}

#[derive(Debug, Serialize)]
pub struct Input {
    pub key: Key,
    pub predecessor_digest: Option<Identifier>,
    pub cutoff_ms: u64,
    /// Quantities use core-seconds for CPU and byte-milliseconds for RAM/disk.
    pub allocation: ExactQuantity,
    pub measured: Option<ExactQuantity>,
    /// Measurement status is uniform across the interval.
    pub status: MeasurementStatus,
    pub measured_revision: Option<Identifier>,
    pub allocation_watermark: Option<Identifier>,
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
    /// Selects the effective revision at the cutoff with verified scope, units and lineage.
    fn read(&self, key: &Key) -> Result<Input, Conflict>;
}

pub trait SnapshotWriter {
    /// Same key and digest returns the original. Changed digest conflicts.
    fn append(&mut self, snapshot: Snapshot) -> Result<&Snapshot, Conflict>;
}

pub fn evaluate(input: &Input) -> Result<Snapshot, Conflict> {
    let key = &input.key;
    let interval = &key.interval;
    if interval.from_ms >= interval.to_ms
        || interval.to_ms > input.cutoff_ms
        || key.revision == 0
        || (key.revision == 1) != input.predecessor_digest.is_none()
    {
        return Err(Conflict);
    }
    let bytes = serde_json::to_vec(&("shadow-interval-v1", input)).map_err(|_| Conflict)?;
    Ok(Snapshot {
        key: key.clone(),
        digest: format!("{:x}", Sha256::digest(bytes)),
        decision: compare(input)?,
    })
}

fn compare(input: &Input) -> Result<Decision, Conflict> {
    use Decision::{Blocked, Fail, Matched};
    use MeasurementStatus::*;
    let cap = input.allocation.checked_ratio().ok_or(Conflict)?;
    let measured = input
        .measured
        .as_ref()
        .map(|quantity| quantity.checked_ratio().ok_or(Conflict))
        .transpose()?;
    let inactive = match input.key.interval.metric {
        Metric::Disk => NotAllocated,
        Metric::Cpu | Metric::Memory => NotRunning,
    };
    match input.status {
        Measured | Estimated if measured.is_some() => {}
        Pending | Unmapped if measured.is_none() => {}
        status if status == inactive && measured.is_none() && cap.numer().bits() == 0 => {}
        _ => return Err(Conflict),
    }
    if measured.as_ref().is_some_and(|quantity| quantity > &cap) {
        return Ok(Fail);
    }
    let Some(funding) = &input.funding else {
        return Ok(Blocked(MissingEvidence::Funding));
    };
    if funding.interval != input.key.interval {
        return Err(Conflict);
    }
    let upper = funding.upper_bound.checked_ratio().ok_or(Conflict)?;
    if !funding.balances_match || measured.is_some_and(|quantity| quantity > upper) {
        return Ok(Fail);
    }
    if matches!(input.status, Pending | Unmapped)
        || input.measured_revision.is_none()
        || input.allocation_watermark.is_none()
    {
        return Ok(Blocked(MissingEvidence::Measurement));
    }
    if let Some(delivery) = &input.delivery {
        let prefix = Some(&delivery.outbox);
        if delivery.source_matches
            && delivery.outbox.sequence > 0
            && (input.key.interval.to_ms..=input.cutoff_ms).contains(&delivery.outbox.cutoff_ms)
            && delivery.receipt.as_ref() == prefix
            && delivery.custody.as_ref() == prefix
            && delivery.validated.as_ref() == prefix
        {
            return Ok(Matched);
        }
    }
    Ok(Blocked(MissingEvidence::Delivery))
}

#[cfg(test)]
mod tests;
