use super::*;
use crate::retirement_archive::{
    tests::{request, Custody, Temp},
    DatabaseRecord,
};
use layer3_adapter::permanent_custody::PermanentCustody;

#[tokio::test]
async fn frozen_census_before_legal_receipts_still_matches() {
    assert_eq!(manifest_collections(), pre_legal_consent::COLLECTIONS);
    assert_eq!(schema_sha256(), pre_legal_consent::SHA256);
    let current = manifest().await;
    assert!(matches_current(&current, current.clone()).unwrap());
    let mut missing = current;
    missing.database.pop();
    assert!(validate_manifest_census(&missing).is_err());
}

#[test]
fn frozen_legacy_census_matches_the_exact_pre_allocation_descriptor() {
    let audit: Vec<_> = super::super::census_catalog::AUDIT
        .iter()
        .copied()
        .filter(|(name, _)| legacy::COLLECTIONS.contains(name))
        .collect();
    let names: Vec<_> = crate::store::retirement_archive::manifest_collections()
        .iter()
        .copied()
        .filter(|name| legacy::COLLECTIONS.contains(name))
        .collect();
    assert_eq!(names, legacy::COLLECTIONS);
    assert_eq!(
        digest(format!("archive-census-v2\n{audit:?}\n{}", names.join("\n")).as_bytes()),
        legacy::SHA256
    );
    // Old tools accept only their exact schema. New captures require this release
    // or later even if the allocation switch has since been turned back off.
    assert_ne!(schema_sha256(), legacy::SHA256);
}

async fn manifest() -> Manifest {
    let temp = Temp::new();
    let request = request(&temp);
    let custody = Custody::default();
    let receipt = custody.retain(&request.operation_id, vec![]).await.unwrap();
    Manifest {
        format: 1,
        operation_id: request.operation_id,
        request_sha256: digest(&[]),
        original_request: receipt.clone(),
        original_source: receipt.clone(),
        exclusion: receipt,
        custody: custody.policy().await.unwrap(),
        schema_sha256: schema_sha256(),
        external: vec![],
        files: vec![],
        database: crate::store::retirement_archive::manifest_collections()
            .iter()
            .map(|name| DatabaseRecord {
                collection: (*name).into(),
                documents: 0,
                bytes: 0,
                sha256: digest(&[]),
                objects: vec![],
            })
            .collect(),
    }
}

#[tokio::test]
async fn frozen_pre_capture_census_requires_new_evidence_namespaces_to_be_empty() {
    for (collections, schema) in [
        (pre_capture::COLLECTIONS, pre_capture::SHA256),
        (pre_execution::COLLECTIONS, pre_execution::SHA256),
        (
            pre_capture_accounts::COLLECTIONS,
            pre_capture_accounts::SHA256,
        ),
        (
            pre_capture_backups::COLLECTIONS,
            pre_capture_backups::SHA256,
        ),
    ] {
        let audit: Vec<_> = super::super::census_catalog::AUDIT
            .iter()
            .copied()
            .filter(|(name, _)| collections.contains(name))
            .collect();
        let names: Vec<_> = crate::store::retirement_archive::manifest_collections()
            .iter()
            .copied()
            .filter(|name| collections.contains(name))
            .collect();
        assert_eq!(names, collections);
        assert_eq!(
            digest(format!("archive-census-v2\n{audit:?}\n{}", names.join("\n")).as_bytes()),
            schema
        );
        let current = manifest().await;
        let mut old = current.clone();
        old.schema_sha256 = schema.into();
        old.database
            .retain(|row| collections.contains(&row.collection.as_str()));
        assert!(matches_current(&old, current.clone()).unwrap());
        for &name in manifest_collections()
            .iter()
            .filter(|name| !collections.contains(name))
        {
            let mut pending = current.clone();
            pending
                .database
                .iter_mut()
                .find(|row| row.collection == name)
                .unwrap()
                .documents = 1;
            assert!(!matches_current(&old, pending).unwrap());
        }
    }
}

#[tokio::test]
async fn exact_current_and_legacy_censuses_refuse_unknown_duplicate_and_missing_names() {
    let current = manifest().await;
    let mut old = current.clone();
    old.schema_sha256 = legacy::SHA256.into();
    old.database
        .retain(|r| legacy::COLLECTIONS.contains(&r.collection.as_str()));
    for source in [&current, &old] {
        validate_manifest_census(source).unwrap();
        let mut missing = source.clone();
        missing.database.pop();
        assert!(validate_manifest_census(&missing).is_err());
        let mut duplicate = source.clone();
        duplicate.database.push(duplicate.database[0].clone());
        assert!(validate_manifest_census(&duplicate).is_err());
        let mut unknown = source.clone();
        unknown.database[0].collection = "future_unknown".into();
        assert!(validate_manifest_census(&unknown).is_err());
        let mut schema = source.clone();
        schema.schema_sha256 = "0".repeat(64);
        assert!(validate_manifest_census(&schema).is_err());
    }
    let mut mismatched = current.clone();
    mismatched.schema_sha256 = legacy::SHA256.into();
    assert!(validate_manifest_census(&mismatched).is_err());
    mismatched = old.clone();
    mismatched.schema_sha256 = schema_sha256();
    assert!(validate_manifest_census(&mismatched).is_err());
    assert!(matches_current(&old, current.clone()).unwrap());
    for &name in crate::store::retirement_archive::manifest_collections()
        .iter()
        .filter(|name| !legacy::COLLECTIONS.contains(name))
    {
        for mode in 0..5 {
            let mut changed = current.clone();
            let extra = changed
                .database
                .iter_mut()
                .find(|r| r.collection == name)
                .unwrap();
            match mode {
                0 => extra.documents = 1,
                1 => extra.bytes = 5,
                2 => extra.sha256 = digest(b"changed"),
                3 => extra.objects.push(changed.original_source.clone()),
                _ => changed.database[0].documents += 1,
            }
            assert!(!matches_current(&old, changed).unwrap());
        }
    }
    assert!(matches_current(&old, old.clone()).is_err());
    assert!(matches_current(&current, current.clone()).unwrap());
}

#[path = "pre_backup_account_census_tests.rs"]
mod pre_backup_account_tests;
#[path = "pre_backup_census_tests.rs"]
mod pre_backup_tests;
