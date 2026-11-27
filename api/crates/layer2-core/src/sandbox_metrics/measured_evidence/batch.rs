//! Frozen evidence identity and append-only correction lineage for measured use.
use super::{
    coverage::{normalize_coverage, CoverageInterval, MeasurementStatus, COVERAGE_POLICY_VERSION},
    digest,
    interval::CustodySample,
};
use crate::sandbox_usage::UsageEvent;
use layer3_adapter::sandbox_metrics::SourceMetric;
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;
const MAX_WINDOW_MS: u64 = 15 * 60 * 1000;
const MAX_INTERVALS: usize = 12_288;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceScope {
    pub owner_id: String,
    pub sandbox_id: String,
    pub window_id: String,
    pub from_ms: u64,
    pub to_ms: u64,
    pub rate_card_id: String,
    pub rate_card_version: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorrectionReference {
    pub evidence_id: String,
    pub evidence_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllocationWatermark {
    pub first_revision: u64,
    pub last_revision: u64,
    pub last_source_seq: u64,
    pub history_sha256: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricCoverage {
    pub expected_ms: u64,
    pub measured_ms: u64,
    pub estimated_ms: u64,
    pub pending_ms: u64,
    pub unmapped_ms: u64,
    pub not_running_ms: u64,
    pub not_allocated_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricEvidence {
    pub metric_code: String,
    pub quantity_unit: String,
    pub coverage: MetricCoverage,
    pub intervals: Vec<CoverageInterval>,
    pub interval_sha256: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PayloadV1 {
    measured_evidence_schema_version: u32,
    metric_definition_version: u32,
    coverage_policy_version: u32,
    scope: EvidenceScope,
    source_cutoff_ms: u64,
    allocation_watermark: Option<AllocationWatermark>,
    revision: u64,
    correction_of: Option<CorrectionReference>,
    metrics: Vec<MetricEvidence>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenEvidence {
    evidence_id: String,
    evidence_sha256: String,
    payload: PayloadV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BatchError {
    #[error("measured batch scope, cutoff, or evidence is invalid")]
    Invalid,
    #[error("measured correction conflicts with the preceding evidence")]
    Conflict,
}

impl EvidenceScope {
    fn validate(&self) -> Result<(), BatchError> {
        if [
            &self.owner_id,
            &self.sandbox_id,
            &self.window_id,
            &self.rate_card_id,
        ]
        .into_iter()
        .any(|value| value.is_empty() || value.len() > 256)
            || self.rate_card_version == 0
            || !self.from_ms.is_multiple_of(1000)
            || !self.to_ms.is_multiple_of(1000)
            || self.from_ms >= self.to_ms
            || self.to_ms - self.from_ms > MAX_WINDOW_MS
        {
            return Err(BatchError::Invalid);
        }
        Ok(())
    }

    pub fn stream_id(&self) -> Result<String, BatchError> {
        self.validate()?;
        // The time bounds define the stream. The original batch pins the
        // provider window label and rate card.
        digest(&(
            "measured-evidence-stream-v1",
            &self.owner_id,
            &self.sandbox_id,
            self.from_ms,
            self.to_ms,
        ))
        .map_err(|_| BatchError::Invalid)
    }
}

impl FrozenEvidence {
    pub fn freeze(
        scope: EvidenceScope,
        cutoff_ms: u64,
        samples: &[CustodySample<'_>],
        allocation: &[UsageEvent],
        previous: Option<&Self>,
    ) -> Result<Self, BatchError> {
        scope.validate()?;
        let intervals = normalize_coverage(
            &scope.owner_id,
            &scope.sandbox_id,
            samples,
            allocation,
            scope.from_ms,
            scope.to_ms,
            cutoff_ms,
        )
        .map_err(|_| BatchError::Invalid)?;
        if intervals.len() > MAX_INTERVALS {
            return Err(BatchError::Invalid);
        }
        let (revision, correction_of) = if let Some(previous) = previous {
            previous.validate()?;
            if previous.payload.scope != scope || previous.payload.source_cutoff_ms > cutoff_ms {
                return Err(BatchError::Conflict);
            }
            (
                previous
                    .revision()
                    .checked_add(1)
                    .ok_or(BatchError::Conflict)?,
                Some(previous.reference()),
            )
        } else {
            (1, None)
        };
        let payload = PayloadV1 {
            measured_evidence_schema_version: SCHEMA_VERSION,
            metric_definition_version: 1,
            coverage_policy_version: COVERAGE_POLICY_VERSION,
            scope,
            source_cutoff_ms: cutoff_ms,
            allocation_watermark: watermark(allocation)?,
            revision,
            correction_of,
            metrics: metric_evidence(intervals)?,
        };
        if previous.is_some_and(|prior| {
            prior.payload.metrics == payload.metrics
                && prior.payload.allocation_watermark == payload.allocation_watermark
        }) {
            return Err(BatchError::Conflict);
        }
        let evidence_id = evidence_id(&payload)?;
        let evidence_sha256 = digest(&(&evidence_id, &payload)).map_err(|_| BatchError::Invalid)?;
        let result = Self {
            evidence_id,
            evidence_sha256,
            payload,
        };
        result.validate()?;
        Ok(result)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, BatchError> {
        let value: Self = serde_json::from_slice(bytes).map_err(|_| BatchError::Invalid)?;
        value.validate()?;
        if serde_json::to_vec(&value).map_err(|_| BatchError::Invalid)? != bytes {
            return Err(BatchError::Invalid);
        }
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), BatchError> {
        let payload = &self.payload;
        payload.scope.validate()?;
        if payload.measured_evidence_schema_version != SCHEMA_VERSION
            || payload.metric_definition_version != 1
            || payload.coverage_policy_version != COVERAGE_POLICY_VERSION
            || payload.source_cutoff_ms < payload.scope.to_ms
            || payload.revision == 0
            || (payload.revision == 1) != payload.correction_of.is_none()
            || self.evidence_id != evidence_id(payload)?
            || self.evidence_sha256
                != digest(&(&self.evidence_id, payload)).map_err(|_| BatchError::Invalid)?
        {
            return Err(BatchError::Invalid);
        }
        if payload.metrics.len() != 3
            || payload
                .metrics
                .iter()
                .map(|m| m.intervals.len())
                .sum::<usize>()
                > MAX_INTERVALS
        {
            return Err(BatchError::Invalid);
        }
        for (evidence, metric) in payload.metrics.iter().zip([
            SourceMetric::CpuCounter,
            SourceMetric::WorkingSet,
            SourceMetric::PvcUsed,
        ]) {
            if evidence.metric_code != metric_code(metric)
                || evidence.quantity_unit != unit(metric)
                || evidence.intervals.first().map(|row| row.from_ms) != Some(payload.scope.from_ms)
                || evidence.intervals.last().map(|row| row.to_ms) != Some(payload.scope.to_ms)
                || evidence
                    .intervals
                    .iter()
                    .any(|row| row.metric != metric || !valid_interval(row))
                || evidence
                    .intervals
                    .windows(2)
                    .any(|pair| pair[0].to_ms != pair[1].from_ms)
                || evidence.interval_sha256.len() != evidence.intervals.len()
                || evidence
                    .intervals
                    .iter()
                    .zip(&evidence.interval_sha256)
                    .any(|(row, expected)| digest(row).ok().as_ref() != Some(expected))
                || evidence.coverage != coverage(&evidence.intervals)
            {
                return Err(BatchError::Invalid);
            }
        }
        Ok(())
    }

    pub fn validate_successor(&self, prior: &Self) -> Result<(), BatchError> {
        self.validate()?;
        prior.validate()?;
        if self.payload.scope != prior.payload.scope
            || prior.revision().checked_add(1) != Some(self.revision())
            || self.payload.source_cutoff_ms < prior.payload.source_cutoff_ms
            || self.payload.correction_of.as_ref() != Some(&prior.reference())
        {
            return Err(BatchError::Conflict);
        }
        Ok(())
    }

    pub fn matches_request(
        &self,
        scope: &EvidenceScope,
        cutoff_ms: u64,
        correction: Option<&CorrectionReference>,
    ) -> bool {
        &self.payload.scope == scope
            && self.payload.source_cutoff_ms == cutoff_ms
            && self.payload.correction_of.as_ref() == correction
    }

    pub fn scope(&self) -> &EvidenceScope {
        &self.payload.scope
    }
    pub fn source_cutoff_ms(&self) -> u64 {
        self.payload.source_cutoff_ms
    }
    pub fn allocation_watermark(&self) -> Option<&AllocationWatermark> {
        self.payload.allocation_watermark.as_ref()
    }
    pub fn correction_of(&self) -> Option<&CorrectionReference> {
        self.payload.correction_of.as_ref()
    }
    pub fn metrics(&self) -> &[MetricEvidence] {
        &self.payload.metrics
    }
    pub fn revision(&self) -> u64 {
        self.payload.revision
    }
    pub fn reference(&self) -> CorrectionReference {
        CorrectionReference {
            evidence_id: self.evidence_id.clone(),
            evidence_sha256: self.evidence_sha256.clone(),
        }
    }
    pub fn evidence_id(&self) -> &str {
        &self.evidence_id
    }
    pub fn evidence_sha256(&self) -> &str {
        &self.evidence_sha256
    }
}

fn evidence_id(payload: &PayloadV1) -> Result<String, BatchError> {
    digest(&(
        "measured-evidence-v1",
        payload.scope.stream_id()?,
        payload.revision,
    ))
    .map_err(|_| BatchError::Invalid)
}

fn watermark(events: &[UsageEvent]) -> Result<Option<AllocationWatermark>, BatchError> {
    let mut ordered: Vec<_> = events.iter().collect();
    ordered.sort_by_key(|event| event.revision);
    let (Some(first), Some(last)) = (ordered.first(), ordered.last()) else {
        return Ok(None);
    };
    Ok(Some(AllocationWatermark {
        first_revision: first.revision,
        last_revision: last.revision,
        last_source_seq: last.source_seq,
        history_sha256: digest(&ordered).map_err(|_| BatchError::Invalid)?,
    }))
}

fn metric_evidence(intervals: Vec<CoverageInterval>) -> Result<Vec<MetricEvidence>, BatchError> {
    [
        SourceMetric::CpuCounter,
        SourceMetric::WorkingSet,
        SourceMetric::PvcUsed,
    ]
    .into_iter()
    .map(|metric| {
        let rows: Vec<_> = intervals
            .iter()
            .filter(|row| row.metric == metric)
            .cloned()
            .collect();
        let interval_sha256 = rows
            .iter()
            .map(digest)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| BatchError::Invalid)?;
        Ok(MetricEvidence {
            metric_code: metric_code(metric).to_owned(),
            quantity_unit: unit(metric).to_owned(),
            coverage: coverage(&rows),
            intervals: rows,
            interval_sha256,
        })
    })
    .collect()
}
fn unit(metric: SourceMetric) -> &'static str {
    if metric == SourceMetric::CpuCounter {
        "core_seconds"
    } else {
        "byte_milliseconds"
    }
}

fn valid_interval(row: &CoverageInterval) -> bool {
    if row.from_ms >= row.to_ms {
        return false;
    }
    let duration = row.to_ms - row.from_ms;
    let hash = |value: &str| {
        value.len() == 64
            && value
                .bytes()
                .all(|byte| matches!(byte,b'0'..=b'9'|b'a'..=b'f'))
    };
    let allocation = match (
        &row.allocation_revision,
        &row.allocation_sha256,
        &row.allocated_quantity,
    ) {
        (Some(revision), Some(digest), Some(quantity)) if *revision > 0 && hash(digest) => {
            quantity.checked_ratio()
        }
        (None, None, None) if row.measurement_status == MeasurementStatus::Unmapped => None,
        _ => return false,
    };
    if row.allocation_revision.is_some() != allocation.is_some()
        || row.expected_ms > duration
        || row
            .physical_series_sha256
            .as_ref()
            .is_some_and(|id| !hash(id))
        || row.source_references.iter().any(|reference| {
            !hash(&reference.evidence_sha256) || !hash(&reference.physical_sample_id)
        })
    {
        return false;
    }
    let Some(quantity) = row.quantity.as_ref() else {
        return row.derivation.is_none()
            && match row.measurement_status {
                MeasurementStatus::Pending => allocation.is_some() && row.expected_ms == duration,
                MeasurementStatus::Unmapped => {
                    row.expected_ms == if allocation.is_some() { duration } else { 0 }
                }
                MeasurementStatus::NotRunning => {
                    !row.metric.is_pvc() && allocation.is_some() && row.expected_ms == 0
                }
                MeasurementStatus::NotAllocated => {
                    row.metric.is_pvc() && allocation.is_some() && row.expected_ms == 0
                }
                _ => false,
            };
    };
    let Some(quantity) = quantity.checked_ratio() else {
        return false;
    };
    if row.expected_ms != duration
        || row.physical_series_sha256.is_none()
        || row.source_references.len() < 2
        || row.derivation.is_none()
    {
        return false;
    }
    match row.measurement_status {
        MeasurementStatus::Measured => allocation.is_some(),
        MeasurementStatus::Estimated => allocation.is_some_and(|cap| quantity <= cap),
        _ => false,
    }
}

fn metric_code(metric: SourceMetric) -> &'static str {
    match metric {
        SourceMetric::CpuCounter => "cma.sandbox.cpu.runtime_core_seconds",
        SourceMetric::WorkingSet => "cma.sandbox.ram.working_set_gib_seconds",
        SourceMetric::PvcUsed => "cma.sandbox.disk.pvc_used_gib_seconds",
        SourceMetric::PvcCapacity => "cma.sandbox.disk.pvc_capacity_gib_seconds",
    }
}
fn coverage(rows: &[CoverageInterval]) -> MetricCoverage {
    let mut result = MetricCoverage::default();
    for row in rows {
        result.expected_ms += row.expected_ms;
        let duration = row.to_ms - row.from_ms;
        let total = match row.measurement_status {
            MeasurementStatus::Measured => &mut result.measured_ms,
            MeasurementStatus::Estimated => &mut result.estimated_ms,
            MeasurementStatus::Pending => &mut result.pending_ms,
            MeasurementStatus::Unmapped => &mut result.unmapped_ms,
            MeasurementStatus::NotRunning => &mut result.not_running_ms,
            MeasurementStatus::NotAllocated => &mut result.not_allocated_ms,
        };
        *total += duration;
    }
    result
}
