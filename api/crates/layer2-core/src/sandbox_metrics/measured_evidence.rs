//! Immutable source observations for a later measured billing ledger.
use super::{Binding, BindingKind, ProofStatus, Range, SOURCE};
use layer3_adapter::sandbox_metrics::{RawPhysicalSeries, SourceMetric};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub mod batch;
pub mod coverage;
pub mod interval;

const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(test, derive(Debug))]
#[serde(transparent)]
pub struct MeasuredObservation(ObservationV1);

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[serde(deny_unknown_fields)]
struct ObservationV1 {
    schema_version: u32,
    source: String,
    metric: SourceMetric,
    timestamp_ms: u64,
    /// The original IEEE 754 bytes preserve the source quantity until normalization.
    source_value_be: [u8; 8],
    physical_sample_id: String,
    mapping_revision: u64,
    binding: Binding,
}

impl MeasuredObservation {
    pub fn physical_sample_id(&self) -> &str {
        &self.0.physical_sample_id
    }

    pub fn evidence_sha256(&self) -> Result<String, EvidenceError> {
        digest(self)
    }

    pub fn attribution_id(&self) -> Result<String, EvidenceError> {
        digest(&self.0.attribution())
    }

    /// Call after looking up the physical sample ID. An equivalent attribution
    /// is a replay even when a subsequent proof has a different revision.
    pub fn verify_replay(&self, incoming: &Self) -> Result<(), EvidenceError> {
        self.0.verify_replay(&incoming.0)
    }
}

/// Canonical searchable keys independently derived from validated custody.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustodyProjection {
    pub physical_series_id: String,
    pub physical_sample_id: String,
    pub timestamp_ms: u64,
    pub owner_id: String,
    pub sandbox_id: String,
    pub evidence_sha256: String,
}

/// A validated durable observation can prove replay without becoming input
/// to the source-capture path.
pub struct RetainedObservation(ObservationV1);

impl RetainedObservation {
    pub fn decode(
        payload: &[u8],
        physical_sample_id: &str,
        evidence_sha256: &str,
    ) -> Result<Self, EvidenceError> {
        let observation: ObservationV1 =
            serde_json::from_slice(payload).map_err(|_| EvidenceError::InvalidSample)?;
        let physical = PhysicalIdentity::for_binding(
            &observation.binding,
            observation.metric,
            observation.mapping_revision,
        )?;
        let expected_id = physical.sample_id(
            &observation.binding,
            observation.metric,
            observation.timestamp_ms,
        )?;
        let value = f64::from_be_bytes(observation.source_value_be);
        let range = observation.binding.range(u64::MAX);
        if observation.schema_version != SCHEMA_VERSION
            || observation.source != SOURCE
            || observation.physical_sample_id != physical_sample_id
            || expected_id != physical_sample_id
            || digest(&observation)? != evidence_sha256
            || !(range.from..range.to).contains(&(observation.timestamp_ms / 1000))
            || !value.is_finite()
            || value < 0.0
        {
            return Err(EvidenceError::InvalidSample);
        }
        Ok(Self(observation))
    }

    pub fn custody_projection(&self) -> Result<CustodyProjection, EvidenceError> {
        Ok(CustodyProjection {
            physical_series_id: self.0.series_id()?,
            physical_sample_id: self.0.physical_sample_id.clone(),
            timestamp_ms: self.0.timestamp_ms,
            owner_id: self.0.binding.owner_subject.clone(),
            sandbox_id: self.0.binding.sandbox_id.clone(),
            evidence_sha256: digest(&self.0)?,
        })
    }

    pub fn timestamp_ms(&self) -> u64 {
        self.0.timestamp_ms
    }

    pub fn shares_physical_series(&self, other: &Self) -> Result<bool, EvidenceError> {
        self.0.shares_physical_series(&other.0)
    }

