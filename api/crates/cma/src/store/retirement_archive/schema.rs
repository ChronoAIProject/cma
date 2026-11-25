//! Exact archive census versions. Compatibility never means an arbitrary subset.
use super::{
    census_catalog::{AUDIT, DEPLOYMENT_ONLY, OUTSIDE_MANIFEST},
    *,
};
use crate::retirement_archive::Manifest;
use std::{collections::BTreeSet, sync::LazyLock};

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
#[path = "pre_resource_execution_census.rs"]
mod pre_execution;
#[path = "pre_legal_consent_census.rs"]
#[cfg(test)]
mod pre_legal_consent;
#[cfg(test)]
pub(super) const PRE_LEGAL_CONSENT_SHA256: &str = pre_legal_consent::SHA256;
#[cfg(test)]
pub(super) const PRE_LEGAL_CONSENT_COLLECTIONS: &[&str] = pre_legal_consent::COLLECTIONS;

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

// Creation evidence follows its owner across Sandbox retirement.
const CREATION_PROOFS: &str = super::super::creation_outcomes::COLLECTION;

pub(crate) fn manifest_collections() -> &'static [&'static str] {
    static NAMES: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
        super::super::isolation::COLLECTIONS
            .iter()
            .copied()
            .filter(|name| {
                *name != CREATION_PROOFS
                    && !OUTSIDE_MANIFEST.contains(name)
                    && !DEPLOYMENT_ONLY.contains(name)
            })
            .collect()
    });
    &NAMES
}

pub(crate) fn schema_sha256() -> String {
    let audit: Vec<_> = AUDIT
        .iter()
        .copied()
        .filter(|(name, _)| *name != CREATION_PROOFS && !DEPLOYMENT_ONLY.contains(name))
        .collect();
    digest(
        format!(
            "archive-census-v2\n{audit:?}\n{}",
            manifest_collections().join("\n")
        )
        .as_bytes(),
    )
}

fn collections(schema: &str) -> Result<&'static [&'static str]> {
    if schema == schema_sha256() {
        Ok(manifest_collections())
    } else if schema == pre_execution::SHA256 {
        Ok(pre_execution::COLLECTIONS)
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
