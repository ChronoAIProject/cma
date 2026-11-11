//! Strict parent marker encoding and immutable transactional audit receipts.

use layer2_core::sandbox::ledger_maintenance_admission::LedgerMaintenanceStateV1;
use layer2_core::sandbox::store::ledger_maintenance::LedgerMaintenanceStoreError as Domain;
use layer2_core::Caller;
use mongodb::bson::{doc, spec::BinarySubtype, Binary, Bson, Document};
use sha2::{Digest, Sha256};

use super::sandbox_mongo::DOCUMENT_SCHEMA_VERSION;

pub(super) const FIELD: &str = "ledger_maintenance_v1";
pub(super) const AUDITS: &str = "ledger_maintenance_audits_v1";

pub(super) fn encode(state: &LedgerMaintenanceStateV1) -> Result<Bson, Domain> {
    state.accepted().map_err(|_| Domain::Corrupt)?;
    // Serialize metadata without expanding the bounded original artifact into
    // a BSON integer array. Only this private binary retains the accepted bytes.
    let mut metadata = state.clone();
    metadata.accepted_plan_bytes.clear();
    let mut document =
        mongodb::bson::serialize_to_document(&metadata).map_err(|_| Domain::Corrupt)?;
    document.insert(
        "accepted_plan_bytes",
        Bson::Binary(Binary {
            subtype: BinarySubtype::Generic,
            bytes: state.accepted_plan_bytes.clone(),
        }),
    );
    Ok(Bson::Document(document))
}

pub(super) fn read(parent: &Document) -> Result<Option<LedgerMaintenanceStateV1>, Domain> {
    let Some(value) = parent.get(FIELD) else {
        return Ok(None);
    };
    let mut document = value.as_document().cloned().ok_or(Domain::Corrupt)?;
    let Some(Bson::Binary(binary)) = document.remove("accepted_plan_bytes") else {
        return Err(Domain::Corrupt);
    };
    if binary.subtype != BinarySubtype::Generic {
        return Err(Domain::Corrupt);
    }
    document.insert("accepted_plan_bytes", Bson::Array(Vec::new()));
    let mut state: LedgerMaintenanceStateV1 =
        mongodb::bson::deserialize_from_document(document).map_err(|_| Domain::Corrupt)?;
    state.accepted_plan_bytes = binary.bytes;
    state.accepted().map_err(|_| Domain::Corrupt)?;
    Ok(Some(state))
}

pub(super) fn state_hash(state: &LedgerMaintenanceStateV1) -> Result<String, Domain> {
    let encoded = encode(state)?;
    let bytes =
        mongodb::bson::serialize_to_vec(&doc! {"state":encoded}).map_err(|_| Domain::Corrupt)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub(super) fn audit(
    previous: Option<&LedgerMaintenanceStateV1>,
    next: &LedgerMaintenanceStateV1,
    caller: &Caller,
) -> Result<Document, Domain> {
    let next_hash = state_hash(next)?;
    let previous_hash = previous.map(state_hash).transpose()?;
    Ok(doc! {
        "_id": &next_hash,
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "format": "ledger-maintenance-audit-v1",
        "binding_sha256": next.record.binding.binding_sha256().map_err(|_| Domain::Corrupt)?,
        "plan_sha256": &next.record.plan_sha256,
        "previous_sha256": previous_hash,
        "state_sha256": next_hash,
        "epoch": i64::try_from(next.record.binding.maintenance_epoch).map_err(|_| Domain::Corrupt)?,
        "revision": i64::try_from(next.record.record_revision).map_err(|_| Domain::Corrupt)?,
        // Private audit authority is retained, never emitted in summaries.
        "actor": caller.subject.as_str(),
        "authority": if caller.authorization.allows(layer2_core::Capability::ControlAnySandbox) {
            "operator"
        } else {
            "owner"
        },
        "recorded_at": &next.updated_at,
    })
}

pub(super) fn validate_audit(
    audit: &Document,
    state: &LedgerMaintenanceStateV1,
) -> Result<(), Domain> {
    let digest = state_hash(state)?;
    let prior = match audit.get("previous_sha256") {
        Some(Bson::Null) => state.record.record_revision == 1,
        Some(Bson::String(hash)) => {
            state.record.record_revision > 1
                && layer3_adapter::runtime_ledger_maintenance::valid_sha256(hash)
        }
        _ => false,
    };
    if audit.len() != 12
        || audit.get_str("_id").ok() != Some(digest.as_str())
        || audit.get_str("state_sha256").ok() != Some(digest.as_str())
        || audit.get_str("format").ok() != Some("ledger-maintenance-audit-v1")
        || audit.get_i64("document_schema_version").ok() != Some(DOCUMENT_SCHEMA_VERSION)
        || audit.get_str("binding_sha256").ok()
            != Some(
                state
                    .record
                    .binding
                    .binding_sha256()
                    .map_err(|_| Domain::Corrupt)?
                    .as_str(),
            )
        || audit.get_str("plan_sha256").ok() != Some(state.record.plan_sha256.as_str())
        || audit.get_i64("epoch").ok() != i64::try_from(state.record.binding.maintenance_epoch).ok()
        || audit.get_i64("revision").ok() != i64::try_from(state.record.record_revision).ok()
        || audit.get_str("recorded_at").ok() != Some(state.updated_at.as_str())
        || !audit
            .get_str("actor")
            .is_ok_and(|actor| !actor.trim().is_empty())
        || !matches!(audit.get_str("authority"), Ok("owner" | "operator"))
        || !prior
    {
        return Err(Domain::Corrupt);
    }
    Ok(())
}
