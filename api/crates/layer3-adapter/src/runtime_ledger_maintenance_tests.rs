use super::*;
use serde_json::{json, Value};

fn fixture() -> LedgerBindingV1 {
    LedgerBindingV1 {
        format: LEDGER_BINDING_FORMAT.into(),
        sandbox_id: "fixture-sandbox".into(),
        home_identity: "h".repeat(43),
        generation: 7,
        boot_id: "b".repeat(32),
        roots_sha256: "a".repeat(64),
        source_format: LedgerSourceFormatV1::V1,
        archive_id: uuid::Uuid::from_u128(1).to_string(),
        maintenance_id: "fixture-maintenance".into(),
        maintenance_epoch: 3,
        sandbox_revision: 12,
        candidate_sha256: "c".repeat(64),
        recovery_sha256: "d".repeat(64),
        exclusion_receipt_sha256: "e".repeat(64),
    }
}

#[test]
fn explicit_source_formats_round_trip_with_every_frozen_field() {
    for (source, wire) in [
        (LedgerSourceFormatV1::V1, "v1"),
        (LedgerSourceFormatV1::V2, "v2"),
    ] {
        let mut binding = fixture();
        binding.source_format = source;
        let value = serde_json::to_value(&binding).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 14);
        assert_eq!(value["source_format"], wire);
        assert_eq!(
            serde_json::from_value::<LedgerBindingV1>(value).unwrap(),
            binding
        );
        assert_eq!(binding.validate(), Ok(()));
    }
}

#[test]
fn direct_deserialization_refuses_bad_versions_unknown_and_missing_fields() {
    let original = serde_json::to_value(fixture()).unwrap();
    for field in original.as_object().unwrap().keys() {
        let mut value = original.clone();
        value.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<LedgerBindingV1>(value).is_err());
    }
    for (field, value) in [
        ("format", json!("runtime-ledger-binding-v2")),
        ("source_format", json!("V1")),
        ("source_format", json!("v3")),
        ("source_format", json!(null)),
        ("unknown", json!(true)),
    ] {
        let mut invalid = original.clone();
        invalid[field] = value;
        assert!(serde_json::from_value::<LedgerBindingV1>(invalid).is_err());
    }
}

#[test]
fn direct_deserialization_refuses_invalid_identities_references_and_counters() {
    let original = serde_json::to_value(fixture()).unwrap();
    for (field, invalid) in [
        ("sandbox_id", json!("")),
        ("sandbox_id", json!("leading ")),
        ("sandbox_id", json!("a\nb")),
        ("sandbox_id", json!("x".repeat(257))),
        ("home_identity", json!("h".repeat(42))),
        ("home_identity", json!("!".repeat(43))),
        ("boot_id", json!("B".repeat(32))),
        ("boot_id", json!("b".repeat(31))),
        ("archive_id", json!("missing")),
        ("archive_id", json!(uuid::Uuid::nil().to_string())),
        (
            "archive_id",
            json!(uuid::Uuid::from_u128(1).simple().to_string()),
        ),
        ("maintenance_id", json!("")),
        ("generation", json!(0)),
        ("maintenance_epoch", json!(0)),
        ("sandbox_revision", json!(0)),
        ("sandbox_revision", json!(-1)),
    ] {
        let mut value = original.clone();
        value[field] = invalid;
        assert!(serde_json::from_value::<LedgerBindingV1>(value).is_err());
    }
    for field in [
        "roots_sha256",
        "candidate_sha256",
        "recovery_sha256",
        "exclusion_receipt_sha256",
    ] {
        for invalid in [
            "A".repeat(64),
            "a".repeat(63),
            "g".repeat(64),
            String::new(),
        ] {
            let mut value = original.clone();
            value[field] = json!(invalid);
            assert!(serde_json::from_value::<LedgerBindingV1>(value).is_err());
        }
    }
}

