//! Prove desktop leases expired from the same snapshot used for pause admission.
use layer2_core::{
    sandbox::{has_open_desktop_session, InactivityPauseRefusal as Refusal},
    seconds_from_rfc3339, SandboxId,
};
use layer3_adapter::DesktopOwnership;
use mongodb::{
    bson::{doc, Bson, Document},
    options::Hint,
    ClientSession,
};

use crate::store::sandbox_sessions::{EXPIRY_INDEX, LEGACY_V1_FIELD_COUNT, MAX_SESSION_ID_BYTES};

use super::{ApplyError, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION};

pub(in crate::store::sandbox_maintenance) const MAX_LEASE_ROWS: i64 = 64;

pub(in crate::store::sandbox_maintenance) async fn clear(
    store: &MongoSandboxStore,
    sandbox: &SandboxId,
    observed_at: &str,
    session: &mut ClientSession,
) -> Result<bool, ApplyError> {
    Ok(refusal(store, sandbox, observed_at, session)
        .await?
        .is_none())
}

pub(in crate::store::sandbox_maintenance) async fn refusal(
    store: &MongoSandboxStore,
    sandbox: &SandboxId,
    observed_at: &str,
    session: &mut ClientSession,
) -> Result<Option<Refusal>, ApplyError> {
    let Some(observed_at_ms) = stamp_ms(observed_at) else {
        return Ok(blocked("invalid_observation", Refusal::InvalidActivityTime));
    };
    if has_open_desktop_session(sandbox) {
        return Ok(blocked("local_session", Refusal::OpenSession));
    }
    // An expiry predicate could omit corrupt rows. Bound the complete metadata
    // set instead, with the extra row proving when the limit was exceeded.
    let mut rows = store
        .sandbox_sessions
        .find(doc! {"sandbox_id": sandbox.as_str()})
        .hint(Hint::Name(EXPIRY_INDEX.into()))
        .limit(MAX_LEASE_ROWS + 1)
        .projection(projection())
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    let mut seen = 0;
    while rows
        .advance(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
    {
        seen += 1;
        if seen > MAX_LEASE_ROWS {
            return Ok(blocked("lease_bound", Refusal::SessionEvidenceUnclear));
        }
        let row = rows.deserialize_current().map_err(ApplyError::Driver)?;
        match expired(&row, sandbox, observed_at_ms) {
            Some(true) => {}
            Some(false) => return Ok(blocked("live_lease", Refusal::OpenSession)),
            None => {
                return Ok(blocked(
                    "indeterminate_lease",
                    Refusal::SessionEvidenceUnclear,
                ))
            }
        }
    }
    Ok(None)
}

pub(in crate::store::sandbox_maintenance) fn projection() -> Document {
    doc! {
        "sandbox_id": 1, "document_schema_version": 1, "lease_expires_at": 1,
        "issued_at_ms": 1, "expires_at_ms": 1, "epoch": 1, "runtime_id": 1, "boot_id": 1,
        "database_now": "$$NOW",
        "stored_field_count": {"$size": {"$objectToArray": "$$ROOT"}},
    }
}

pub(in crate::store::sandbox_maintenance) fn expired(
    row: &Document,
    sandbox: &SandboxId,
    observed_at_ms: i64,
) -> Option<bool> {
    // Mongo's equality predicate can also match an array containing the parent ID.
    if row.get_str("sandbox_id").ok()? != sandbox.as_str() {
        return None;
    }
    let session_id = row.get_str("_id").ok()?;
    if session_id.trim().is_empty() || session_id.len() > MAX_SESSION_ID_BYTES {
        return None;
    }
    let versioned = match row.get("document_schema_version") {
        None => false,
        Some(Bson::Int32(version)) if i64::from(*version) == DOCUMENT_SCHEMA_VERSION => true,
        Some(Bson::Int64(version)) if *version == DOCUMENT_SCHEMA_VERSION => true,
        _ => return None,
    };
    let expiry = stamp_ms(row.get_str("lease_expires_at").ok()?)?;
    // A projection can hide unknown fields from the exact legacy shape check.
    if !versioned
        && usize::try_from(row.get_i32("stored_field_count").ok()?).ok()? != LEGACY_V1_FIELD_COUNT
    {
        return None;
    }
    let exclusive = [
        "issued_at_ms",
        "expires_at_ms",
        "epoch",
        "runtime_id",
        "boot_id",
    ]
    .into_iter()
    .any(|field| row.contains_key(field));
    let database_now = row.get_datetime("database_now").ok()?.timestamp_millis();
    if !exclusive {
        return Some(expiry <= observed_at_ms.min(database_now));
    }
    if !versioned {
        return None;
    }
    let authority = DesktopOwnership {
        sandbox_id: sandbox.to_string(),
        session_id: session_id.to_owned(),
        runtime_id: row.get_str("runtime_id").ok()?.to_owned(),
        boot_id: match row.get("boot_id")? {
            Bson::Null => None,
            Bson::String(id) => Some(id.clone()),
            _ => return None,
        },
        epoch: row.get_i64("epoch").ok()?.try_into().ok()?,
        issued_at_ms: row.get_i64("issued_at_ms").ok()?,
        expires_at_ms: row.get_i64("expires_at_ms").ok()?,
    };
    authority.validate().ok()?;
    if expiry != authority.expires_at_ms || authority.issued_at_ms > database_now {
        return None;
    }
    // The lease starts on Mongo's clock, so a fast manager cannot expire it early.
    Some(expiry <= observed_at_ms.min(database_now))
}

fn stamp_ms(stamp: &str) -> Option<i64> {
    i64::try_from(seconds_from_rfc3339(stamp)?)
        .ok()?
        .checked_mul(1000)
}

fn blocked(reason: &'static str, refusal: Refusal) -> Option<Refusal> {
    tracing::debug!(parent: None, reason, "desktop session evidence prevents inactivity pause");
    Some(refusal)
}

#[cfg(test)]
#[path = "sessions_tests.rs"]
mod tests;
