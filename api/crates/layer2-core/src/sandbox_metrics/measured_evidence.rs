//! Immutable source observations for a later measured billing ledger.
use super::{Binding, BindingKind, ProofStatus, Range, SOURCE};
use layer3_adapter::sandbox_metrics::{RawPhysicalSeries, SourceMetric};
use serde::Serialize;
use std::collections::BTreeMap;

const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(test, derive(Debug))]
pub struct MeasuredObservation {
    schema_version: u32,
    source: &'static str,
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
        &self.physical_sample_id
    }

    pub fn evidence_sha256(&self) -> Result<String, EvidenceError> {
        digest(self)
    }

    /// Call after looking up the physical sample ID. An equivalent attribution
    /// is a replay even when a subsequent proof has a different revision.
    pub fn verify_replay(&self, incoming: &Self) -> Result<(), EvidenceError> {
        if self.physical_sample_id != incoming.physical_sample_id
            || self.source_value_be != incoming.source_value_be
            || self.binding.owner_subject != incoming.binding.owner_subject
            || self.binding.sandbox_id != incoming.binding.sandbox_id
            || (!self.metric.is_pvc()
                && (self.binding.runtime_generation != incoming.binding.runtime_generation
                    || self.binding.native_sandbox_uid != incoming.binding.native_sandbox_uid
                    || self.binding.claim_uid != incoming.binding.claim_uid))
        {
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
    let from_ms = proof
        .from
        .checked_mul(1000)
        .ok_or(EvidenceError::InvalidSample)?;
    let to_ms = proof
        .to
        .checked_mul(1000)
        .ok_or(EvidenceError::InvalidSample)?;
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
            let physical_sample_id = digest(&(
                SOURCE,
                &binding.cluster,
                &binding.namespace,
                metric,
                &physical,
                timestamp_ms,
            ))?;
            Ok(MeasuredObservation {
                schema_version: SCHEMA_VERSION,
                source: SOURCE,
                metric,
                timestamp_ms,
                source_value_be,
                physical_sample_id,
                mapping_revision,
                binding: binding.clone(),
            })
        })
        .collect()
}

#[derive(Serialize)]
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

fn digest(value: &impl Serialize) -> Result<String, EvidenceError> {
    crate::profiles::content_digest(value).map_err(|_| EvidenceError::InvalidSample)
}

#[cfg(test)]
mod tests;
