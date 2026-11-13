//! Server-time eligibility for the existing Sandbox/Workspace work projections.

use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::bson::{doc, Bson, DateTime, Document};

/// Combine with the original revision/parent fence at the acquisition write.
/// Caller time may narrow discovery, but cannot steal a live claim or run work
/// before its persisted due time. The stored canonical string format is intact.
pub(super) fn eligible(until: &str) -> Result<Document, SandboxStoreError> {
    let until = layer2_core::seconds_from_rfc3339(until)
        .and_then(|seconds| seconds.checked_mul(1000))
        .and_then(|millis| i64::try_from(millis).ok())
        .map(DateTime::from_millis)
        .ok_or(SandboxStoreError::Conflict)?;
    Ok(doc! {
        "work_pending":true,
        "work_ready_at":{"$regex":CANONICAL_TIME},
        "$or":[
            {"work_lease_expires_at":Bson::Null},
            {"work_lease_expires_at":{"$regex":CANONICAL_TIME}},
        ],
        "$expr":{"$and":[
            due("work_ready_at"),
            {"$or":[
                {"$in":[{"$type":"$work_lease_expires_at"},["missing","null"]]},
                due("work_lease_expires_at"),
            ]},
            {"$gt":[until,"$$NOW"]},
        ]},
    })
}

const CANONICAL_TIME: &str = r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$";

fn due(field: &str) -> Document {
    // Conversion errors become refusal, never BSON type-order authority.
    let date = doc! {"$convert":{"input":format!("${field}"),"to":"date",
    "onError":Bson::Null,"onNull":Bson::Null}};
    doc! {"$and":[{"$ne":[date.clone(),Bson::Null]},{"$lte":[date,"$$NOW"]}]}
}
