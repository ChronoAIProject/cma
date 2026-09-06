use layer3_adapter::hosted::MAX_CHECKPOINT_ARCHIVE_BYTES;
use mongodb::bson::{doc, Bson, DateTime};

use super::model::{
    manifest_document, manifest_indexes, read_manifest, sha256_hex, validate_reference,
    validate_sandbox_id, ArchiveIdentity, ManifestDoc,
};
use crate::store::checkpoint_storage::UPLOAD_LIFETIME;

const SANDBOX: &str = "sbx_checkpoint-test_01";

fn identity(bytes: &[u8]) -> ArchiveIdentity {
    let sha256 = sha256_hex(bytes);
    ArchiveIdentity::from_input(format!("home-{sha256}.cma"), sha256, bytes.len() as u64).unwrap()
}

fn manifest(bytes: &[u8]) -> ManifestDoc {
    ManifestDoc::new(SANDBOX, &identity(bytes), DateTime::now())
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
        "sbx_unicode_\u{e9}",
    ] {
        assert!(validate_sandbox_id(invalid).is_err(), "{invalid}");
    }
    assert!(validate_sandbox_id(&format!("sbx_{}", "a".repeat(125))).is_err());
}

#[test]
fn reference_digest_and_size_must_be_canonical_and_correlated() {
    let valid = identity(b"sealed checkpoint archive");
    assert!(validate_reference(&valid.object_reference).is_ok());
    assert!(
        ArchiveIdentity::from_input(valid.object_reference.clone(), valid.sha256.clone(), 1)
            .is_ok()
    );
    assert!(ArchiveIdentity::from_input(
        valid.object_reference.clone(),
        sha256_hex(b"other bytes"),
        1
    )
    .is_err());
    assert!(ArchiveIdentity::from_input(
        valid.object_reference.to_uppercase(),
        valid.sha256.clone(),
        1
    )
    .is_err());
    assert!(
        ArchiveIdentity::from_input(valid.object_reference.clone(), valid.sha256.clone(), 0)
            .is_err()
    );
    assert!(ArchiveIdentity::from_input(
        valid.object_reference.clone(),
        valid.sha256.clone(),
        MAX_CHECKPOINT_ARCHIVE_BYTES
    )
    .is_ok());
    assert!(ArchiveIdentity::from_input(
        valid.object_reference.clone(),
        valid.sha256.clone(),
        MAX_CHECKPOINT_ARCHIVE_BYTES + 1
    )
    .is_err());
    assert!(validate_reference("home-deadbeef.cma").is_err());
    assert!(validate_reference(&format!("home-{}.tar", valid.sha256)).is_err());
}

#[test]
fn stage_identity_is_unique_sandbox_bound_and_opaque() {
    let identity = identity(b"same bytes");
    let first = ManifestDoc::new("sbx_one", &identity, DateTime::now());
    let second = ManifestDoc::new("sbx_one", &identity, DateTime::now());
    let foreign = ManifestDoc::new("sbx_two", &identity, DateTime::now());
    assert_ne!(first.id, second.id);
    assert_ne!(first.object_key, second.object_key);
    assert_ne!(
        first.object_key.split('/').nth(2),
        foreign.object_key.split('/').nth(2)
    );
    assert_eq!(first.id.len(), "cpa_".len() + 32);
    assert!(!first.object_key.contains("sbx_one"));
    assert!(!first.object_key.contains(&identity.object_reference));
    assert!(first.matches("sbx_one", &identity));
    assert!(!first.matches("sbx_two", &identity));
}

#[test]
fn manifest_round_trips_with_a_bounded_ambiguous_upload_lifetime() {
    let now = DateTime::now();
    let expected = ManifestDoc::new(SANDBOX, &identity(b"round trip archive"), now);
    assert_eq!(
        read_manifest(manifest_document(&expected).unwrap()).unwrap(),
        expected
    );
    assert_eq!(
        expected.cleanup_after.timestamp_millis() - now.timestamp_millis(),
        UPLOAD_LIFETIME.as_millis() as i64
    );
}

#[test]
fn corrupt_manifest_fields_and_cross_sandbox_storage_keys_are_rejected() {
    let expected = manifest(b"manifest corruption cases");
    let foreign = ManifestDoc::new(
        "sbx_foreign",
        &identity(b"manifest corruption cases"),
        DateTime::now(),
    );
    let cases: Vec<(&str, Bson)> = vec![
        ("document_schema_version", Bson::Int64(2)),
        ("sandbox_id", Bson::String("sbx_".into())),
        ("object_reference", Bson::String("home-invalid.cma".into())),
        ("sha256", Bson::String("A".repeat(64))),
        ("size_bytes", Bson::Int64(0)),
        ("object_key", Bson::String(foreign.object_key)),
        ("state", Bson::String("unknown".into())),
        ("state", Bson::String("published".into())),
        ("upload_confirmed", Bson::Boolean(true)),
        ("cleanup_after", Bson::DateTime(DateTime::from_millis(0))),
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
fn indexes_enforce_published_identity_and_support_bounded_cleanup() {
    let indexes = manifest_indexes();
    assert_eq!(indexes.len(), 3);
    assert_eq!(
        indexes[0].keys,
        doc! { "sandbox_id": 1, "object_reference": 1 }
    );
    let published = indexes[0].options.as_ref().unwrap();
    assert_eq!(published.unique, Some(true));
    assert_eq!(
        published.partial_filter_expression,
        Some(doc! { "state": "published" })
    );
    assert_eq!(
        indexes[1].keys,
        doc! { "state": 1, "cleanup_after": 1, "_id": 1 }
    );
    assert_eq!(indexes[2].keys, doc! { "sandbox_id": 1, "_id": 1 });
}

#[test]
fn validation_errors_do_not_echo_untrusted_values() {
    let sandbox_secret = "sbx_secret/path/archive-contents";
    let reference_secret = "home-secret-archive-contents.cma";
    let rendered = format!(
        "{:?} {:?}",
        validate_sandbox_id(sandbox_secret).unwrap_err(),
        validate_reference(reference_secret).unwrap_err()
    );
    assert!(!rendered.contains(sandbox_secret));
    assert!(!rendered.contains(reference_secret));
}
