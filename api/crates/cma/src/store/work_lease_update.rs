//! Preserve revision-stable renewal when an older content snapshot is saved.

use mongodb::bson::{doc, Bson, Document};

pub(super) use layer2_core::sandbox::store::{
    sandbox_content_matches as sandbox_matches, workspace_content_matches as workspace_matches,
};

/// Keep the driver's existing increment validation and overflow behavior.
/// Both writes belong to the caller's transaction; no intermediate state is
/// published, and an increment does not change the content-revision filter.
pub(super) async fn with_fence(
    rows: &mongodb::Collection<Document>,
    filter: Document,
    fields: Option<Document>,
    counter: &str,
    session: &mut mongodb::ClientSession,
) -> Result<mongodb::results::UpdateResult, mongodb::error::Error> {
    let filter = super::work_publication::for_collection(rows, filter)
        .map_err(mongodb::error::Error::custom)?;
    let result = rows
        .update_one(filter.clone(), doc! {"$inc":{counter:1_i64}})
        .session(&mut *session)
        .await?;
    match fields {
        Some(fields) if result.matched_count == 1 => {
            rows.update_one(filter, replacement(fields))
                .session(session)
                .await
        }
        _ => Ok(result),
    }
}

/// Evaluate preservation at the write, including for nontransactional callers.
/// A different holder/fence or a cleared lease still uses the submitted value;
/// the caller's acquisition/publication filter remains responsible for authority.
pub(super) fn replacement(fields: Document) -> Vec<Document> {
    let mut update = fields
        .iter()
        .map(|(key, value)| (key.clone(), Bson::Document(doc! {"$literal":value.clone()})))
        .collect::<Document>();
    if let Some(lease) = fields
        .get_document("record")
        .ok()
        .and_then(|record| record.get_document("work").ok())
        .and_then(|work| work.get_document("lease").ok())
    {
        let same = doc! {"$and":[
            {"$eq":["$record.work.lease.holder",{"$literal":lease.get("holder").cloned().unwrap_or(Bson::Null)}]},
            {"$eq":["$record.work.lease.fence",{"$literal":lease.get("fence").cloned().unwrap_or(Bson::Null)}]},
        ]};
        let record = fields.get_document("record").expect("lease has a record");
        let preserved = doc! {"$mergeObjects":["$$nextRecord",{"work":{
            "$mergeObjects":["$$nextRecord.work",{"lease":{
                "$mergeObjects":["$$nextRecord.work.lease",
                    {"expires_at":"$record.work.lease.expires_at"}]
            }}]
        }}]};
        // Bind the payload once so the update does not double the BSON record
        // size merely to select between retaining and completing a claim.
        update.insert(
            "record",
            doc! {"$let":{"vars":{"nextRecord":{"$literal":record.clone()}},
            "in":{"$cond":[same.clone(),preserved,"$$nextRecord"]}}},
        );
        update.insert("work_lease_expires_at",doc! {"$cond":[same,
            "$work_lease_expires_at",{"$literal":fields.get("work_lease_expires_at").cloned().unwrap_or(Bson::Null)}]});
    }
    if fields
        .get_document("record")
        .ok()
        .is_some_and(|record| record.contains_key("work"))
    {
        let record = update
            .remove("record")
            .expect("work aggregate has a record update");
        update.insert("record", doc! {"$let":{"vars":{"next":record},"in":{
            "$cond":[{"$eq":["$document_schema_version",3_i64]},
                {"$mergeObjects":["$$next",{"work":{"$mergeObjects":["$$next.work",{"authority_protocol":"renewed_v1"}]}}]},
                "$$next"]
        }}});
        update.insert(
            "document_schema_version",
            doc! {"$cond":[
            {"$and":[{"$eq":["$document_schema_version",3_i64]},
                {"$not":[{"$in":[{"$literal":fields.get("document_schema_version").cloned().unwrap_or(Bson::Null)},[
                    super::work_schema::RUNTIME_MIGRATION_VERSION,
                    super::work_schema::RUNTIME_IMAGE_ORIGIN_VERSION,
                    super::work_schema::MANAGED_MACHINE_ORIGIN_VERSION,
                ]]}]}]},3_i64,
            {"$literal":fields.get("document_schema_version").cloned().unwrap_or(Bson::Null)}]},
        );
    }
    vec![doc! {"$set":update}]
}

/// Cap a newly acquired short claim using Mongo time, independently of caller
/// clock skew. Existing long claims keep their original OFF-mode representation.
pub(super) fn acquisition(fields: Document) -> Vec<Document> {
    let short = fields
        .get_document("record")
        .ok()
        .and_then(|record| record.get_document("work").ok())
        .is_some_and(|work| work.get_str("authority_protocol").ok() == Some("renewed_v1"));
    let mut update = replacement(fields);
    if short {
        let deadline = doc! {"$dateToString":{"format":"%Y-%m-%dT%H:%M:%SZ",
        "date":{"$min":[{"$toDate":"$work_lease_expires_at"},
            {"$dateAdd":{"startDate":"$$NOW","unit":"second","amount":15_i64}}]},"timezone":"UTC"}};
        update.push(doc! {"$set":{"work_lease_expires_at":deadline.clone(),"record.work.lease.expires_at":deadline}});
    }
    update
}