#[test]
fn every_changed_binding_component_refuses_and_changes_structured_identity() {
    let accepted = fixture();
    let original = serde_json::to_value(&accepted).unwrap();
    for (field, changed) in [
        ("sandbox_id", json!("other-sandbox")),
        ("home_identity", json!("z".repeat(43))),
        ("generation", json!(8)),
        ("boot_id", json!("c".repeat(32))),
        ("roots_sha256", json!("f".repeat(64))),
        ("source_format", json!("v2")),
        ("archive_id", json!(uuid::Uuid::from_u128(2).to_string())),
        ("maintenance_id", json!("other-operation")),
        ("maintenance_epoch", json!(4)),
        ("sandbox_revision", json!(13)),
        ("candidate_sha256", json!("f".repeat(64))),
        ("recovery_sha256", json!("f".repeat(64))),
        ("exclusion_receipt_sha256", json!("f".repeat(64))),
    ] {
        let mut value = original.clone();
        value[field] = changed;
        let binding: LedgerBindingV1 = serde_json::from_value(value).unwrap();
        assert_eq!(
            binding.validate_for(&accepted),
            Err(LedgerBindingRefusal::BindingChanged)
        );
        assert_ne!(
            binding.binding_sha256().unwrap(),
            accepted.binding_sha256().unwrap()
        );
    }
    assert_eq!(accepted.validate_for(&accepted), Ok(()));
}

#[test]
fn public_field_mutation_must_not_bypass_validation() {
    let accepted = fixture();
    let mut invalid = accepted.clone();
    invalid.format = "future".into();
    assert_eq!(
        invalid.validate_for(&accepted),
        Err(LedgerBindingRefusal::InvalidEvidence)
    );
    assert_eq!(
        invalid.binding_sha256(),
        Err(LedgerBindingRefusal::InvalidEvidence)
    );
    assert_eq!(
        accepted.validate_for(&invalid),
        Err(LedgerBindingRefusal::InvalidEvidence)
    );
}

#[test]
fn exact_bytes_survive_parse_and_are_distinct_from_structured_identity() {
    let binding = fixture();
    let compact = serde_json::to_vec(&binding).unwrap();
    let pretty = serde_json::to_vec_pretty(&binding).unwrap();
    let first = LedgerBindingArtifactV1::parse(&compact).unwrap();
    let second = LedgerBindingArtifactV1::parse(&pretty).unwrap();
    assert_eq!(first.original_bytes(), compact);
    assert_eq!(second.original_bytes(), pretty);
    assert_eq!(first.binding(), second.binding());
    assert_eq!(
        first.binding().binding_sha256(),
        second.binding().binding_sha256()
    );
    assert_ne!(first.bytes_sha256(), second.bytes_sha256());
    assert_eq!(
        first.bytes_sha256(),
        format!("{:x}", Sha256::digest(&compact))
    );
    assert_ne!(first.bytes_sha256(), binding.binding_sha256().unwrap());
}

#[test]
fn schema_duplicate_trailing_and_oversized_artifacts_refuse() {
    let original = serde_json::to_string(&fixture()).unwrap();
    let duplicate = original.replacen('{', "{\"generation\":7,", 1);
    for bytes in [
        b"null".to_vec(),
        duplicate.into_bytes(),
        format!("{original} {{}}").into_bytes(),
    ] {
        assert!(matches!(
            LedgerBindingArtifactV1::parse(&bytes),
            Err(LedgerBindingRefusal::InvalidEvidence)
        ));
    }
    let mut max = original.into_bytes();
    max.resize(MAX_LEDGER_BINDING_BYTES, b' ');
    assert!(LedgerBindingArtifactV1::parse(&max).is_ok());
    max.push(b' ');
    assert!(matches!(
        LedgerBindingArtifactV1::parse(&max),
        Err(LedgerBindingRefusal::Oversized)
    ));
}

#[test]
fn debug_exposes_only_counts_and_hashes_even_for_invalid_in_process_values() {
    let mut binding = fixture();
    let artifact = LedgerBindingArtifactV1::parse(&serde_json::to_vec(&binding).unwrap()).unwrap();
    binding.format = "private-invalid-format".into();
    let debug = format!("{binding:?} {artifact:?}");
    let value = serde_json::to_value(binding).unwrap();
    for field in [
        "format",
        "sandbox_id",
        "home_identity",
        "boot_id",
        "archive_id",
        "maintenance_id",
    ] {
        assert!(!debug.contains(value[field].as_str().unwrap()));
    }
    assert!(debug.contains(artifact.bytes_sha256()));
}

#[test]
fn length_prefixed_identity_does_not_alias_component_boundaries() {
    let mut a = fixture();
    let mut b = fixture();
    a.sandbox_id = "a/b".into();
    a.maintenance_id = "c".into();
    b.sandbox_id = "a".into();
    b.maintenance_id = "b/c".into();
    assert_ne!(a.binding_sha256().unwrap(), b.binding_sha256().unwrap());
    let json: Value = serde_json::from_slice(&serde_json::to_vec(&a).unwrap()).unwrap();
    assert_eq!(serde_json::from_value::<LedgerBindingV1>(json).unwrap(), a);
}
