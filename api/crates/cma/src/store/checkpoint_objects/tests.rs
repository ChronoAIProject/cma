use layer3_adapter::hosted::MAX_CHECKPOINT_ARCHIVE_BYTES;
use mongodb::bson::{doc, oid::ObjectId, Bson, DateTime, Document};
use mongodb::gridfs::FilesCollectionDocument;
use mongodb::options::{Acknowledgment, ReadConcernLevel};

use super::model::{
    bucket_options, chunk_indexes, file_indexes, manifest_document, manifest_id, manifest_indexes,
    metadata_document, read_manifest, sha256_hex, validate_gridfs_file, validate_reference,
    validate_sandbox_id, ArchiveIdentity, GridFsMetadata, ManifestDoc, GRIDFS_BUCKET,
    GRIDFS_CHUNK_BYTES,
};

const SANDBOX: &str = "sbx_checkpoint-test_01";

fn identity(bytes: &[u8]) -> ArchiveIdentity {
    let sha256 = sha256_hex(bytes);
    ArchiveIdentity::from_input(format!("home-{sha256}.cma"), sha256, bytes.len() as u64).unwrap()
}

fn manifest(bytes: &[u8]) -> ManifestDoc {
    ManifestDoc::new(SANDBOX, &identity(bytes), ObjectId::new())
}

fn file_document(manifest: &ManifestDoc) -> Document {
    doc! {
        "_id": manifest.gridfs_id,
        "length": manifest.size_bytes,
        "chunkSize": manifest.chunk_size_bytes,
        "uploadDate": DateTime::now(),
        "filename": &manifest.object_reference,
        "metadata": metadata_document(manifest).unwrap(),
    }
}

fn file_from(document: Document) -> FilesCollectionDocument {
    mongodb::bson::deserialize_from_document(document).unwrap()
}

#[test]
fn sandbox_identity_is_scoped_bounded_and_storage_safe() {
    for valid in ["sbx_a", "sbx_123", "sbx_legacy-name", SANDBOX] {
        assert!(validate_sandbox_id(valid).is_ok(), "{valid}");
    }
    for invalid in [
        "",
        "sbx_",
        "sandbox_a",
        "sbx_has/slash",
        "sbx_has.dot",
        "sbx_unicode_é",
    ] {
        assert!(validate_sandbox_id(invalid).is_err(), "{invalid}");
    }
    assert!(validate_sandbox_id(&format!("sbx_{}", "a".repeat(125))).is_err());
}

#[test]
fn reference_digest_and_size_must_be_canonical_and_correlated() {
    let bytes = b"sealed checkpoint archive";
    let valid = identity(bytes);
    assert!(validate_reference(&valid.object_reference).is_ok());
    assert!(
        ArchiveIdentity::from_input(valid.object_reference.clone(), valid.sha256.clone(), 1,)
            .is_ok()
    );

    let other = sha256_hex(b"other bytes");
    assert!(ArchiveIdentity::from_input(valid.object_reference.clone(), other, 1).is_err());
    assert!(ArchiveIdentity::from_input(
        valid.object_reference.to_uppercase(),
        valid.sha256.clone(),
        1,
    )
    .is_err());
    assert!(
        ArchiveIdentity::from_input(valid.object_reference.clone(), valid.sha256.clone(), 0,)
            .is_err()
    );
    assert!(ArchiveIdentity::from_input(
        valid.object_reference.clone(),
        valid.sha256.clone(),
        MAX_CHECKPOINT_ARCHIVE_BYTES,
    )
    .is_ok());
    assert!(ArchiveIdentity::from_input(
        valid.object_reference.clone(),
        valid.sha256.clone(),
        MAX_CHECKPOINT_ARCHIVE_BYTES + 1,
    )
    .is_err());
    assert!(validate_reference("home-deadbeef.cma").is_err());
    assert!(validate_reference(&format!("home-{}.tar", valid.sha256)).is_err());
}

#[test]
fn manifest_identity_is_deterministic_sandbox_bound_and_opaque() {
    let reference = identity(b"same bytes").object_reference;
    let first = manifest_id("sbx_one", &reference);
    assert_eq!(first, manifest_id("sbx_one", &reference));
    assert_ne!(first, manifest_id("sbx_two", &reference));
    assert_eq!(first.len(), "cpo_".len() + 64);
    assert!(!first.contains("sbx_one"));
    assert!(!first.contains(&reference));
}

#[test]
fn manifest_and_gridfs_metadata_round_trip_exactly() {
    let expected = manifest(b"round trip archive");
    let decoded = read_manifest(manifest_document(&expected).unwrap()).unwrap();
    assert_eq!(decoded, expected);

    let file = file_from(file_document(&expected));
    let metadata = validate_gridfs_file(&file, &expected).unwrap();
    assert_eq!(metadata, GridFsMetadata::from(&expected));
    assert_eq!(metadata.manifest(), expected);
}

