use super::*;
use layer2_core::sandbox::{SandboxCapacitySnapshot, SandboxQuotaUsage, SandboxStatus};
use mongodb::ClientSession;

// Keep this expression aligned with Sandbox::retirement_released_owner_slot.
// Missing settlement facts charge quota for retained rows.
fn owner_slot_released() -> Document {
    doc! {"$and": [
        {"$in": ["$document_schema_version", super::super::work_schema::SANDBOX_VERSIONS.to_vec()]},
        {"$eq": ["$status", "retiring"]},
        {"$eq": ["$record.status", "retiring"]},
        {"$eq": ["$owner", "$record.owner"]},
        {"$eq": [{"$ifNull": ["$record.retired_at", Bson::Null]}, Bson::Null]},
        {"$eq": ["$record.work.step.kind", "mark_retired"]},
        {"$eq": [{"$ifNull": ["$record.runtime", Bson::Null]}, Bson::Null]},
        {"$eq": [{"$ifNull": ["$record.lifecycle.suspended_runtime", Bson::Null]}, Bson::Null]},
        {"$eq": [{"$ifNull": ["$record.credential.current", Bson::Null]}, Bson::Null]},
        {"$eq": [{"$ifNull": ["$record.credential.pending", Bson::Null]}, Bson::Null]},
        {"$eq": ["$record.credential.delivered", false]},
        {"$or": [
            {"$eq": [{"$type": "$record.credential.revoke_debt"}, "missing"]},
            {"$eq": ["$record.credential.revoke_debt", Bson::Array(vec![])]},
        ]},
        {"$or": [
            {"$eq": ["$record.lifecycle.home.released", true]},
            {"$eq": ["$record.lifecycle.home.native_pool.cleaned", true]},
        ]},
    ]}
}

pub(in crate::store) fn charged_owner_slots(owner: &str) -> Document {
    doc! {
        "owner": owner, "retired_at": Bson::Null,
        "$expr": {"$not": [owner_slot_released()]},
    }
}

pub(in crate::store) async fn quota_usage(
    store: &MongoSandboxStore,
    owner: &str,
    session: &mut ClientSession,
) -> Result<SandboxQuotaUsage, mongodb::error::Error> {
    let mut rows = store
        .sandboxes
        .aggregate([
            doc! {"$match": {"owner": owner, "retired_at": Bson::Null, "status": {"$in": ["active", "retiring"]}}},
            doc! {"$group": {
                "_id": Bson::Null,
                "active": {"$sum": {"$cond": [{"$eq": ["$status", "active"]}, 1_i64, 0_i64]}},
                "retiring": {"$sum": {"$cond": [{"$eq": ["$status", "retiring"]}, 1_i64, 0_i64]}},
                "released": {"$sum": {"$cond": [owner_slot_released(), 1_i64, 0_i64]}},
            }},
        ])
        .max_time(std::time::Duration::from_secs(5))
        .session(&mut *session)
        .await?;
    let Some(row) = rows.next(session).await.transpose()? else {
        return Ok(SandboxQuotaUsage::default());
    };
    let count = |key| row.get_i64(key).unwrap_or_default().max(0) as u64;
    let released = count("released");
    Ok(SandboxQuotaUsage {
        active: count("active"),
        retiring_charged: count("retiring").saturating_sub(released),
        retiring_released: released,
    })
}

pub(super) async fn read(
    store: &MongoSandboxStore,
    owner: &SubjectId,
) -> Result<SandboxCapacitySnapshot, SandboxStoreError> {
    let public_owner_limit = store
        .public_owner_sandbox_limit(owner, None)
        .await
        .map_err(unavailable)?;
    // Group every record charged by admission, including unsupported envelopes.
    // Invalid quota facts fail the whole read instead of disappearing from it.
    let pipeline = vec![
        doc! {"$match": {"owner": owner.as_str(), "retired_at": Bson::Null}},
        doc! {"$group": {
            "_id": {
                "status": "$status",
                "owner_slot_released": owner_slot_released(),
                "valid": {"$and": [
                    {"$in": ["$document_schema_version", super::super::work_schema::SANDBOX_VERSIONS.to_vec()]},
                    {"$eq": ["$status", "$record.status"]},
                    {"$eq": ["$owner", "$record.owner"]},
                    {"$eq": [{"$ifNull": ["$record.retired_at", Bson::Null]}, Bson::Null]},
                ]},
            },
            "count": {"$sum": 1_i64},
        }},
    ];
    let rows: Vec<Document> = store
        .sandboxes
        .aggregate(pipeline)
        .read_concern(mongodb::options::ReadConcern::snapshot())
        .max_time(std::time::Duration::from_secs(5))
        .await
        .map_err(unavailable)?
        .try_collect()
        .await
        .map_err(unavailable)?;
    let mut counts = Vec::new();
    let mut retiring_released = 0;
    for row in rows {
        let invalid = || SandboxStoreError::Corrupt("inconsistent Sandbox capacity".into());
        let key = row.get_document("_id").map_err(|_| invalid())?;
        if !matches!(key.get_bool("valid"), Ok(true)) {
            return Err(invalid());
        }
        let status: SandboxStatus =
            mongodb::bson::deserialize_from_bson(key.get("status").cloned().ok_or_else(invalid)?)
                .map_err(|_| invalid())?;
        if status == SandboxStatus::Retired {
            return Err(invalid());
        }
        let count = match row.get("count") {
            Some(Bson::Int64(value)) => *value,
            Some(Bson::Int32(value)) => i64::from(*value),
            _ => return Err(invalid()),
        };
        if count <= 0 {
            return Err(invalid());
        }
        if key.get_bool("owner_slot_released").map_err(|_| invalid())? {
            retiring_released = count as u64;
        } else {
            counts.push((status, count as u64));
        }
    }
    Ok(SandboxCapacitySnapshot {
        counts,
        retiring_released,
        public_owner_limit,
        observed_at: layer2_core::now_rfc3339(),
    })
}

fn unavailable(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}
