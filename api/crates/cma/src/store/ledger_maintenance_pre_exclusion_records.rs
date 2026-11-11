//! Explicit parent format and original outcomes. Version 2 is scoped to this
//! Sandbox envelope only; it never changes the global schema-v1 constant.

use layer2_core::sandbox::ledger_maintenance_pre_exclusion::{
    LedgerPreExclusionArtifactV1 as Artifact, LedgerPreExclusionStateV1 as State,
};
use layer2_core::sandbox::store::ledger_maintenance::LedgerMaintenanceStoreError as Domain;
use mongodb::bson::{doc, spec::BinarySubtype, Binary, Bson, Document};
use sha2::{Digest, Sha256};

pub(super) const FIELD: &str = "ledger_pre_exclusion_v1";
pub(super) const AUDITS: &str = "ledger_pre_exclusion_audits_v1";
pub(super) const PARENT_VERSION: i64 = 2;

fn metadata(state: &State) -> Result<Document, Domain> {
    state.accepted().map_err(|_| Domain::Corrupt)?;
    let mut value = state.clone();
    value.accepted_admission_bytes.clear();
    mongodb::bson::serialize_to_document(&value).map_err(|_| Domain::Corrupt)
}

pub(super) fn encode(state: &State) -> Result<Bson, Domain> {
    let mut document = metadata(state)?;
    document.insert(
        "accepted_admission_bytes",
        Bson::Binary(Binary {
            subtype: BinarySubtype::Generic,
            bytes: state.accepted_admission_bytes.clone(),
        }),
    );
    Ok(Bson::Document(document))
}

pub(super) fn read(parent: &Document) -> Result<Option<State>, Domain> {
    let version = parent.get("document_schema_version").and_then(|v| match v {
        Bson::Int32(n) => Some(i64::from(*n)),
        Bson::Int64(n) => Some(*n),
        _ => None,
    });
    let Some(value) = parent.get(FIELD) else {
        return if version == Some(super::sandbox_mongo::DOCUMENT_SCHEMA_VERSION) {
            Ok(None)
        } else {
            Err(Domain::Corrupt)
        };
    };
    if version != Some(PARENT_VERSION)
        || parent.contains_key(super::ledger_maintenance_records::FIELD)
    {
        return Err(Domain::Corrupt);
    }
    let mut document = value.as_document().cloned().ok_or(Domain::Corrupt)?;
    let Some(Bson::Binary(binary)) = document.remove("accepted_admission_bytes") else {
        return Err(Domain::Corrupt);
    };
    if binary.subtype != BinarySubtype::Generic {
        return Err(Domain::Corrupt);
    }
    document.insert("accepted_admission_bytes", Bson::Array(Vec::new()));
    let mut state: State =
        mongodb::bson::deserialize_from_document(document).map_err(|_| Domain::Corrupt)?;
    state.accepted_admission_bytes = binary.bytes;
    let accepted = state.accepted().map_err(|_| Domain::Corrupt)?;
    if parent.get_str("_id").ok() != Some(accepted.plan().sandbox_id.as_str())
        || parent.get_str("owner").ok() != Some(accepted.plan().authorized_by.as_str())
    {
        return Err(Domain::Corrupt);
    }
    Ok(Some(state))
}

/// Explicit version dispatcher for A's reader hook. The v2 envelope must be
/// recognized and validated before its unchanged domain aggregate is decoded.
/// Stored bytes are never stripped, rewritten or made readable by old binaries.
pub(super) fn source_document(parent: &Document) -> Result<Document, Domain> {
    let state = read(parent)?;
    let mut source = parent.clone();
    if state.is_some() {
        source.remove(FIELD);
        source.insert(
            "document_schema_version",
            super::sandbox_mongo::DOCUMENT_SCHEMA_VERSION,
        );
    }
    Ok(source)
}

pub(super) fn state_hash(state: &State) -> Result<String, Domain> {
    let value = doc! {"state":encode(state)?};
    let bytes = mongodb::bson::serialize_to_vec(&value).map_err(|_| Domain::Corrupt)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

/// Same logical operation/revision always addresses the same audit slot, even
/// when a caller changes its accepted bytes. Length prefixes avoid ambiguity.
pub(super) fn audit_key(accepted: &Artifact, revision: u64) -> String {
    let mut hash = Sha256::new_with_prefix(b"chrono.ledger.pre-exclusion-result.v1\0");
    for value in [
        accepted.plan().sandbox_id.as_str(),
        &accepted.plan().operation_id,
    ] {
        hash.update((value.len() as u64).to_be_bytes());
        hash.update(value.as_bytes());
    }
    hash.update(revision.to_be_bytes());
    format!("{:x}", hash.finalize())
}

pub(super) fn audit(
    previous: Option<&State>,
    next: &State,
    caller: &layer2_core::Caller,
) -> Result<Document, Domain> {
    let accepted = next.accepted().map_err(|_| Domain::Corrupt)?;
    Ok(doc! {
        "_id":audit_key(&accepted, next.record_revision),
        "document_schema_version":super::sandbox_mongo::DOCUMENT_SCHEMA_VERSION,
        "format":"ledger-pre-exclusion-audit-v1",
        "admission_sha256":&next.admission_sha256,
        "previous_sha256":previous.map(state_hash).transpose()?,
        "state_sha256":state_hash(next)?,
        // Private authenticated audit fields; never emitted as operational evidence.
        "actor":caller.subject.as_str(),
        "authority":if caller.authorization.allows(layer2_core::Capability::ControlAnySandbox) {"operator"} else {"owner"},
        // Original outcome metadata is small. Artifact bytes remain immutable
        // in the held parent and are validated by digest on every reconstruction.
        "outcome":metadata(next)?,
    })
}

pub(super) fn outcome(audit: &Document, accepted: &Artifact) -> Result<State, Domain> {
    let metadata = audit
        .get_document("outcome")
        .map_err(|_| Domain::Corrupt)?
        .clone();
    let mut state: State =
        mongodb::bson::deserialize_from_document(metadata).map_err(|_| Domain::Corrupt)?;
    if !state.accepted_admission_bytes.is_empty() {
        return Err(Domain::Corrupt);
    }
    state.accepted_admission_bytes = accepted.original_bytes().to_vec();
    state.accepted().map_err(|_| Domain::Corrupt)?;
    let prior_valid = match audit.get("previous_sha256") {
        Some(Bson::Null) => state.record_revision == 1,
        Some(Bson::String(hash)) => {
            state.record_revision > 1
                && layer3_adapter::runtime_ledger_maintenance::valid_sha256(hash)
        }
        _ => false,
    };
    if audit.len() != 9
        || audit.get_str("_id").ok() != Some(audit_key(accepted, state.record_revision).as_str())
        || audit.get_str("format").ok() != Some("ledger-pre-exclusion-audit-v1")
        || audit.get_i64("document_schema_version").ok()
            != Some(super::sandbox_mongo::DOCUMENT_SCHEMA_VERSION)
        || audit.get_str("admission_sha256").ok() != Some(accepted.sha256())
        || audit.get_str("state_sha256").ok() != Some(state_hash(&state)?.as_str())
        || !audit
            .get_str("actor")
            .is_ok_and(|actor| !actor.trim().is_empty())
        || !matches!(audit.get_str("authority"), Ok("owner" | "operator"))
        || !prior_valid
    {
        return Err(Domain::Corrupt);
    }
    Ok(state)
}
