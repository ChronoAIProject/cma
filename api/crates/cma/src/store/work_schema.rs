//! Retained ordinary-work envelope: old version-one readers and CAS writers fail closed.

use layer2_core::sandbox::store::WorkAuthorityProtocol;
use layer2_core::{released_runtime::ReleasedLane, sandbox::Sandbox};
use mongodb::bson::{doc, Bson, Document};
use serde::de::DeserializeOwned;

pub(super) mod floor;

pub(super) const RENEWED_VERSION: i64 = 3;
/// Version four is the first envelope that may contain the strict warm
/// installation proof. Older renewed readers must refuse it before decoding.
pub(super) const PREPARED_VERSION: i64 = 4;
/// Custom image and scoped migration authority require compatible readers.
pub(super) const RUNTIME_MIGRATION_VERSION: i64 = 5;
/// The Sandbox record contains authoritative image-selection provenance.
pub(super) const RUNTIME_IMAGE_ORIGIN_VERSION: i64 = 6;
/// An explicitly selected image can carry managed Machine release authority.
pub(super) const MANAGED_MACHINE_ORIGIN_VERSION: i64 = 7;

pub(super) fn version(protocol: Option<WorkAuthorityProtocol>, prepared_installation: bool) -> i64 {
    if prepared_installation {
        PREPARED_VERSION
    } else if protocol.is_some() {
        RENEWED_VERSION
    } else {
        1
    }
}

pub(super) fn sandbox_version(sandbox: &Sandbox) -> i64 {
    if sandbox.managed_runtime_lane() == Some(ReleasedLane::Machine) {
        MANAGED_MACHINE_ORIGIN_VERSION
    } else if sandbox.lifecycle.runtime_image_origin.is_some() {
        RUNTIME_IMAGE_ORIGIN_VERSION
    } else if sandbox.lifecycle.custom_runtime.is_some()
        || sandbox.lifecycle.runtime_migration_admission.is_some()
    {
        RUNTIME_MIGRATION_VERSION
    } else {
        version(
            sandbox.work.authority_protocol,
            sandbox.work.prepared_installation_digest.is_some(),
        )
    }
}

pub(super) const SANDBOX_VERSIONS: [i64; 6] = [
    1,
    RENEWED_VERSION,
    PREPARED_VERSION,
    RUNTIME_MIGRATION_VERSION,
    RUNTIME_IMAGE_ORIGIN_VERSION,
    MANAGED_MACHINE_ORIGIN_VERSION,
];

pub(super) fn versions() -> Document {
    doc! {"$in": SANDBOX_VERSIONS.to_vec()}
}

pub(super) fn require(document: &Document) -> Result<(), String> {
    let version = match document.get("document_schema_version") {
        Some(Bson::Int64(version)) => Some(*version),
        Some(Bson::Int32(version)) => Some(i64::from(*version)),
        _ => None,
    };
    if version.is_some_and(|version| SANDBOX_VERSIONS.contains(&version)) {
        Ok(())
    } else {
        Err("unsupported ordinary-work envelope".into())
    }
}

pub(super) fn read<T: DeserializeOwned>(document: &Document) -> Result<T, String> {
    let record = validated_record(document)?;
    mongodb::bson::deserialize_from_document(record.clone()).map_err(|error| error.to_string())
}

pub(super) fn read_owned<T: DeserializeOwned>(document: &mut Document) -> Result<T, String> {
    validated_record(document)?;
    let record = document.remove("record").ok_or("missing work record")?;
    mongodb::bson::deserialize_from_bson(record).map_err(|error| error.to_string())
}

