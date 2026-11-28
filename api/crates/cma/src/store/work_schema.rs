//! Bridge between the retained-work envelope and the Agent Manager store.

pub(super) mod floor;
pub(super) use cma_am_sandbox_state::work_schema::{
    for_collection, query, read, require, sandbox_version, version, versions,
    MANAGED_MACHINE_ORIGIN_VERSION, PREPARED_VERSION, RENEWED_VERSION,
    RUNTIME_IMAGE_ORIGIN_VERSION, RUNTIME_MIGRATION_VERSION, SANDBOX_VERSIONS,
};

impl super::sandbox_mongo::MongoSandboxStore {
    pub(crate) fn with_renewable_work(mut self, enabled: bool) -> Self {
        self.renewable_work = enabled;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mongodb::bson::{doc, Bson, Document};

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
