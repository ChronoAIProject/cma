use super::*;

#[test]
fn frozen_pre_backup_census_matches_the_previous_release_descriptor() {
    let audit: Vec<_> = super::super::super::census_catalog::AUDIT
        .iter()
        .copied()
        .filter(|(name, _)| pre_backup::COLLECTIONS.contains(name))
        .collect();
    let names: Vec<_> = crate::store::retirement_archive::manifest_collections()
        .iter()
        .copied()
        .filter(|name| pre_backup::COLLECTIONS.contains(name))
        .collect();
    assert_eq!(names, pre_backup::COLLECTIONS);
    assert_eq!(
        digest(format!("archive-census-v2\n{audit:?}\n{}", names.join("\n")).as_bytes()),
        pre_backup::SHA256
    );
}

#[tokio::test]
async fn previous_archive_census_only_reconciles_canonically_empty_backup_collections() {
    let current = manifest().await;
    let mut previous = current.clone();
    previous.schema_sha256 = pre_backup::SHA256.into();
    previous
        .database
        .retain(|row| pre_backup::COLLECTIONS.contains(&row.collection.as_str()));
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
    let mut missing = previous.clone();
    missing.database.pop();
    assert!(validate_manifest_census(&missing).is_err());
    let mut unknown = previous.clone();
    unknown.schema_sha256 = "0".repeat(64);
    assert!(validate_manifest_census(&unknown).is_err());
    let mut duplicate = previous.clone();
    duplicate.database.push(previous.database[0].clone());
    assert!(validate_manifest_census(&duplicate).is_err());
    assert!(matches_current(&previous, previous.clone()).is_err());
}
