use super::*;
use crate::sandbox::lifecycle_test_support::sandbox;
fn held() -> Sandbox {
    let mut s = sandbox();
    s.status = SandboxStatus::Retiring;
    s.lifecycle.retirement_archive = Some(RetirementArchiveRequirement {
        format: 1,
        operation_id: "de98646f-1234-4234-8234-0123456789ab".into(),
        plan_sha256: "a".repeat(64),
        source_sha256: "b".repeat(64),
        exclusion_sha256: "c".repeat(64),
        verified: None,
    });
    s
}
#[test]
fn hold_survives_roundtrip_and_refuses_retirement_and_ordinary_saves() {
    let s = held();
    let s: Sandbox = serde_json::from_slice(&serde_json::to_vec(&s).unwrap()).unwrap();
    assert!(s.retirement_archive_pending());
    assert!(s.validate_retirement_archive().is_ok());
    assert!(s.validate_retirement_archive_successor(&s).is_err());
    let mut next = s.clone();
    next.status = SandboxStatus::Retired;
    assert!(next.validate_retirement_archive().is_err());
    next.lifecycle.retirement_archive = None;
    assert!(s.validate_retirement_archive_successor(&next).is_err());
}
#[test]
fn verified_receipt_is_immutable_and_permits_original_retirement() {
    let mut s = held();
    s.lifecycle.retirement_archive.as_mut().unwrap().verified = Some(RetirementArchiveReceipt {
        manifest_sha256: "d".repeat(64),
        custody_sha256: "e".repeat(64),
        verification_sha256: "f".repeat(64),
    });
    let mut next = s.clone();
    next.status = SandboxStatus::Retired;
    assert!(s.validate_retirement_archive_successor(&next).is_ok());
    next.lifecycle
        .retirement_archive
        .as_mut()
        .unwrap()
        .operation_id = uuid::Uuid::new_v4().to_string();
    assert!(s.validate_retirement_archive_successor(&next).is_err());
    next = s.clone();
    next.status = SandboxStatus::Active;
    assert!(next.validate_retirement_archive().is_err());
}
#[test]
fn malformed_or_manufactured_requirements_are_refused() {
    let s = held();
    let mut ordinary = s.clone();
    ordinary.lifecycle.retirement_archive = None;
    assert!(ordinary.validate_retirement_archive_successor(&s).is_err());
    for bad in ["", "not-a-hash", &"A".repeat(64)] {
        let mut s = held();
        s.lifecycle
            .retirement_archive
            .as_mut()
            .unwrap()
            .source_sha256 = bad.into();
        assert!(s.validate_retirement_archive().is_err());
    }
}

#[test]
fn legacy_records_remain_unchanged_and_future_gate_versions_fail_closed() {
    let ordinary = sandbox();
    let value = serde_json::to_value(&ordinary).unwrap();
    assert!(value["lifecycle"].get("retirement_archive").is_none());
    let parsed: Sandbox = serde_json::from_value(value).unwrap();
    assert_eq!(parsed, ordinary);
    let mut future = held();
    future.lifecycle.retirement_archive.as_mut().unwrap().format = 2;
    assert!(future.validate_retirement_archive().is_err());
    let mut raw = serde_json::to_value(held()).unwrap();
    raw["lifecycle"]["retirement_archive"]["unknown_release_authority"] = serde_json::json!(true);
    assert!(serde_json::from_value::<Sandbox>(raw).is_err());
}
