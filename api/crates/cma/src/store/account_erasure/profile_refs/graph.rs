//! Validate owned edges without trusting a single denormalized identity.
use super::*;
use std::collections::BTreeMap;
/// Any available identity can establish that a damaged row belongs to this
/// cohort. Never silently lose an owned edge because one query field is bad.
pub(super) fn owned_revision<'a>(
    name: &'static str,
    row: &Document,
    owners: &'a BTreeMap<String, (String, u8)>,
) -> Result<Option<(&'a str, &'a str, u64)>, Error> {
    let record = row.get_document("record").ok();
    let candidate = [
        record.and_then(|record| record.get_str("profile_id").ok()),
        row.get_str("profile_id").ok(),
        row.get_str("_id")
            .ok()
            .and_then(|key| key.rsplit_once(':').map(|(id, _)| id)),
    ]
    .into_iter()
    .flatten()
    .find_map(|id| owners.get_key_value(id));
    let Some((id, (owner, kind))) = candidate else {
        return Ok(None);
    };
    let invalid = || Error::InvalidRecord(name).attribute(owner);
    let record = record.ok_or_else(invalid)?;
    let revision = number(record.get("revision"))
        .filter(|revision| (1..=u64::from(u32::MAX)).contains(revision))
        .ok_or_else(invalid)?;
    if crate::store::sandbox_mongo::require_version(row).is_err()
        || REVISIONS.get(usize::from(*kind)).copied() != Some(name)
        || row.get_str("profile_id").ok() != Some(id.as_str())
        || record.get_str("profile_id").ok() != Some(id.as_str())
        || number(row.get("revision")) != Some(revision)
        || row.get_str("_id").ok() != Some(key(id, revision).as_str())
        || record.get_document("content").is_err()
    {
        return Err(invalid());
    }
    Ok(Some((id, owner, revision)))
}
pub(in crate::store::account_erasure::profile_refs) fn key(id: &str, revision: u64) -> String {
    format!("{id}:{revision:010}")
}
fn number(value: Option<&Bson>) -> Option<u64> {
    match value {
        Some(Bson::Int32(v)) => u64::try_from(*v).ok(),
        Some(Bson::Int64(v)) => u64::try_from(*v).ok(),
        _ => None,
    }
}
pub(super) fn collect(value: Option<&Bson>, output: &mut BTreeSet<String>) {
    match value {
        Some(Bson::Document(row)) => {
            if let (Some(id), Some(revision)) = (
                row.get_str("id")
                    .or_else(|_| row.get_str("profile_id"))
                    .ok(),
                number(row.get("revision")),
            ) {
                if id.starts_with("agp_") || id.starts_with("wkp_") || id.starts_with("sbp_") {
                    output.insert(key(id, revision));
                }
            }
            for value in row.values() {
                collect(Some(value), output);
            }
        }
        Some(Bson::Array(values)) => {
            for value in values {
                collect(Some(value), output);
            }
        }
        _ => {}
    }
}
