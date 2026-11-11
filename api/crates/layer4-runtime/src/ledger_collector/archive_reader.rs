//! Read A's retained originals and cold final receipt without mutation authority.
//! The caller supplies the original accepted binding, not a reconstructed permit.
use super::types::*;
use crate::ledger_reclamation::CollectionEvidence;
use crate::retirement_archive::{Archive, CollectionCompletion};
use layer3_adapter::runtime_ledger_maintenance::{LedgerBindingV1, LedgerSourceFormatV1};

pub(crate) async fn read_originals(
    archive: &Archive,
    accepted: &LedgerBindingV1,
    intent: &Intent,
    reference: &CollectionReference,
) -> Result<CollectionEvidence> {
    validate_input(accepted, intent, reference)?;
    let evidence = archive
        .read_collection(accepted.clone(), reference.clone())
        .await
        .map_err(|_| Refusal::UnknownOutcome)?;
    if evidence.operation_id() != reference.operation_id
        || evidence.group_key() != reference.group_key
        || evidence.active_revision() != intent.before_revision
        || evidence.observation_sha256() != hex(&intent.observation_sha256)
        || evidence.fingerprint() != hex(&intent.evidence_sha256)
        || evidence.originals().map(|bytes| bytes.len() as u64) != intent.artifact_lengths
    {
        return Err(Refusal::InvalidEvidence);
    }
    // This checks original artifacts only. Source membership, current exclusion
    // and the held effect window remain the executing port's separate duties.
    Ok(evidence)
}

pub(crate) async fn completion(
    archive: &Archive,
    accepted: &LedgerBindingV1,
    intent: &Intent,
    reference: &CollectionReference,
) -> Result<Option<Receipt>> {
    read_originals(archive, accepted, intent, reference).await?;
    // A verifies the retained encoding, checksum, proof and completed archive
    // group before returning this value. Never project an RPC submission reply
    // or deserialize a public receipt into B's acknowledged checkpoint.
    archive
        .collection_completion(accepted.clone(), reference.clone())
        .await
        .map_err(|_| Refusal::UnknownOutcome)?
        .map(|cold| project(cold, intent))
        .transpose()
}

fn validate_input(
    accepted: &LedgerBindingV1,
    intent: &Intent,
    reference: &CollectionReference,
) -> Result<()> {
    let binding = accepted
        .binding_sha256()
        .map_err(|_| Refusal::InvalidEvidence)?;
    if !intent.valid()
        || intent.decision != Decision::SettledDeleted
        || *reference != intent.reference()
        || accepted.archive_id != reference.archive_id
        || binding != hex(&intent.binding.sha256)
        || accepted.maintenance_epoch != intent.binding.epoch
        // The currently published A collection archive is V2-only. A V1
        // adapter must be supplied explicitly; never relabel V1 originals.
        || accepted.source_format != LedgerSourceFormatV1::V2
        || intent.binding.format != SourceFormat::V2
    {
        return Err(Refusal::InvalidEvidence);
    }
    Ok(())
}

fn project(cold: CollectionCompletion, intent: &Intent) -> Result<Receipt> {
    if cold.format != "runtime-ledger-transition-receipt-v1" || cold.phase != "acknowledged" {
        return Err(Refusal::InvalidEvidence);
    }
    let receipt = Receipt {
        version: 1,
        binding_sha256: digest(&cold.binding_sha256)?,
        operation: *uuid::Uuid::parse_str(&cold.operation_id)
            .map_err(|_| Refusal::InvalidEvidence)?
            .as_bytes(),
        plan_sha256: digest(&cold.plan_sha256)?,
        group: digest(&cold.group_key)?,
        source_sha256: digest(&cold.source_sha256)?,
        observation_sha256: digest(&cold.observation_sha256)?,
        before_sha256: digest(&cold.before_sha256)?,
        after_sha256: digest(&cold.after_sha256)?,
        before_revision: cold.before_revision,
        after_revision: cold.after_revision,
        operations: cold.operations,
        authority: cold.authority,
        cleanup: cold.cleanup,
        free_pages_before: cold.free_pages_before,
        free_pages_after: cold.free_pages_after,
        phase: Phase::Acknowledged,
        sha256: digest(&cold.receipt_sha256)?,
    };
    if !receipt.matches(intent) {
        return Err(Refusal::InvalidEvidence);
    }
    Ok(receipt)
}

fn digest(value: &str) -> Result<Digest> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Refusal::InvalidEvidence);
    }
    let mut result = [0; 32];
    for (index, byte) in result.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| Refusal::InvalidEvidence)?;
    }
    Ok(result)
}