    pub fn verify_replay(&self, incoming: &MeasuredObservation) -> Result<(), EvidenceError> {
        self.0.verify_replay(&incoming.0)
    }

    pub fn verify_retained_replay(&self, incoming: &Self) -> Result<(), EvidenceError> {
        self.0.verify_replay(&incoming.0)
    }

    pub fn attribution_id(&self) -> Result<String, EvidenceError> {
        digest(&self.0.attribution())
    }
}

#[derive(PartialEq, Serialize)]
struct Attribution<'a> {
    physical_sample_id: &'a str,
    source_value_be: [u8; 8],
    owner_subject: &'a str,
    sandbox_id: &'a str,
    compute_lineage: Option<(Option<u64>, &'a str, &'a str)>,
}

impl ObservationV1 {
    fn series_id(&self) -> Result<String, EvidenceError> {
        let binding = &self.binding;
        let identity = PhysicalIdentity::for_binding(binding, self.metric, self.mapping_revision)?;
        digest(&(
            &self.source,
            &binding.cluster,
            &binding.namespace,
            self.metric,
            identity,
        ))
    }

    fn shares_physical_series(&self, other: &Self) -> Result<bool, EvidenceError> {
        let a = &self.binding;
        let b = &other.binding;
        Ok(self.source == other.source
            && self.metric == other.metric
            && a.cluster == b.cluster
            && a.namespace == b.namespace
            && PhysicalIdentity::for_binding(a, self.metric, self.mapping_revision)?
                == PhysicalIdentity::for_binding(b, other.metric, other.mapping_revision)?)
    }

