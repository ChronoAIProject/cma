//! Preserve permanent references for late checkpoint uploads from the retired bridge.
//! This adapter cannot admit channel work or delete retained history.
use layer2_core::profiles::content_digest;
use mongodb::{
    bson::{doc, Bson, Document},
    ClientSession, Database,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Reference {
    schema_version: u16,
    allocation_id: String,
    kind: String,
    target_id: String,
    target_sha256: String,
}

pub(super) async fn tombstone(
    database: &Database,
    sandbox_id: &str,
    document: &Document,
    session: &mut ClientSession,
) -> Result<(), String> {
    let rows = database.collection::<Document>("bridge_allocations");
    let mut cursor = rows
        .find(doc! {"creates_sandbox":true, "record.placement.target.sandbox_id":sandbox_id})
        .limit(2)
        .session(&mut *session)
        .await
        .map_err(unavailable)?;
    let mut allocation = None;
    while let Some(row) = cursor
        .next(&mut *session)
        .await
        .transpose()
        .map_err(unavailable)?
    {
        if allocation.is_some() {
            return Err("ambiguous legacy allocation".into());
        }
        allocation = Some(row);
    }
    let Some(row) = allocation else {
        return Ok(());
    };
    super::sandbox_mongo::require_version(&row)?;
    let record = row.get_document("record").map_err(unavailable)?;
    let allocation_id = row.get_str("_id").map_err(unavailable)?;
    if record.get_str("id").ok() != Some(allocation_id) {
        return Err("legacy allocation identity mismatch".into());
    }
    if let Some(policy) = row.get("channel_history_policy") {
        let policy = policy
            .as_document()
            .ok_or("invalid legacy history policy")?;
        let mut material = vec![Bson::String("channel-history-allocation-v1".into())];
        for name in [
            "identity",
            "owner",
            "pin",
            "config",
            "first_source",
            "placement",
            "operation_id",
            "predecessor",
            "created_at_ms",
        ] {
            material.push(match record.get(name) {
                Some(value) => value.clone(),
                None if name == "predecessor" => Bson::Null,
                None => return Err("incomplete legacy allocation identity".into()),
            });
        }
        if policy.get_str("allocation_id").ok() != Some(allocation_id)
            || policy.get_str("allocation_sha256").ok()
                != Some(content_digest(&material).map_err(unavailable)?.as_str())
            || policy.get_str("disposition").ok() != Some("retain")
        {
            return Err("legacy history policy identity mismatch".into());
        }
    }
    let count = match row.get("channel_history_reference_count") {
        Some(Bson::Int64(value)) if *value >= 0 => *value,
        None if !row.contains_key("channel_history_policy") => 0,
        _ => return Err("invalid legacy reference count".into()),
    };
    let mut projection = Document::new();
    for field in ["_id", "document_schema_version", "object_key"] {
        projection.insert(
            field,
            document.get(field).cloned().ok_or("incomplete tombstone")?,
        );
    }
    let reference = Reference {
        schema_version: 1,
        allocation_id: allocation_id.into(),
        kind: "late_upload_tombstone".into(),
        target_id: document.get_str("_id").map_err(unavailable)?.into(),
        target_sha256: content_digest(&Bson::Document(projection)).map_err(unavailable)?,
    };
    let id = format!(
        "chr_{}",
        content_digest(&(
            "channel-history-reference-v1",
            allocation_id,
            &reference.kind,
            &reference.target_id
        ))
        .map_err(unavailable)?
    );
    let references = database.collection::<Document>("channel_history_references");
    if let Some(existing) = references
        .find_one(doc! {"_id":&id})
        .session(&mut *session)
        .await
        .map_err(unavailable)?
    {
        let retained: Reference = super::sandbox_mongo::read_record(&existing)?;
        if retained != reference || existing.get_str("allocation_id").ok() != Some(allocation_id) {
            return Err("legacy reference identity mismatch".into());
        }
        return Ok(());
    }
    let changed=rows.update_one(doc! {"_id":allocation_id,"channel_history_reference_count":row.get("channel_history_reference_count").cloned().unwrap_or(Bson::Null)},doc! {"$set":{"channel_history_reference_count":count.checked_add(1).ok_or("legacy reference overflow")?}}).session(&mut *session).await.map_err(unavailable)?;
    if changed.matched_count != 1 {
        return Err("legacy reference count conflict".into());
    }
    references.insert_one(doc! {"_id":id,"document_schema_version":super::schema::DOCUMENT_SCHEMA_VERSION,"allocation_id":allocation_id,"record":super::sandbox_mongo::record_bson(&reference)?}).session(session).await.map_err(unavailable)?;
    Ok(())
}
fn unavailable(error: impl std::fmt::Display) -> String {
    error.to_string()
}
