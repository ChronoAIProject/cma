//! Validated bounded collection evidence, not writer or maintenance authority.
//! Original artifacts stay byte-exact; holding them here is not persistence.
use layer3_adapter::runtime_ledger_collection::{
    CollectionObservation, CollectionRequest, LedgerCandidatePageV1, LedgerGroupDecisionV1,
    MAX_COLLECTION_OBSERVATION_BYTES, MAX_COLLECTION_REQUEST_BYTES,
};
use layer3_adapter::runtime_ledger_maintenance::{LedgerBindingV1, LedgerSourceFormatV1};
use sha2::{Digest, Sha256};

use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};
use crate::retirement_archive::GroupProof;

pub(crate) struct CollectionEvidence {
    binding: LedgerBindingV1,
    operation_id: String,
    active_revision: u64,
    group_key: String,
    observation_sha256: String,
    originals: [Vec<u8>; 3],
}

impl CollectionEvidence {
    /// The caller supplies the accepted binding independently of the artifacts.
    /// A matching binding still needs C's qualified exclusion before source use.
    pub(crate) fn parse(
        accepted: &LedgerBindingV1,
        group_key: &str,
        originals: [Vec<u8>; 3],
    ) -> Result<Self> {
        if originals[0].len() > 2 * 1024 * 1024
            || originals[1].len() > MAX_COLLECTION_REQUEST_BYTES
            || originals[2].len() > MAX_COLLECTION_OBSERVATION_BYTES
        {
            return Err(invalid());
        }
        let page: LedgerCandidatePageV1 =
            serde_json::from_slice(&originals[0]).map_err(|_| invalid())?;
        let request = CollectionRequest::decode(&originals[1]).ok_or_else(invalid)?;
        let observation: CollectionObservation =
            serde_json::from_slice(&originals[2]).map_err(|_| invalid())?;
        if !page.validate()
            || page.binding.validate_for(accepted).is_err()
            || request.binding.validate_for(accepted).is_err()
            || request.page_sha256 != hash(&originals[0])
            || request.active_revision != page.active_revision
            || request
                .groups
                .iter()
                .any(|group| !page.groups.contains(group))
            || !observation.validate_for(&request)
        {
            return Err(invalid());
        }
        let index = request
            .groups
            .iter()
            .position(|group| group.key() == group_key)
            .ok_or_else(invalid)?;
        let authority = &observation.groups[index];
        if authority.decision != LedgerGroupDecisionV1::SettledDeleted {
            return Err(RuntimeError::rejected(
                "ledger_collection_resource_retained",
                "bounded authority requires the selected resource to remain retained",
            ));
        }
        Ok(Self {
            binding: request.binding,
            operation_id: request.operation_id,
            active_revision: request.active_revision,
            group_key: group_key.into(),
            observation_sha256: authority.observation_sha256.clone(),
            originals,
        })
    }

    pub(crate) fn binding(&self) -> &LedgerBindingV1 {
        &self.binding
    }

    pub(crate) fn operation_id(&self) -> &str {
        &self.operation_id
    }

    pub(crate) fn observation_sha256(&self) -> &str {
        &self.observation_sha256
    }

    pub(crate) fn group_key(&self) -> &str {
        &self.group_key
    }

    pub(crate) fn active_revision(&self) -> u64 {
        self.active_revision
    }

    pub(crate) fn into_originals(self) -> [Vec<u8>; 3] {
        self.originals
    }

    /// These original page/request/batch bytes must be durably retained before
    /// Selected. Re-encoding a decoded value cannot replace any of these bytes.
    pub(crate) fn originals(&self) -> [&[u8]; 3] {
        self.originals.each_ref().map(Vec::as_slice)
    }

    /// Identifies this selected group's exact artifact bundle, not a permit,
    /// source digest, group-observation hash or persisted transition receipt.
    pub(crate) fn fingerprint(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"chrono.runtime.ledger.collection.artifacts.v1\0");
        for bytes in std::iter::once(self.group_key.as_bytes()).chain(self.originals()) {
            digest.update((bytes.len() as u64).to_be_bytes());
            digest.update(bytes);
        }
        format!("{:x}", digest.finalize())
    }

    pub(crate) fn validate_source(
        &self,
        config: &RuntimeConfig,
        proof: &GroupProof,
        source_revision: u64,
    ) -> Result<()> {
        if self.binding.source_format != LedgerSourceFormatV1::V2
            || self.binding.sandbox_id != config.sandbox_id
            || Some(&self.binding.home_identity) != config.home_identity.as_ref()
            || self.binding.generation != config.generation
            || self.active_revision != source_revision
            || self.group_key != proof.key
            || self.observation_sha256 != proof.observation_sha256
        {
            return Err(RuntimeError::rejected(
                "ledger_collection_source_changed",
                "bounded collection evidence does not match the selected source",
            ));
        }
        Ok(())
    }
}

impl std::fmt::Debug for CollectionEvidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CollectionEvidence")
            .field("artifacts_sha256", &self.fingerprint())
            .field("bytes", &self.originals.each_ref().map(Vec::len))
            .finish()
    }
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn invalid() -> RuntimeError {
    RuntimeError::rejected(
        "ledger_collection_evidence_invalid",
        "bounded collection evidence is invalid or differs from the accepted binding",
    )
}

#[cfg(test)]
#[path = "ledger_reclamation/tests.rs"]
mod tests;
