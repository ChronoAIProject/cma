//! Immutable, aggregate-only final outcome. It is an archive receipt, not a
//! maintenance permit or evidence that a home is released for serving.
use sha2::{Digest, Sha256};

use super::{invalid, CollectionReference, GroupProof};
use crate::mutation_ledger::retirement_inventory::DeactivationReceipt;
use crate::{failure::Result, ledger_reclamation::CollectionEvidence};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CollectionCompletion {
    pub(crate) format: String,
    pub(crate) binding_sha256: String,
    pub(crate) operation_id: String,
    pub(crate) plan_sha256: String,
    pub(crate) group_key: String,
    pub(crate) source_sha256: String,
    pub(crate) observation_sha256: String,
    pub(crate) before_sha256: String,
    pub(crate) after_sha256: String,
    pub(crate) before_revision: u64,
    pub(crate) after_revision: u64,
    pub(crate) operations: u64,
    pub(crate) authority: u64,
    pub(crate) cleanup: u64,
    pub(crate) free_pages_before: u64,
    pub(crate) free_pages_after: u64,
    pub(crate) phase: String,
    pub(crate) receipt_sha256: String,
}

impl CollectionCompletion {
    pub(super) fn new(
        reference: &CollectionReference,
        evidence: &CollectionEvidence,
        proof: &GroupProof,
        source: DeactivationReceipt,
    ) -> Result<Self> {
        let mut value = Self {
            format: "runtime-ledger-transition-receipt-v1".into(),
            binding_sha256: reference.binding_sha256.clone(),
            operation_id: reference.operation_id.clone(),
            plan_sha256: reference.plan_sha256.clone(),
            group_key: reference.group_key.clone(),
            source_sha256: source.source_sha256,
            observation_sha256: evidence.observation_sha256().into(),
            before_sha256: source.before_sha256,
            after_sha256: source.after_sha256,
            before_revision: source.before_revision,
            after_revision: source.after_revision,
            operations: source.operations,
            authority: source.authority,
            cleanup: source.cleanup,
            free_pages_before: source.free_pages_before,
            free_pages_after: source.free_pages_after,
            phase: "acknowledged".into(),
            receipt_sha256: String::new(),
        };
        value.receipt_sha256 = value.hash();
        value.validate(reference, evidence, proof)?;
        Ok(value)
    }

    pub(super) fn validate(
        &self,
        reference: &CollectionReference,
        evidence: &CollectionEvidence,
        proof: &GroupProof,
    ) -> Result<()> {
        super::collection_reference::validate(reference)?;
        let removed = self
            .operations
            .checked_add(self.authority)
            .and_then(|count| count.checked_add(self.cleanup))
            .ok_or_else(invalid)?;
        if self.format != "runtime-ledger-transition-receipt-v1"
            || self.phase != "acknowledged"
            || self.operation_id != reference.operation_id
            || self.binding_sha256 != reference.binding_sha256
            || self.plan_sha256 != reference.plan_sha256
            || self.group_key != reference.group_key
            || self.group_key != proof.key
            || self.source_sha256 != proof.source_sha256
            || self.observation_sha256 != proof.observation_sha256
            || self.observation_sha256 != evidence.observation_sha256()
            || self.before_revision != evidence.active_revision()
            || self.before_revision.checked_add(1) != Some(self.after_revision)
            || self.operations == 0
            || self.authority == 0
            || removed > u64::from(proof.records)
            || self.free_pages_after < self.free_pages_before
            || [
                &self.before_sha256,
                &self.after_sha256,
                &self.source_sha256,
                &self.observation_sha256,
                &self.receipt_sha256,
            ]
            .iter()
            .any(|value| !super::record::digest(value))
            || self.hash() != self.receipt_sha256
        {
            return Err(invalid());
        }
        Ok(())
    }

    fn hash(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(b"chrono.runtime.ledger.transition.receipt.v1\0");
        for value in [
            &self.format,
            &self.binding_sha256,
            &self.operation_id,
            &self.plan_sha256,
            &self.group_key,
            &self.source_sha256,
            &self.observation_sha256,
            &self.before_sha256,
            &self.after_sha256,
            &self.phase,
        ] {
            hash.update((value.len() as u64).to_be_bytes());
            hash.update(value.as_bytes());
        }
        for value in [
            self.before_revision,
            self.after_revision,
            self.operations,
            self.authority,
            self.cleanup,
            self.free_pages_before,
            self.free_pages_after,
        ] {
            hash.update(value.to_be_bytes());
        }
        format!("{:x}", hash.finalize())
    }
}
