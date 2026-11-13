//! BSON claim namespaces and checked deadlines for durable title coordination.
use std::time::SystemTime;

use super::{clear_claim, corrupt, insert_claim, read_claim, ClaimFields, StoredClaim};
use layer2_core::sandbox::{
    AgentTitleClaim, AgentTitleSource, AgentTitleStoreError, StoredAgentTitle,
};
use mongodb::bson::{Bson, DateTime, Document};

// Legacy readers reject a stored title beside the original title_lease_* fields.
// The separate top-level envelope is ignored by those readers, even after a crash.
pub(super) const REGENERATION_LEASE: &str = "title_regeneration_lease";

pub(super) fn regeneration_lease(
    document: &Document,
    stored: Option<&StoredAgentTitle>,
) -> Result<Option<StoredClaim>, AgentTitleStoreError> {
    // An older writer can leave this unknown envelope behind when it sets a user
    // title. The canonical title wins and fences that stale regeneration claim.
    if !stored.is_some_and(|title| title.source == AgentTitleSource::Fallback) {
        return Ok(None);
    }
    match document.get(REGENERATION_LEASE) {
        None | Some(Bson::Null) => Ok(None),
        Some(Bson::Document(lease)) => read_claim(lease, ClaimFields::lease()),
        Some(_) => corrupt(),
    }
}

pub(super) fn write_lease(fields: &mut Document, claim: &AgentTitleClaim, regenerating: bool) {
    if regenerating {
        let mut lease = Document::new();
        insert_claim(&mut lease, ClaimFields::lease(), claim);
        clear_claim(fields, ClaimFields::lease());
        fields.insert(REGENERATION_LEASE, lease);
    } else {
        insert_claim(fields, ClaimFields::lease(), claim);
        fields.insert(REGENERATION_LEASE, Bson::Null);
    }
}

pub(super) fn lease_filter(fields: Document, regenerating: bool) -> Document {
    if regenerating {
        fields
            .into_iter()
            .map(|(key, value)| (format!("{REGENERATION_LEASE}.{key}"), value))
            .collect()
    } else {
        fields
    }
}

pub(in crate::store) fn checked_claim_times(
    now: SystemTime,
    expires_at: SystemTime,
) -> Result<(DateTime, DateTime), AgentTitleStoreError> {
    let now = checked_datetime(now)?;
    let expires_at = checked_datetime(expires_at)?;
    Ok((now, expires_at))
}

fn checked_datetime(time: SystemTime) -> Result<DateTime, AgentTitleStoreError> {
    let millis = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| AgentTitleStoreError::InvalidClaim)?
        .as_millis();
    let millis = i64::try_from(millis).map_err(|_| AgentTitleStoreError::InvalidClaim)?;
    Ok(DateTime::from_millis(millis))
}
