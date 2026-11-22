//! Exact archive census versions. Compatibility never means an arbitrary subset.
use super::*;
use crate::retirement_archive::Manifest;
use std::collections::BTreeSet;

#[path = "legacy_census.rs"]
mod legacy;
#[path = "pre_backup_census.rs"]
mod pre_backup;
#[path = "pre_backup_account_census.rs"]
mod pre_backup_accounts;
#[path = "pre_usage_capture_census.rs"]
mod pre_capture;
#[path = "pre_usage_capture_account_census.rs"]
mod pre_capture_accounts;
#[path = "pre_usage_capture_backup_census.rs"]
mod pre_capture_backups;

#[cfg(test)]
pub(super) const PRE_BACKUP_COLLECTIONS: &[&str] = pre_backup::COLLECTIONS;
#[cfg(test)]
pub(super) const PRE_BACKUP_SHA256: &str = pre_backup::SHA256;
#[cfg(test)]
pub(super) const PRE_BACKUP_ACCOUNT_COLLECTIONS: &[&str] = pre_backup_accounts::COLLECTIONS;
#[cfg(test)]
pub(super) const PRE_BACKUP_ACCOUNT_SHA256: &str = pre_backup_accounts::SHA256;

#[cfg(test)]
pub(super) const LEGACY_COLLECTIONS: &[&str] = legacy::COLLECTIONS;
#[cfg(test)]
pub(super) const LEGACY_SHA256: &str = legacy::SHA256;

pub(crate) fn schema_sha256() -> String {
    digest(
        format!(
            "archive-census-v2\n{:?}\n{}",
            super::census_catalog::AUDIT,
            super::super::isolation::COLLECTIONS.join("\n")
        )
        .as_bytes(),
    )
}

fn collections(schema: &str) -> Result<&'static [&'static str]> {
    if schema == schema_sha256() {
        Ok(super::super::isolation::COLLECTIONS)
    } else if schema == pre_capture::SHA256 {
        Ok(pre_capture::COLLECTIONS)
    } else if schema == pre_capture_accounts::SHA256 {
        Ok(pre_capture_accounts::COLLECTIONS)
    } else if schema == pre_capture_backups::SHA256 {
        Ok(pre_capture_backups::COLLECTIONS)
    } else if schema == pre_backup::SHA256 {
        Ok(pre_backup::COLLECTIONS)
    } else if schema == pre_backup_accounts::SHA256 {
        Ok(pre_backup_accounts::COLLECTIONS)
    } else if schema == legacy::SHA256 {
        Ok(legacy::COLLECTIONS)
    } else {
        Err(REFUSED)
    }
}

pub(crate) fn validate_manifest_census(manifest: &Manifest) -> Result<()> {
    let expected = collections(&manifest.schema_sha256)?;
    let names: BTreeSet<_> = manifest
        .database
        .iter()
        .map(|r| r.collection.as_str())
        .collect();
    if names.len() != manifest.database.len()
        || names.len() != expected.len()
        || !expected.iter().all(|name| names.contains(name))
    {
        return Err(REFUSED);
    }
    Ok(())
}

/// Reconcile a pending old archive with a fresh CURRENT census. A new namespace
/// may be removed from this comparison only when its complete owned inventory is
/// canonically empty. New captures always publish the current schema and census.
pub(crate) fn matches_current(manifest: &Manifest, mut current: Manifest) -> Result<bool> {
    validate_manifest_census(manifest)?;
    validate_manifest_census(&current)?;
    if current.schema_sha256 != schema_sha256() {
        return Err(REFUSED);
    }
    if manifest.schema_sha256 != current.schema_sha256 {
        let previous = collections(&manifest.schema_sha256)?;
        for record in &current.database {
            if !previous.contains(&record.collection.as_str())
                && (record.documents != 0
                    || record.bytes != 0
                    || record.sha256 != digest(&[])
                    || !record.objects.is_empty())
            {
                return Ok(false);
            }
        }
        current
            .database
            .retain(|r| previous.contains(&r.collection.as_str()));
        current.schema_sha256 = manifest.schema_sha256.clone();
    }
    Ok(&current == manifest)
}

#[cfg(test)]
#[path = "schema_tests.rs"]
mod tests;
