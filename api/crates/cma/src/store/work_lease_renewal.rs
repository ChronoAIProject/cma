//! Renew existing work claims without changing their generation or revision.

use layer2_core::sandbox::store::{SandboxStoreError, WORK_LEASE_SECONDS};
use mongodb::bson::{doc, DateTime, Document};
use mongodb::options::ReturnDocument;

use super::sandbox_mongo::DOCUMENT_SCHEMA_VERSION;

pub(super) async fn renew(
    rows: &mongodb::Collection<Document>,
    identity: Document,
    requested_until: &str,
    session: Option<&mut mongodb::ClientSession>,
) -> Result<Option<String>, SandboxStoreError> {
    let requested_until = layer2_core::seconds_from_rfc3339(requested_until)
        .and_then(|value| value.checked_mul(1000))
        .and_then(|value| i64::try_from(value).ok())
        .map(DateTime::from_millis)
        .ok_or(SandboxStoreError::Conflict)?;
    let filter = doc! {"$and":[identity,{
        "document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "$expr":{"$gt":[requested_until,"$$NOW"]},
    }]};
    let deadline = doc! {"$dateToString":{"format":"%Y-%m-%dT%H:%M:%SZ",
    "date":{"$min":[requested_until,{"$dateAdd":{"startDate":"$$NOW","unit":"second",
        "amount":WORK_LEASE_SECONDS as i64}}]},"timezone":"UTC"}};
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
        .projection(doc! {"work_lease_expires_at":1});
    let result = match session {
        Some(session) => update.session(session).await,
        None => update.await,
    }
    .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    result
        .map(|row| {
            row.get_str("work_lease_expires_at")
                .map(str::to_owned)
                .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
        })
        .transpose()
}
