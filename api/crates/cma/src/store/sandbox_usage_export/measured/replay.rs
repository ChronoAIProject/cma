use super::freeze::Manifest;
use super::*;
use layer2_core::{
    sandbox_metrics::{Binding, BindingKind, ProofStatus, Range},
    sandbox_usage_evidence::{FrozenMeasuredMinute, MeasuredMinute, ObservedSupport},
};

#[derive(Clone, Debug)]
pub(super) struct PreviousEvidence {
    fact: MeasuredMinute,
    bindings: Vec<Binding>,
    support: Vec<(String, Binding)>,
    observations: Vec<ObservedSupport>,
}

impl CaptureStore {
    pub(super) async fn previous_measured_evidence(
        &self,
        stream: &MeasuredStream,
        minute: u64,
        head: &FactHead,
        session: &mut ClientSession,
    ) -> Result<PreviousEvidence, SandboxStoreError> {
        let row = self
            .collection(EVIDENCE)
            .find_one(doc! {"_id":format!("measured:{}:{}",stream.id,head.evidence_id)})
            .session(session)
            .await
            .map_err(unavailable)?
            .ok_or_else(corrupt)?;
        let bytes = row.get_binary_generic("manifest").map_err(|_| corrupt())?;
        if hash(bytes) != head.evidence_id {
            return Err(corrupt());
        }
        let retained: Manifest = serde_json::from_slice(bytes).map_err(|_| corrupt())?;
        let original = &retained.input;
        let frozen = retained.frozen.as_ref().ok_or_else(corrupt)?;
        if retained.version != 1
            || retained.hold.is_some()
            || original.minute != minute
            || frozen.fact().bucket_start != minute
            || original.stream.id != stream.id
            || original.stream.source_stream != stream.source_stream
            || original.stream.source_epoch != stream.source_epoch
            || original.stream.owner != stream.owner
            || original.stream.owner_issuer != stream.owner_issuer
            || original.stream.namespace != stream.namespace
            || original.stream.sandbox_id != stream.sandbox_id
            || original.resource != head.resource
            || hash(&serde_json::to_vec(&(frozen.fact(), &head.resource)).map_err(|_| corrupt())?)
                != head.input_digest
        {
            return Err(corrupt());
        }
        Ok(PreviousEvidence {
            fact: frozen.fact().clone(),
            bindings: original.bindings.clone(),
            support: original.binding_segments(),
            observations: frozen.observed_support().map_err(|_| corrupt())?,
        })
    }
}

impl MeasuredInput {
    pub(super) fn refresh_hold(
        &self,
        frozen: Option<&FrozenMeasuredMinute>,
        complete: bool,
    ) -> Result<Option<String>, SandboxStoreError> {
        if self.predecessor.is_none() {
            return Ok(None);
        }
        let previous = self.previous_evidence.as_ref().ok_or_else(corrupt)?;
        if !complete {
            return Ok(Some("refresh_source_unavailable".into()));
        }
        let frozen = frozen.ok_or_else(corrupt)?;
        let observations = frozen.observed_support().map_err(|_| corrupt())?;
        let current = frozen.fact();
        let current_support = self.binding_segments();
        for (index, (old, new)) in previous
            .fact
            .metrics
            .iter()
            .zip(&current.metrics)
            .enumerate()
        {
            let old_coverage: u128 = old.covered_binding_ms.parse().map_err(|_| corrupt())?;
            let new_coverage: u128 = new.covered_binding_ms.parse().map_err(|_| corrupt())?;
            if (new_coverage < old_coverage
                || (old.quantity.is_some()
                    && (new.quantity.is_none()
                        || !self.retains_support(previous, &current_support, index)
                        || !retains_samples(previous, &current_support, &observations, index))))
                && !self.invalidates_bindings(previous, index)
            {
                return Ok(Some("refresh_evidence_incomplete".into()));
            }
        }
        Ok(None)
    }

