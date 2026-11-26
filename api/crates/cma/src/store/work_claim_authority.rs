//! Server-time eligibility for the existing Sandbox/Workspace work projections.

use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::bson::{doc, Bson, DateTime, Document};

pub(super) const LEDGER_MAINTENANCE_FIELD: &str = "ledger_maintenance";

pub(super) fn without_ledger_maintenance(mut filter: Document) -> Document {
    // Any persisted hold blocks ordinary work. Only a future authorized
    // maintenance transition can decide when the Sandbox can serve again.
    filter.insert(LEDGER_MAINTENANCE_FIELD, doc! {"$exists": false});
    filter
}

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

pub(super) const CANONICAL_TIME: &str = r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$";

fn due(field: &str) -> Document {
    // Conversion errors become refusal, never BSON type-order authority.
    let date = doc! {"$convert":{"input":format!("${field}"),"to":"date",
    "onError":Bson::Null,"onNull":Bson::Null}};
    doc! {"$and":[{"$ne":[date.clone(),Bson::Null]},{"$lte":[date,"$$NOW"]}]}
}

pub(super) fn sandbox_claim(
    claimed: &layer2_core::sandbox::Sandbox,
) -> Result<Document, SandboxStoreError> {
    let lease = claimed
        .work
        .lease
        .as_ref()
        .ok_or(SandboxStoreError::Conflict)?;
    let filter = held(
        claimed.id.as_str(),
        claimed.owner.as_str(),
        mongodb::bson::serialize_to_bson(&claimed.work.step)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?,
        &lease.holder,
        lease.fence,
    )?;
    let operation = claimed
        .lifecycle
        .operation
        .as_ref()
        .map(|operation| Bson::String(operation.authority().operation_id.clone()))
        .unwrap_or(Bson::Null);
    Ok(without_ledger_maintenance(doc! {"$and":[filter,{
        "record.lifecycle.operation.authority.operation_id":operation.clone(),
        "$expr":{"$eq":[{"$ifNull":["$record.lifecycle.operation.authority.operation_id",Bson::Null]},
            {"$literal":operation}]},
    }]}))
}

pub(super) fn workspace_claim(
    claimed: &layer2_core::sandbox::Workspace,
) -> Result<Document, SandboxStoreError> {
    let lease = claimed
        .work
        .lease
        .as_ref()
        .ok_or(SandboxStoreError::Conflict)?;
    let filter = held(
        claimed.id.as_str(),
        claimed.owner.as_str(),
        mongodb::bson::serialize_to_bson(&claimed.work.step)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?,
        &lease.holder,
        lease.fence,
    )?;
    Ok(doc! {"$and":[filter,{
        "sandbox_id":claimed.sandbox_id.as_str(),
        "record.sandbox_id":claimed.sandbox_id.as_str(),
        "$expr":{"$and":[
            {"$eq":["$sandbox_id",{"$literal":claimed.sandbox_id.as_str()}]},
            {"$eq":["$record.sandbox_id",{"$literal":claimed.sandbox_id.as_str()}]},
        ]},
    }]})
}

fn held(
    id: &str,
    owner: &str,
    step: Bson,
    holder: &str,
    fence: u64,
) -> Result<Document, SandboxStoreError> {
    let fence = i64::try_from(fence).map_err(|_| SandboxStoreError::Conflict)?;
    if holder.is_empty() || fence <= 0 {
        return Err(SandboxStoreError::Conflict);
    }
    let current = doc! {"$convert":{"input":"$work_lease_expires_at","to":"date",
    "onError":Bson::Null,"onNull":Bson::Null}};
    Ok(doc! {
        "_id":id,"owner":owner,"record.owner":owner,"work_pending":true,
        "record.work.step":step.clone(),"record.work.lease.holder":holder,
        "record.work.lease.fence":fence,
        "record.lifecycle.checkpoint_capture":Bson::Null,
        "$nor":[{"record.lifecycle.retirement_archive.verified":{"$type":"null"}}],
        "channel_reuse_cleanup_execution":{"$exists":false},
        "owned_workspace_cleanup_admission_v1":{"$exists":false},
        "work_lease_expires_at":{"$regex":CANONICAL_TIME},
        "$expr":{"$and":[
            // Match-query equality can select array elements. Authority must
            // retain the exact scalar holder/fence and original work value.
            {"$eq":["$record.work.lease.holder",{"$literal":holder}]},
            {"$eq":["$record.work.lease.fence",fence]},
            {"$in":[{"$type":"$record.work.lease.fence"},["int","long"]]},
            {"$eq":["$record.work.step",{"$literal":step}]},
            {"$eq":["$owner",{"$literal":owner}]},
            {"$eq":["$record.owner",{"$literal":owner}]},
            {"$eq":["$work_lease_expires_at","$record.work.lease.expires_at"]},
            {"$ne":[current.clone(),Bson::Null]}, {"$gt":[current,"$$NOW"]},
        ]},
    })
}