fn validated_record(document: &Document) -> Result<&Document, String> {
    require(document)?;
    let record = document
        .get_document("record")
        .map_err(|_| "missing work record")?;
    let protocol = record
        .get_document("work")
        .map_err(|_| "missing work state")?
        .get("authority_protocol");
    let version = document.get("document_schema_version");
    let renewed = matches!(
        protocol,
        Some(Bson::String(value)) if value == "renewed_v1"
    );
    let proof = record
        .get_document("work")
        .ok()
        .is_some_and(|work| work.contains_key("prepared_installation_digest"));
    let migration = record
        .get_document("lifecycle")
        .ok()
        .is_some_and(|lifecycle| {
            lifecycle.contains_key("custom_runtime")
                || lifecycle.contains_key("runtime_migration_admission")
        });
    let origin = record
        .get_document("lifecycle")
        .ok()
        .and_then(|lifecycle| lifecycle.get("runtime_image_origin"));
    let valid_origin = match version {
        Some(Bson::Int64(RUNTIME_IMAGE_ORIGIN_VERSION) | Bson::Int32(6)) => {
            matches!(origin, Some(Bson::String(value)) if matches!(value.as_str(), "managed_desktop" | "user_selected"))
        }
        Some(Bson::Int64(MANAGED_MACHINE_ORIGIN_VERSION) | Bson::Int32(7)) => {
            matches!(origin, Some(Bson::String(value)) if value == "managed_machine") && !migration
        }
        _ => origin.is_none(),
    };
    if !valid_origin {
        return Err("runtime image origin disagrees with ordinary-work envelope".into());
    }
    let valid_version = match version {
        Some(Bson::Int64(1) | Bson::Int32(1)) => !renewed && !proof && !migration,
        Some(Bson::Int64(RENEWED_VERSION) | Bson::Int32(3)) => renewed && !proof && !migration,
        Some(Bson::Int64(PREPARED_VERSION) | Bson::Int32(4)) => proof && !migration,
        Some(Bson::Int64(RUNTIME_MIGRATION_VERSION) | Bson::Int32(5)) => migration,
        Some(
            Bson::Int64(RUNTIME_IMAGE_ORIGIN_VERSION | MANAGED_MACHINE_ORIGIN_VERSION)
            | Bson::Int32(6 | 7),
        ) => true,
        _ => false,
    };
    if !valid_version || (protocol.is_some() && !renewed) {
        return Err("ordinary-work writer floor disagrees with envelope".into());
    }
    Ok(record)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn record(version: i64, protocol: Option<&str>, proof: bool) -> Document {
        let mut work = doc! {};
        if let Some(protocol) = protocol {
            work.insert("authority_protocol", protocol);
        }
        if proof {
            work.insert("prepared_installation_digest", "a".repeat(64));
        }
        doc! {"document_schema_version": version, "record": {"work": work}}
    }

    #[test]
    fn proof_schema_roundtrips_with_authority_off_or_on() {
        for (version, protocol, proof) in [
            (1, None, false),
            (RENEWED_VERSION, Some("renewed_v1"), false),
            (PREPARED_VERSION, None, true),
            (PREPARED_VERSION, Some("renewed_v1"), true),
        ] {
            assert!(read::<Document>(&record(version, protocol, proof)).is_ok());
        }
    }

    #[test]
    fn image_origin_requires_its_envelope_and_a_known_marker() {
        let mut document = record(RUNTIME_IMAGE_ORIGIN_VERSION, Some("renewed_v1"), false);
        assert!(read::<Document>(&document).is_err());
        let retained = document.get_document_mut("record").unwrap();
        retained.insert(
            "lifecycle",
            doc! {"runtime_image_origin": "managed_desktop"},
        );
        assert!(read::<Document>(&document).is_ok());
        document.insert("document_schema_version", RENEWED_VERSION);
        assert!(read::<Document>(&document).is_err());
        document.insert("document_schema_version", RUNTIME_IMAGE_ORIGIN_VERSION);
        document
            .get_document_mut("record")
            .unwrap()
            .insert("lifecycle", doc! {"runtime_image_origin": Bson::Null});
        assert!(read::<Document>(&document).is_err());
    }

    #[test]
    fn machine_origin_has_its_own_rollback_envelope() {
        let mut document = record(MANAGED_MACHINE_ORIGIN_VERSION, Some("renewed_v1"), false);
        let retained = document.get_document_mut("record").unwrap();
        retained.insert(
            "lifecycle",
            doc! {"runtime_image_origin": "managed_machine"},
        );
        assert!(read::<Document>(&document).is_ok());
        document.insert("document_schema_version", RUNTIME_IMAGE_ORIGIN_VERSION);
        assert!(read::<Document>(&document).is_err());
        document.insert("document_schema_version", MANAGED_MACHINE_ORIGIN_VERSION);
        document.get_document_mut("record").unwrap().insert(
            "lifecycle",
            doc! {"runtime_image_origin": "managed_machine", "custom_runtime": {}},
        );
        assert!(read::<Document>(&document).is_err());
    }

    #[test]
    fn proof_schema_rejects_mismatched_or_unknown_envelopes() {
        for document in [
            record(RENEWED_VERSION, Some("renewed_v1"), true),
            record(PREPARED_VERSION, None, false),
            record(1, Some("renewed_v1"), false),
            record(PREPARED_VERSION, Some("other"), true),
        ] {
            assert!(read::<Document>(&document).is_err());
        }
    }
}