#[test]
fn corrupt_manifest_fields_are_rejected() {
    let expected = manifest(b"manifest corruption cases");
    let cases: Vec<(&str, Bson)> = vec![
        ("document_schema_version", Bson::Int64(2)),
        ("sandbox_id", Bson::String("sbx_".into())),
        ("object_reference", Bson::String("home-invalid.cma".into())),
        ("sha256", Bson::String("A".repeat(64))),
        ("size_bytes", Bson::Int64(0)),
        ("chunk_size_bytes", Bson::Int64(1)),
        ("_id", Bson::String("raw-storage-key".into())),
    ];
    for (field, value) in cases {
        let mut document = manifest_document(&expected).unwrap();
        document.insert(field, value);
        assert!(read_manifest(document).is_err(), "accepted corrupt {field}");
    }

    let mut extra = manifest_document(&expected).unwrap();
    extra.insert("unexpected", true);
    assert!(read_manifest(extra).is_err());
}

#[test]
fn gridfs_identity_shape_and_ownership_are_rechecked() {
    let expected = manifest(b"file corruption cases");
    let valid = file_document(&expected);
    let mut cases = Vec::new();

    let mut wrong_id = valid.clone();
    wrong_id.insert("_id", ObjectId::new());
    cases.push(wrong_id);
    let mut wrong_name = valid.clone();
    wrong_name.insert("filename", "home-wrong.cma");
    cases.push(wrong_name);
    let mut wrong_length = valid.clone();
    wrong_length.insert("length", expected.size_bytes + 1);
    cases.push(wrong_length);
    let mut wrong_chunk_size = valid.clone();
    wrong_chunk_size.insert("chunkSize", i64::from(GRIDFS_CHUNK_BYTES) / 2);
    cases.push(wrong_chunk_size);
    for (field, value) in [
        ("sandbox_id", Bson::String("sbx_foreign".into())),
        ("gridfs_id", Bson::ObjectId(ObjectId::new())),
        ("document_schema_version", Bson::Int64(9)),
        ("size_bytes", Bson::Int64(expected.size_bytes + 1)),
        ("sha256", Bson::String("f".repeat(64))),
    ] {
        let mut document = valid.clone();
        document
            .get_document_mut("metadata")
            .unwrap()
            .insert(field, value);
        cases.push(document);
    }

    for document in cases {
        let file = file_from(document);
        assert!(validate_gridfs_file(&file, &expected).is_err());
    }
}

#[test]
fn indexes_enforce_identity_and_support_scoped_purge() {
    let manifests = manifest_indexes();
    assert_eq!(manifests.len(), 3);
    assert_eq!(
        manifests[0].keys,
        doc! { "sandbox_id": 1, "object_reference": 1 }
    );
    assert_eq!(
        manifests[0]
            .options
            .as_ref()
            .and_then(|options| options.unique),
        Some(true)
    );
    assert_eq!(manifests[1].keys, doc! { "gridfs_id": 1 });
    assert_eq!(
        manifests[1]
            .options
            .as_ref()
            .and_then(|options| options.unique),
        Some(true)
    );
    assert_eq!(manifests[2].keys, doc! { "sandbox_id": 1, "_id": 1 });

    let files = file_indexes();
    assert!(files
        .iter()
        .any(|index| index.keys == doc! { "metadata.sandbox_id": 1, "_id": 1 }));
    let chunks = chunk_indexes();
    assert_eq!(chunks[0].keys, doc! { "files_id": 1, "n": 1 });
    assert_eq!(
        chunks[0]
            .options
            .as_ref()
            .and_then(|options| options.unique),
        Some(true)
    );
}

#[test]
fn gridfs_bucket_requires_majority_reads_and_writes() {
    let options = bucket_options();
    assert_eq!(options.bucket_name.as_deref(), Some(GRIDFS_BUCKET));
    assert_eq!(options.chunk_size_bytes, Some(GRIDFS_CHUNK_BYTES));
    assert_eq!(
        options.read_concern.as_ref().map(|concern| &concern.level),
        Some(&ReadConcernLevel::Majority)
    );
    assert_eq!(
        options
            .write_concern
            .as_ref()
            .and_then(|concern| concern.w.as_ref()),
        Some(&Acknowledgment::Majority)
    );
}

#[test]
fn validation_errors_and_ids_do_not_echo_untrusted_values() {
    let sandbox_secret = "sbx_secret/path/archive-contents";
    let reference_secret = "home-secret-archive-contents.cma";
    let sandbox_error = validate_sandbox_id(sandbox_secret).unwrap_err();
    let reference_error = validate_reference(reference_secret).unwrap_err();
    let rendered = format!("{sandbox_error:?} {reference_error:?}");
    assert!(!rendered.contains(sandbox_secret));
    assert!(!rendered.contains(reference_secret));

    let id = manifest_id("sbx_private-owner", "home-private-object.cma");
    assert!(!id.contains("private-owner"));
    assert!(!id.contains("private-object"));
}
