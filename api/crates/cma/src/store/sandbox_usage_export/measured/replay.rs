use super::freeze::Manifest;
use super::*;
use layer2_core::{
    sandbox_metrics::{Binding, BindingKind, ProofStatus},
    sandbox_usage_evidence::{FrozenMeasuredMinute, MeasuredMinute, MeasuredQuality},
};

#[derive(Clone, Debug)]
pub(super) struct PreviousEvidence {
    fact: MeasuredMinute,
    bindings: Vec<Binding>,
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
        let current = frozen.ok_or_else(corrupt)?.fact();
        for (index, (old, new)) in previous
            .fact
            .metrics
            .iter()
            .zip(&current.metrics)
            .enumerate()
        {
            let old_coverage: u128 = old.covered_binding_ms.parse().map_err(|_| corrupt())?;
            let new_coverage: u128 = new.covered_binding_ms.parse().map_err(|_| corrupt())?;
            if new.quality != MeasuredQuality::Observed
                && (new_coverage < old_coverage
                    || (old.quantity.is_some() && new.quantity.is_none()))
                && !self.invalidates_bindings(previous, index)
            {
                return Ok(Some("refresh_evidence_incomplete".into()));
            }
        }
        Ok(None)
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
