//! Synthetic authority artifacts for the create-only qualification probe.
//! These values never grant access to a retained home or maintenance authority.
use layer3_adapter::runtime_ledger_collection::{
    CollectionGroup, CollectionObservation, CollectionRequest, LedgerCandidatePageV1,
    LedgerGroupAuthorityV1, LedgerGroupDecisionV1, LedgerGroupEvidenceV1, LedgerResourceRevisionV1,
    COLLECTION_FORMAT,
};
use layer3_adapter::runtime_ledger_maintenance::{
    LedgerBindingV1, LedgerSourceFormatV1, LEDGER_BINDING_FORMAT,
};
use sha2::{Digest, Sha256};

use super::invalid;
use crate::{config::RuntimeConfig, failure::Result};

pub(crate) fn artifacts(
    config: &RuntimeConfig,
    archive: &str,
    revision: u64,
    group: CollectionGroup,
) -> Result<(LedgerBindingV1, [Vec<u8>; 3])> {
    let binding = LedgerBindingV1 {
        format: LEDGER_BINDING_FORMAT.into(),
        sandbox_id: config.sandbox_id.clone(),
        home_identity: config.home_identity.clone().ok_or_else(invalid)?,
        generation: config.generation,
        boot_id: "b".repeat(32),
        roots_sha256: "c".repeat(64),
        source_format: LedgerSourceFormatV1::V2,
        archive_id: archive.into(),
        maintenance_id: "synthetic-held-probe".into(),
        maintenance_epoch: 1,
        sandbox_revision: 1,
        candidate_sha256: "d".repeat(64),
        recovery_sha256: "e".repeat(64),
        exclusion_receipt_sha256: "f".repeat(64),
    };
    let page = LedgerCandidatePageV1 {
        format: "runtime-ledger-candidates-v1".into(),
        binding: binding.clone(),
        active_revision: revision,
        cursor: None,
        next_cursor: None,
        groups: vec![group.clone()],
        complete: true,
    };
    let page_bytes = encode(&page)?;
    let request = CollectionRequest {
        format: COLLECTION_FORMAT.into(),
        operation_id: uuid::Uuid::new_v4().to_string(),
        binding: binding.clone(),
        active_revision: revision,
        page_sha256: hash(&page_bytes),
        groups: vec![group.clone()],
    };
    let evidence = LedgerGroupEvidenceV1 {
        format: "runtime-ledger-group-evidence-v1".into(),
        request_sha256: request.fingerprint().ok_or_else(invalid)?,
        binding_sha256: binding.binding_sha256().map_err(|_| invalid())?,
        group_key: group.key(),
        sandbox_revision: binding.sandbox_revision,
        resource_revisions: vec![LedgerResourceRevisionV1 {
            resource_sha256: "1".repeat(64),
            revision: 1,
        }],
        source_sha256: "2".repeat(64),
        observed_at: "2026-11-11T00:00:00Z".into(),
        decision: LedgerGroupDecisionV1::SettledDeleted,
    };
    let bytes = encode(&evidence)?;
    let observation = CollectionObservation {
        format: "runtime-ledger-authority-batch-v1".into(),
        request_sha256: evidence.request_sha256.clone(),
        groups: vec![LedgerGroupAuthorityV1 {
            format: "runtime-ledger-group-authority-v1".into(),
            binding: binding.clone(),
            group_key: evidence.group_key,
            sandbox_revision: evidence.sandbox_revision,
            resource_revisions: evidence.resource_revisions,
            observed_at: evidence.observed_at,
            observation_sha256: hash(&bytes),
            decision: evidence.decision,
            evidence_reference: format!("sha256:{}", hash(&bytes)),
        }],
        evidence: vec![bytes],
        dependencies_read: 2,
        source_bytes: 512,
    };
    Ok((
        binding,
        [page_bytes, encode(&request)?, encode(&observation)?],
    ))
}

fn encode(value: &impl serde::Serialize) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|_| invalid())?;
    bytes.extend_from_slice(b"\n ");
    Ok(bytes)
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