    fn attribution(&self) -> Attribution<'_> {
        Attribution {
            physical_sample_id: &self.physical_sample_id,
            source_value_be: self.source_value_be,
            owner_subject: &self.binding.owner_subject,
            sandbox_id: &self.binding.sandbox_id,
            compute_lineage: (!self.metric.is_pvc()).then_some((
                self.binding.runtime_generation,
                self.binding.native_sandbox_uid.as_str(),
                self.binding.claim_uid.as_str(),
            )),
        }
    }

    fn verify_replay(&self, incoming: &Self) -> Result<(), EvidenceError> {
        if self.attribution() != incoming.attribution() {
            return Err(EvidenceError::ConflictingObservation);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EvidenceError {
    #[error("measured sample lacks verified physical attribution")]
    Attribution,
    #[error("measured source sample or range is invalid")]
    InvalidSample,
    #[error("physical sample has conflicting observations or attribution")]
    ConflictingObservation,
}

/// Capture one metric for one retained binding. Other bindings for the same
/// physical sample must converge through the stable ID and replay check.
pub fn capture_observations(
    binding: &Binding,
    mapping_revision: u64,
    metric: SourceMetric,
    series: &[RawPhysicalSeries],
    requested: Range,
) -> Result<Vec<MeasuredObservation>, EvidenceError> {
    let physical = PhysicalIdentity::for_binding(binding, metric, mapping_revision)?;
    if requested.from >= requested.to || requested.seconds() > 86_400 {
        return Err(EvidenceError::InvalidSample);
    }
    let proof = binding.range(requested.to).intersect(requested);
    let (from_ms, to_ms) = millisecond_bounds(proof)?;
    let mut values = BTreeMap::new();
    for source in series {
        if source.metric() != metric || !physical.matches(binding, source) {
            return Err(EvidenceError::Attribution);
        }
        for sample in source.samples() {
            if !(from_ms..to_ms).contains(&sample.timestamp_ms) {
                continue;
            }
            if !sample.value.is_finite() || sample.value < 0.0 {
                return Err(EvidenceError::InvalidSample);
            }
            let value = sample.value.to_be_bytes();
            if values
                .insert(sample.timestamp_ms, value)
                .is_some_and(|old| old != value)
            {
                return Err(EvidenceError::ConflictingObservation);
            }
        }
    }
    values
        .into_iter()
        .map(|(timestamp_ms, source_value_be)| {
            let physical_sample_id = physical.sample_id(binding, metric, timestamp_ms)?;
            Ok(MeasuredObservation(ObservationV1 {
                schema_version: SCHEMA_VERSION,
                source: SOURCE.into(),
                metric,
                timestamp_ms,
                source_value_be,
                physical_sample_id,
                mapping_revision,
                binding: binding.clone(),
            }))
        })
        .collect()
}

#[derive(PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum PhysicalIdentity<'a> {
    Compute {
        pod_uid: &'a str,
        container_id: &'a str,
    },
    Home {
        pvc_uid: &'a str,
    },
}

impl<'a> PhysicalIdentity<'a> {
    fn sample_id(
        &self,
        binding: &Binding,
        metric: SourceMetric,
        timestamp_ms: u64,
    ) -> Result<String, EvidenceError> {
        digest(&(
            SOURCE,
            &binding.cluster,
            &binding.namespace,
            metric,
            self,
            timestamp_ms,
        ))
    }

    fn for_binding(
        binding: &'a Binding,
        metric: SourceMetric,
        mapping_revision: u64,
    ) -> Result<Self, EvidenceError> {
        if mapping_revision == 0
            || binding.schema_version != 1
            || binding.revision == 0
            || binding.proof_version == 0
            || binding.proof_status != ProofStatus::Verified
            || [
                &binding.id,
                &binding.owner_subject,
                &binding.sandbox_id,
                &binding.cluster,
                &binding.namespace,
            ]
            .into_iter()
            .any(|value| value.is_empty())
        {
            return Err(EvidenceError::Attribution);
        }
        match (binding.kind, metric) {
            (BindingKind::Compute, SourceMetric::CpuCounter | SourceMetric::WorkingSet)
                if binding
                    .runtime_generation
                    .is_some_and(|generation| generation > 0) =>
            {
                let pod_uid = binding
                    .pod_uid
                    .as_deref()
                    .filter(|uid| !uid.is_empty())
                    .ok_or(EvidenceError::Attribution)?;
                let container_id = binding
                    .runtime_container_id
                    .as_deref()
                    .filter(|id| !id.is_empty())
                    .ok_or(EvidenceError::Attribution)?;
                Ok(Self::Compute {
                    pod_uid,
                    container_id,
                })
            }
            (BindingKind::Home, SourceMetric::PvcUsed | SourceMetric::PvcCapacity)
                if !binding.pvc_uid.is_empty() =>
            {
                Ok(Self::Home {
                    pvc_uid: &binding.pvc_uid,
                })
            }
            _ => Err(EvidenceError::Attribution),
        }
    }

    fn matches(&self, binding: &Binding, source: &RawPhysicalSeries) -> bool {
        let label = |name: &str| source.label(name);
        label("cluster") == Some(&binding.cluster)
            && label("namespace") == Some(&binding.namespace)
            && match self {
                Self::Compute {
                    pod_uid,
                    container_id,
                } => {
                    Some(source.name()) == binding.pod_name.as_deref()
                        && label("pod_uid") == Some(pod_uid)
                        && label("container_id") == Some(container_id)
                }
                Self::Home { pvc_uid } => {
                    source.name() == binding.pvc_name && label("pvc_uid") == Some(pvc_uid)
                }
            }
    }
}

fn millisecond_bounds(range: Range) -> Result<(u64, u64), EvidenceError> {
    let from = range
        .from
        .checked_mul(1000)
        .ok_or(EvidenceError::InvalidSample)?;
    let to = range
        .to
        .checked_mul(1000)
        .ok_or(EvidenceError::InvalidSample)?;
    Ok((from, to))
}

fn digest(value: &impl Serialize) -> Result<String, EvidenceError> {
    crate::profiles::content_digest(value).map_err(|_| EvidenceError::InvalidSample)
}

#[cfg(test)]
mod tests;
