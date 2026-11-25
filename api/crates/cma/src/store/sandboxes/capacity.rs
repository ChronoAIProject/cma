use super::*;
use layer2_core::sandbox::{SandboxCapacitySnapshot, SandboxStatus};

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
        counts.push((status, count as u64));
    }
    Ok(SandboxCapacitySnapshot {
        counts,
        public_owner_limit,
        observed_at: layer2_core::now_rfc3339(),
    })
}

fn unavailable(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}
