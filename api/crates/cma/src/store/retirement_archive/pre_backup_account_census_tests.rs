use super::*;

#[test]
fn frozen_account_pre_backup_census_matches_the_exact_develop_descriptor() {
    let audit: Vec<_> = super::super::super::census_catalog::AUDIT
        .iter()
        .copied()
        .filter(|(name, _)| pre_backup_accounts::COLLECTIONS.contains(name))
        .collect();
    let names: Vec<_> = crate::store::retirement_archive::manifest_collections()
        .iter()
        .copied()
        .filter(|name| pre_backup_accounts::COLLECTIONS.contains(name))
        .collect();
    assert_eq!(names, pre_backup_accounts::COLLECTIONS);
    assert_eq!(
        digest(format!("archive-census-v2\n{audit:?}\n{}", names.join("\n")).as_bytes()),
        pre_backup_accounts::SHA256
    );
    let introduced: Vec<_> = crate::store::retirement_archive::manifest_collections()
        .iter()
        .copied()
        .filter(|name| !pre_backup_accounts::COLLECTIONS.contains(name))
        .collect();
    assert_eq!(
        introduced,
        [
            "resource_execution_effects_v1",
            "resource_execution_grants_v1",
            "resource_execution_records_v1",
            "resource_revocation_operations_v1",
            crate::store::sandbox_usage_export::CAPTURES,
            crate::store::sandbox_usage_export::EVIDENCE,
            crate::store::sandbox_usage_export::OUTBOX,
            crate::store::sandbox_usage_export::STREAMS,
            crate::store::sandbox_usage_export::INVENTORY,
            crate::store::home_backups::SCHEDULES_COLLECTION,
            crate::store::home_backups::BACKUPS_COLLECTION,
        ]
    );
}

#[tokio::test]
async fn account_pre_backup_archive_requires_exact_census_and_empty_new_backup_inventory() {
    let current = manifest().await;
    let mut previous = current.clone();
    previous.schema_sha256 = pre_backup_accounts::SHA256.into();
    previous
        .database
        .retain(|row| pre_backup_accounts::COLLECTIONS.contains(&row.collection.as_str()));
    validate_manifest_census(&previous).unwrap();
    assert!(matches_current(&previous, current.clone()).unwrap());

    for name in [
        crate::store::home_backups::SCHEDULES_COLLECTION,
        crate::store::home_backups::BACKUPS_COLLECTION,
    ] {
        for change in 0..4 {
            let mut fresh = current.clone();
            let row = fresh
                .database
                .iter_mut()
                .find(|row| row.collection == name)
                .unwrap();
            match change {
                0 => row.documents = 1,
                1 => row.bytes = 1,
                2 => row.sha256 = digest(b"different"),
                _ => row.objects.push(previous.original_source.clone()),
            }
            assert!(!matches_current(&previous, fresh).unwrap());
        }
    }

    for index in 0..previous.database.len() {
        let mut missing = previous.clone();
        missing.database.remove(index);
        assert!(validate_manifest_census(&missing).is_err());
        let mut duplicate = previous.clone();
        duplicate.database.push(previous.database[index].clone());
        assert!(validate_manifest_census(&duplicate).is_err());
    }
    for hash in [
        "0".repeat(64),
        pre_backup::SHA256.into(),
        legacy::SHA256.into(),
    ] {
        let mut unknown = previous.clone();
        unknown.schema_sha256 = hash;
        assert!(validate_manifest_census(&unknown).is_err());
    }
    let mut unknown = previous.clone();
    unknown.database[0].collection = "future_unknown".into();
    assert!(validate_manifest_census(&unknown).is_err());
    assert!(matches_current(&previous, previous.clone()).is_err());

    for name in ["account_deletions", "account_deletion_events"] {
        let mut changed = current.clone();
        changed
            .database
            .iter_mut()
            .find(|row| row.collection == name)
            .unwrap()
            .sha256 = digest(b"changed account custody");
        assert!(!matches_current(&previous, changed).unwrap());
    }
}