    fn retains_support(
        &self,
        previous: &PreviousEvidence,
        current: &[(String, Binding)],
        metric: usize,
    ) -> bool {
        let kind = if metric == 2 {
            BindingKind::Home
        } else {
            BindingKind::Compute
        };
        let minute = Range {
            from: self.minute,
            to: self.minute + 60,
        };
        previous
            .support
            .iter()
            .filter(|(_, binding)| {
                binding.kind == kind
                    && binding.proof_status == ProofStatus::Verified
                    && binding.sandbox_id == self.stream.sandbox_id
                    && binding.owner_subject == self.stream.owner
            })
            .all(|(id, old)| {
                let old_range = old.range(minute.to).intersect(minute);
                let mut intervals: Vec<_> = current
                    .iter()
                    .filter(|(new_id, new)| {
                        new_id == id
                            && new.revision >= old.revision
                            && new.proof_status == ProofStatus::Verified
                            && same_assignment(old, new)
                    })
                    .map(|(_, new)| new.range(minute.to).intersect(old_range))
                    .collect();
                intervals.sort_by_key(|range| range.from);
                let mut covered_to = old_range.from;
                for interval in intervals {
                    if interval.from > covered_to {
                        break;
                    }
                    covered_to = covered_to.max(interval.to);
                }
                covered_to >= old_range.to
            })
    }

    fn invalidates_bindings(&self, previous: &PreviousEvidence, metric: usize) -> bool {
        let kind = if metric == 2 {
            BindingKind::Home
        } else {
            BindingKind::Compute
        };
        let relevant: Vec<_> = previous
            .bindings
            .iter()
            .filter(|b| {
                b.kind == kind
                    && b.proof_status == ProofStatus::Verified
                    && b.sandbox_id == self.stream.sandbox_id
                    && b.owner_subject == self.stream.owner
            })
            .collect();
        // Absence is not revocation. Only a fenced, newer attribution decision
        // can withdraw the support supplied by the original binding records.
        !relevant.is_empty()
            && relevant.iter().all(|old| {
                self.bindings.iter().any(|new| {
                    new.id == old.id
                        && new.revision > old.revision
                        && new.physical_key() == old.physical_key()
                        && new.sandbox_id == old.sandbox_id
                        && new.owner_subject == old.owner_subject
                        && new.proof_status == ProofStatus::Unattributed
                })
            })
    }
}

fn retains_samples(
    previous: &PreviousEvidence,
    current: &[(String, Binding)],
    observations: &[ObservedSupport],
    metric: usize,
) -> bool {
    previous
        .observations
        .iter()
        .filter(|old| old.metric.index() == metric)
        .all(|old| {
            let Some((id, binding)) = previous
                .support
                .iter()
                .find(|(_, binding)| binding.id == old.binding_id)
            else {
                return false;
            };
            let mut intervals: Vec<_> = observations
                .iter()
                .filter(|new| {
                    new.metric == old.metric
                        && current.iter().any(|(new_id, new_binding)| {
                            new_binding.id == new.binding_id
                                && new_id == id
                                && new_binding.revision >= binding.revision
                                && same_assignment(binding, new_binding)
                        })
                })
                .flat_map(|new| new.intervals.iter().copied())
                .collect();
            intervals.sort_by(|a, b| a.0.total_cmp(&b.0));
            old.intervals.iter().all(|&(from, to)| {
                let mut covered_to = from;
                for &(start, end) in &intervals {
                    if start > covered_to {
                        break;
                    }
                    covered_to = covered_to.max(end);
                }
                covered_to >= to
            })
        })
}

fn same_assignment(old: &Binding, new: &Binding) -> bool {
    old.kind == new.kind
        && old.physical_key() == new.physical_key()
        && old.deployment == new.deployment
        && old.native_sandbox_uid == new.native_sandbox_uid
        && old.pod_name == new.pod_name
        && old.pod_uid == new.pod_uid
        && old.pvc_name == new.pvc_name
        && old.pvc_uid == new.pvc_uid
        && old.claim_uid == new.claim_uid
        && old.sandbox_id == new.sandbox_id
        && old.owner_subject == new.owner_subject
        && old.runtime_container_id == new.runtime_container_id
        && old.restart_count == new.restart_count
        && old.runtime_generation == new.runtime_generation
}
