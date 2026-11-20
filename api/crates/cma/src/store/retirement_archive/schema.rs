//! Exact archive census versions. Compatibility never means an arbitrary subset.
use super::*;
use crate::retirement_archive::Manifest;
use std::collections::BTreeSet;

#[path = "legacy_census.rs"]
mod legacy;
#[path = "pre_usage_capture_census.rs"]
mod pre_capture;

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

fn collections(schema: &str) -> Result<Vec<&'static str>> {
    if schema == schema_sha256() {
        Ok(super::super::isolation::COLLECTIONS.to_vec())
    } else if schema == pre_capture::SHA256 {
        Ok(pre_capture::COLLECTIONS.to_vec())
    } else if schema == legacy::SHA256 {
        Ok(legacy::COLLECTIONS.to_vec())
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
        let prior = collections(&manifest.schema_sha256)?;
        for record in &current.database {
            if !prior.contains(&record.collection.as_str())
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
            .retain(|r| prior.contains(&r.collection.as_str()));
        current.schema_sha256 = manifest.schema_sha256.clone();
    }
    Ok(&current == manifest)
}

#[cfg(test)]
#[path = "schema_tests.rs"]
mod tests;
