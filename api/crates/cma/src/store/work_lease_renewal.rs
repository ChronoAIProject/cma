//! Renew existing work claims without changing their generation or revision.

use layer2_core::sandbox::store::{SandboxStoreError, WorkLeaseGrant, WORK_LEASE_SECONDS};
use mongodb::bson::{doc, DateTime, Document};
use mongodb::options::ReturnDocument;

use super::work_publication::FenceError;

pub(super) async fn grant(
    rows: &mongodb::Collection<Document>,
    identity: Document,
    requested_until: &str,
    session: Option<&mut mongodb::ClientSession>,
) -> Result<Option<WorkLeaseGrant>, SandboxStoreError> {
    extend(rows, identity, requested_until, session, true)
        .await
        .map_err(renewal_error)
}

pub(super) async fn grant_in_transaction(
    rows: &mongodb::Collection<Document>,
    identity: Document,
    requested_until: &str,
    session: &mut mongodb::ClientSession,
) -> Result<Option<WorkLeaseGrant>, FenceError> {
    extend(rows, identity, requested_until, Some(session), true).await
}

async fn extend(
    rows: &mongodb::Collection<Document>,
    identity: Document,
    requested_until: &str,
    session: Option<&mut mongodb::ClientSession>,
    short: bool,
) -> Result<Option<WorkLeaseGrant>, FenceError> {
    let requested_until = layer2_core::seconds_from_rfc3339(requested_until)
        .and_then(|value| value.checked_mul(1000))
        .and_then(|value| i64::try_from(value).ok())
        .map(DateTime::from_millis)
        .ok_or(SandboxStoreError::Conflict)?;
    let filter = doc! {"$and":[identity,{
        "document_schema_version":super::work_schema::versions(),
        "$expr":{"$gt":[requested_until,"$$NOW"]},
    }]};
    let capped = doc! {"$min":[requested_until,
    {"$dateAdd":{"startDate":"$$NOW","unit":"second",
        "amount":WORK_LEASE_SECONDS as i64}}]};
    let requested = if short {
        capped
    } else {
        // A compatible OFF caller cannot bypass a retained short-claim floor
        // through the older expiry-only API. Never-activated rows stay legacy.
        doc! {"$cond":[{"$eq":["$record.work.authority_protocol","renewed_v1"]},
        capped, requested_until]}
    };
    let deadline = doc! {"$dateToString":{"format":"%Y-%m-%dT%H:%M:%SZ",
    "date":requested,"timezone":"UTC"}};
    let deadline = doc! {"$max":["$work_lease_expires_at",deadline]};
    let update = rows
        .find_one_and_update(
            filter,
            vec![doc! {"$set":{
                "work_lease_expires_at":deadline.clone(),
                "record.work.lease.expires_at":deadline,
            }}],
        )
        .return_document(ReturnDocument::After)
        .projection(doc! {"work_lease_expires_at":1,
        "remaining_millis":{"$subtract":[{"$toDate":"$work_lease_expires_at"},"$$NOW"]}});
    let result = match session {
        Some(session) => update.session(session).await,
        None => update.await,
    }?;
    Ok(result
        .map(|row| {
            let until = row
                .get_str("work_lease_expires_at")
                .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
            let remaining = row
                .get_i64("remaining_millis")
                .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
            if short {
                WorkLeaseGrant::new(until.to_owned(), remaining)
            } else {
                Ok(WorkLeaseGrant {
                    expires_at: until.to_owned(),
                    valid_for: std::time::Duration::from_millis(remaining.max(0) as u64),
                })
            }
        })
        .transpose()?)
}

// Legacy callers retain their expiry-only receipt and long-lease semantics.
pub(super) async fn renew(
    rows: &mongodb::Collection<Document>,
    identity: Document,
    requested_until: &str,
    session: Option<&mut mongodb::ClientSession>,
) -> Result<Option<String>, SandboxStoreError> {
    extend(rows, identity, requested_until, session, false)
        .await
        .map_err(renewal_error)
        .map(|grant| grant.map(|grant| grant.expires_at))
}

fn renewal_error(error: FenceError) -> SandboxStoreError {
    match error {
        FenceError::Domain(error) => error,
        FenceError::Driver(error) => SandboxStoreError::Unavailable(error.to_string()),
    }
}
