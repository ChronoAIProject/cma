//! Retained ordinary-work envelope: old version-one readers and CAS writers fail closed.

use layer2_core::sandbox::store::WorkAuthorityProtocol;
use mongodb::bson::{doc, Bson, Document};
use serde::de::DeserializeOwned;

pub(super) mod floor;

pub(super) const RENEWED_VERSION: i64 = 3;

pub(super) fn version(protocol: Option<WorkAuthorityProtocol>) -> i64 {
    if protocol.is_some() {
        RENEWED_VERSION
    } else {
        1
    }
}

pub(super) fn versions() -> Document {
    doc! {"$in":[1_i64, RENEWED_VERSION]}
}

pub(super) fn require(document: &Document) -> Result<(), String> {
    match document.get("document_schema_version") {
        Some(Bson::Int64(1 | RENEWED_VERSION)) | Some(Bson::Int32(1 | 3)) => Ok(()),
        _ => Err("unsupported ordinary-work envelope".into()),
    }
}

pub(super) fn read<T: DeserializeOwned>(document: &Document) -> Result<T, String> {
    require(document)?;
    let record = document
        .get_document("record")
        .map_err(|_| "missing work record")?;
    let protocol = record
        .get_document("work")
        .map_err(|_| "missing work state")?
        .get("authority_protocol");
    let promoted = matches!(
        document.get("document_schema_version"),
        Some(Bson::Int64(3) | Bson::Int32(3))
    );
    if promoted != matches!(protocol, Some(Bson::String(value)) if value == "renewed_v1")
        || (!promoted && protocol.is_some())
    {
        return Err("ordinary-work writer floor disagrees with envelope".into());
    }
    mongodb::bson::deserialize_from_document(record.clone()).map_err(|error| error.to_string())
}

/// Only Sandbox/Workspace queries call this helper. Other aggregate schemas,
/// especially Agent's independently versioned envelope, keep their own contract.
/// Preserve unknown versions in a predicate so unsupported inputs still refuse.
pub(super) fn query(mut filter: Document) -> Document {
    if matches!(
        filter.get("document_schema_version"),
        Some(Bson::Int64(1) | Bson::Int32(1))
    ) {
        filter.insert("document_schema_version", versions());
    }
    for key in ["$and", "$or", "$nor"] {
        if let Some(Bson::Array(children)) = filter.get_mut(key) {
            for child in children {
                if let Bson::Document(document) = child {
                    *document = query(std::mem::take(document));
                }
            }
        }
    }
    filter
}

impl super::sandbox_mongo::MongoSandboxStore {
    pub(crate) fn with_renewable_work(mut self, enabled: bool) -> Self {
        self.renewable_work = enabled;
        self
    }
}

/// Generic lineage helpers can share a collection parameter without widening
/// the schema accepted for Agents, worktrees, or other persisted records.
pub(super) fn for_collection(rows: &mongodb::Collection<Document>, filter: Document) -> Document {
    if matches!(rows.name(), "sandboxes" | "workspaces") {
        query(filter)
    } else {
        filter
    }
}
